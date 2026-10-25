//! Procesos en marcha: lista, tiempos, memoria, descripción y terminar. Cada handle va en un
//! `Handle`, que lo cierra en todos los caminos.

use std::mem::zeroed;
use std::ptr::null_mut;

use winapi::shared::minwindef::{BOOL, DWORD, FALSE, FILETIME, LPVOID, UINT};
use winapi::shared::winerror::{ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, WAIT_TIMEOUT};
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::processthreadsapi::{
    GetCurrentProcessId, GetProcessTimes, GetSystemTimes, OpenProcess, ProcessIdToSessionId, TerminateProcess,
};
use winapi::um::psapi::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use winapi::um::synchapi::WaitForSingleObject;
use winapi::um::sysinfoapi::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use winapi::um::tlhelp32::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use winapi::um::winbase::QueryFullProcessImageNameW;
use winapi::um::winnt::{HANDLE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, SYNCHRONIZE};
use winapi::um::winver::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use wincleaner::procs::{self, KillTarget, ProcInfo, TermError};

use super::{Handle, wide};

// winapi 0.3.9 no la declara; kernel32 la exporta desde Windows 8.1.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn IsProcessCritical(process: HANDLE, critical: *mut BOOL) -> BOOL;
}

/// `PROCESS_MEMORY_COUNTERS_EX2` (Windows 10 22H2 o posterior), que winapi 0.3.9 no trae.
#[repr(C)]
#[derive(Default)]
struct MemCountersEx2 {
    cb: DWORD,
    page_fault_count: DWORD,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_usage: usize,
    private_working_set_size: usize,
    shared_commit_usage: u64,
}

fn last_error() -> u32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
}

fn ft(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

fn open(pid: u32, access: DWORD) -> Option<Handle> {
    // SAFETY: sin herencia; devuelve nulo si falla.
    let h = unsafe { OpenProcess(access, FALSE, pid) };
    (!h.is_null()).then_some(Handle(h))
}

/// Ruta completa del ejecutable del proceso `h`. Empieza con 1024 caracteres y, si no
/// caben, lo repite con más, hasta el máximo de una ruta larga (32 768).
fn image(h: &Handle) -> Option<String> {
    let mut len = 1024;
    loop {
        let mut buf = vec![0u16; len];
        let mut n = len as DWORD;
        // SAFETY: `buf` mide `n` u16; si tiene éxito, `n` pasa a ser la longitud sin el nulo.
        if unsafe { QueryFullProcessImageNameW(h.0, 0, buf.as_mut_ptr(), &mut n) } != 0 {
            return (n > 0).then(|| String::from_utf16_lossy(&buf[..(n as usize).min(len)]));
        }
        if last_error() != ERROR_INSUFFICIENT_BUFFER || len >= 32768 {
            return None;
        }
        len = (len * 4).min(32768);
    }
}

/// Ruta completa del ejecutable de `pid`, o `None` si ya no existe o no se puede leer.
pub fn exe_of(pid: u32) -> Option<String> {
    image(&open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?)
}

fn info(pid: u32, name: String) -> ProcInfo {
    let mut p = ProcInfo { pid, name, ..ProcInfo::default() };
    let Some(h) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return p };
    p.exe = image(&h);
    // SAFETY: FILETIME son dos enteros: a ceros es válida.
    let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = unsafe { zeroed() };
    // SAFETY: handle válido con QUERY_LIMITED; las cuatro salidas son nuestras.
    if unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) } != 0 {
        p.cpu_100ns = ft(k).saturating_add(ft(u));
    }
    let mut m = MemCountersEx2 { cb: size_of::<MemCountersEx2>() as DWORD, ..Default::default() };
    // SAFETY: `m` empieza como PROCESS_MEMORY_COUNTERS y mide lo que dice `cb`.
    let ok = unsafe { K32GetProcessMemoryInfo(h.0, (&raw mut m).cast(), m.cb) } != 0;
    if ok {
        // Sin EX2 (Windows anterior a 22H2), `private_working_set_size` se queda en 0.
        p.mem = if m.private_working_set_size > 0 { m.private_working_set_size } else { m.working_set_size } as u64;
    } else {
        // SAFETY: estructura C de enteros: a ceros es válida.
        let mut b: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
        b.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as DWORD;
        // SAFETY: igual que arriba, con la estructura básica.
        if unsafe { K32GetProcessMemoryInfo(h.0, &mut b, b.cb) } != 0 {
            p.mem = b.WorkingSetSize as u64;
        }
    }
    let mut crit: BOOL = 0;
    // SAFETY: handle válido; `crit` es nuestro. Si no se puede saber, cuenta como crítico.
    p.critical = unsafe { IsProcessCritical(h.0, &mut crit) } == 0 || crit != 0;
    p
}

/// Todos los procesos menos el inactivo del sistema (PID 0). Si no se puede leer uno, va sin
/// ruta (y `procs::blocked` lo bloquea).
pub fn snapshot() -> Vec<ProcInfo> {
    // SAFETY: sin punteros; devuelve INVALID_HANDLE_VALUE si falla.
    let snap = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
    if snap.0 == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: estructura C de enteros y un array: a ceros es válida; lleva su dwSize.
    let mut e: PROCESSENTRY32W = unsafe { zeroed() };
    e.dwSize = size_of::<PROCESSENTRY32W>() as DWORD;
    // SAFETY: `snap` es una instantánea de procesos y `e` lleva su tamaño.
    let mut more = unsafe { Process32FirstW(snap.0, &mut e) } != 0;
    while more {
        if e.th32ProcessID != 0 {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(info(e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
        }
        // SAFETY: igual que Process32FirstW.
        more = unsafe { Process32NextW(snap.0, &mut e) } != 0;
    }
    out
}

/// `(kernel + usuario, inactivo, memoria usada, memoria total)`. El tiempo de núcleo ya
/// incluye el inactivo, así que el primero es todo el tiempo de CPU de todas las CPU.
pub fn system() -> (u64, u64, u64, u64) {
    // SAFETY: FILETIME son dos enteros: a ceros es válida.
    let (mut i, mut k, mut u): (FILETIME, FILETIME, FILETIME) = unsafe { zeroed() };
    // SAFETY: las tres salidas son nuestras.
    let (total, idle) = if unsafe { GetSystemTimes(&mut i, &mut k, &mut u) } != 0 {
        (ft(k).saturating_add(ft(u)), ft(i))
    } else {
        (0, 0)
    };
    // SAFETY: estructura C de enteros: a ceros es válida; lleva su dwLength.
    let mut m: MEMORYSTATUSEX = unsafe { zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as DWORD;
    // SAFETY: `m` lleva su tamaño.
    let (used, all) = if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
        (m.ullTotalPhys.saturating_sub(m.ullAvailPhys), m.ullTotalPhys)
    } else {
        (0, 0)
    };
    (total, idle, used, all)
}

/// Sesión de Windows del proceso `pid`, si se puede saber.
fn session(pid: u32) -> Option<u32> {
    let mut s = 0;
    // SAFETY: solo escribe en `s`.
    (unsafe { ProcessIdToSessionId(pid, &mut s) } != 0).then_some(s)
}

/// Termina los `pids` que siguen siendo `expected_exe` y son de la sesión de WinCleaner (los de
/// otro usuario no se tocan, ni siquiera como administrador). Cada uno se vuelve a mirar justo
/// antes: si ya no existe o es otro programa (PID reutilizado), no se cuenta. La ruta se lee
/// otra vez con el mismo handle con el que se termina, para que entre medias no lo pueda ocupar
/// otro. Sigue con los demás aunque uno falle; el resultado, según `procs::kill_result`.
pub fn terminate(pids: &[u32], expected_exe: &str) -> Result<(), TermError> {
    let (mut first, mut closed, mut total) = (None, 0, 0);
    let me = own_pid();
    let own = session(me);
    for &pid in pids {
        // WinCleaner no se termina a sí mismo.
        if pid == me {
            continue;
        }
        // Ya no está o es otro programa: no cuenta en el aviso de «se cerraron N de M».
        if procs::kill_target(expected_exe, exe_of(pid).as_deref()) != KillTarget::Kill {
            continue;
        }
        total += 1;
        if !procs::same_session(session(pid), own) {
            continue;
        }
        match kill(pid, expected_exe) {
            Ok(()) => closed += 1,
            Err(e) => {
                first.get_or_insert(e);
            }
        }
    }
    procs::kill_result(closed, total, first)
}

fn kill(pid: u32, expected_exe: &str) -> Result<(), TermError> {
    let access = PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE;
    let Some(h) = open(pid, access) else {
        return match last_error() {
            // Ya no existe.
            ERROR_INVALID_PARAMETER => Ok(()),
            ERROR_ACCESS_DENIED => Err(TermError::Denied),
            e => Err(TermError::Other(e)),
        };
    };
    if procs::kill_target(expected_exe, image(&h).as_deref()) != KillTarget::Kill {
        return Ok(());
    }
    let mut crit: BOOL = 0;
    // SAFETY: handle válido; `crit` es nuestro. Ante la duda, no se termina.
    if unsafe { IsProcessCritical(h.0, &mut crit) } == 0 || crit != 0 {
        return Err(TermError::Denied);
    }
    // SAFETY: handle con PROCESS_TERMINATE de un proceso que acabamos de comprobar.
    if unsafe { TerminateProcess(h.0, 1) } != 0 {
        return Ok(());
    }
    let e = last_error();
    // Si ya estaba terminando, TerminateProcess falla con acceso denegado: no es un error.
    // SAFETY: handle con SYNCHRONIZE; espera 0 ms.
    if unsafe { WaitForSingleObject(h.0, 0) } != WAIT_TIMEOUT {
        return Ok(());
    }
    Err(if e == ERROR_ACCESS_DENIED { TermError::Denied } else { TermError::Other(e) })
}

/// PID de WinCleaner.
pub fn own_pid() -> u32 {
    // SAFETY: sin argumentos.
    unsafe { GetCurrentProcessId() }
}

/// `FileDescription` del ejecutable, en el primer idioma de `\VarFileInfo\Translation`.
pub fn description(exe: &str) -> Option<String> {
    version_string(exe, "FileDescription")
}

/// `CompanyName` del ejecutable (el editor), igual que `description`.
pub fn company(exe: &str) -> Option<String> {
    version_string(exe, "CompanyName")
}

/// `ProductName` del archivo (p. ej. un instalador), igual que `description`.
pub fn product_name(exe: &str) -> Option<String> {
    version_string(exe, "ProductName")
}

fn version_string(exe: &str, key: &str) -> Option<String> {
    let w = wide(exe);
    let mut dummy: DWORD = 0;
    // SAFETY: ruta terminada en nulo; `dummy` es nuestro.
    let size = unsafe { GetFileVersionInfoSizeW(w.as_ptr(), &mut dummy) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` mide `size` bytes.
    if unsafe { GetFileVersionInfoW(w.as_ptr(), 0, size, buf.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let query = |key: &str| -> Option<(*const u8, UINT)> {
        let k = wide(key);
        let mut p: LPVOID = null_mut();
        let mut n: UINT = 0;
        // SAFETY: `buf` es el bloque que rellenó GetFileVersionInfoW; `p` apunta dentro de él.
        let ok = unsafe { VerQueryValueW(buf.as_ptr().cast(), k.as_ptr(), &mut p, &mut n) };
        (ok != 0 && !p.is_null() && n > 0).then_some((p as *const u8, n))
    };
    let (t, n) = query(r"\VarFileInfo\Translation")?;
    if n < 4 {
        return None;
    }
    // SAFETY: hay al menos 4 bytes dentro de `buf`: idioma y página de códigos (u16 cada uno).
    let (lang, cp) = unsafe { (t.cast::<u16>().read_unaligned(), t.add(2).cast::<u16>().read_unaligned()) };
    let (p, n) = query(&format!(r"\StringFileInfo\{lang:04x}{cp:04x}\{key}"))?;
    // SAFETY: para cadenas, VerQueryValueW da `n` caracteres UTF-16 (con el nulo) dentro de
    // `buf`; se leen sin suponer alineación.
    let s: Vec<u16> = (0..n as usize).map(|i| unsafe { p.cast::<u16>().add(i).read_unaligned() }).collect();
    let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    let d = String::from_utf16_lossy(&s[..len]).trim().to_string();
    (!d.is_empty()).then_some(d)
}
