//! Lo que arranca solo sin estar en `Run` ni en Inicio: servicios automáticos y tareas
//! programadas con disparador de arranque o de inicio de sesión. Solo se leen; lo que no se
//! deja leer (acceso denegado…) se salta sin fallar la lista entera.
#![expect(dead_code, reason = "lo usa la Task 14")]

use std::ptr::{null, null_mut};

use winapi::shared::minwindef::DWORD;
use winapi::shared::winerror::ERROR_MORE_DATA;
use winapi::shared::wtypesbase::CLSCTX_INPROC_SERVER;
use winapi::um::combaseapi::CoCreateInstance;
use winapi::um::taskschd::{
    IAction, IActionCollection, IExecAction, IRegisteredTask, IRegisteredTaskCollection, ITaskDefinition,
    ITaskFolder, ITaskFolderCollection, ITaskService, ITrigger, ITriggerCollection, TASK_ACTION_EXEC,
    TASK_ENUM_HIDDEN, TASK_TRIGGER_BOOT, TASK_TRIGGER_LOGON, TaskScheduler,
};
use winapi::um::winnt::{SERVICE_AUTO_START, SERVICE_WIN32};
use winapi::um::winsvc::{
    CloseServiceHandle, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW,
    QUERY_SERVICE_CONFIGW, QueryServiceConfigW, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_QUERY_CONFIG, SERVICE_STATE_ALL,
};
use winapi::{Class, Interface};
use wincleaner::cmdline;

use super::wmi::{Bstr, Variant};
use super::{Com, reg, system32, wide, windows_dir, with_com};

/// Manejador del Administrador de servicios; se cierra al soltarlo.
struct Sc(SC_HANDLE);

impl Drop for Sc {
    fn drop(&mut self) {
        // SAFETY: lo abrió OpenSCManagerW u OpenServiceW, no es nulo y solo se cierra aquí.
        unsafe { CloseServiceHandle(self.0) };
    }
}

/// Cadena UTF-16 terminada en nulo; vacía si es nula.
///
/// # Safety
/// `p` es nulo o apunta a una cadena terminada en nulo.
unsafe fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: termina en nulo, así que se lee hasta él.
    let s = unsafe {
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        std::slice::from_raw_parts(p, len)
    };
    String::from_utf16_lossy(s)
}

/// Servicios de inicio automático (también los retrasados): `(nombre visible, ejecutable)`.
/// Se saltan los que corren dentro de `svchost.exe` y los de ruta que no se entiende.
pub fn services() -> Vec<(String, String)> {
    let (Some(root), Some(sys32)) = (windows_dir(), system32()) else { return Vec::new() };
    // SAFETY: equipo local y base de datos por defecto.
    let scm = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE) };
    if scm.is_null() {
        return Vec::new();
    }
    let scm = Sc(scm);
    let mut out = Vec::new();
    for (name, display) in list(&scm) {
        let Some(image) = auto_start_image(&scm, &name) else { continue };
        let Some(exe) = cmdline::service_exe(&reg::expand_env(&image), &root, &sys32) else { continue };
        let file = exe.rsplit('\\').next().unwrap_or(&exe);
        if !file.eq_ignore_ascii_case("svchost.exe") {
            out.push((if display.is_empty() { name } else { display }, exe));
        }
    }
    out
}

/// `(nombre interno, nombre visible)` de todos los servicios Win32 (sin controladores).
fn list(scm: &Sc) -> Vec<(String, String)> {
    // 256 KiB es lo más que acepta una llamada; con más, se sigue con `resume`.
    let mut buf = vec![0u64; 256 * 1024 / 8];
    let bytes = (buf.len() * 8) as DWORD;
    let (mut resume, mut out) = (0, Vec::new());
    loop {
        let (mut needed, mut n) = (0, 0);
        // SAFETY: `buf` mide `bytes` y está alineado para ENUM_SERVICE_STATUS_PROCESSW (u64).
        let ok = unsafe {
            EnumServicesStatusExW(
                scm.0, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, buf.as_mut_ptr().cast(), bytes,
                &mut needed, &mut n, &mut resume, null(),
            )
        };
        let more = ok == 0 && std::io::Error::last_os_error().raw_os_error() == Some(ERROR_MORE_DATA as i32);
        if ok == 0 && !more {
            break;
        }
        // SAFETY: con éxito (o ERROR_MORE_DATA), el principio de `buf` son `n` entradas cuyas
        // cadenas apuntan dentro de `buf`; se copian antes de la siguiente llamada.
        unsafe {
            let entries = std::slice::from_raw_parts(buf.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(), n as usize);
            out.extend(entries.iter().map(|e| (pwstr(e.lpServiceName), pwstr(e.lpDisplayName))));
        }
        // Sin avance (n == 0) se pararía en bucle: no debería pasar con 256 KiB.
        if !more || n == 0 {
            break;
        }
    }
    out
}

/// `ImagePath` del servicio `name`, tal cual (sin expandir), si es de inicio automático;
/// `None` si no lo es o no se deja leer.
fn auto_start_image(scm: &Sc, name: &str) -> Option<String> {
    let w = wide(name);
    // SAFETY: `w` termina en nulo; solo se pide leer la configuración.
    let h = unsafe { OpenServiceW(scm.0, w.as_ptr(), SERVICE_QUERY_CONFIG) };
    if h.is_null() {
        return None;
    }
    let h = Sc(h);
    let mut needed = 0;
    // SAFETY: búfer nulo y tamaño 0: falla con ERROR_INSUFFICIENT_BUFFER y dice cuánto hace falta.
    unsafe { QueryServiceConfigW(h.0, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` mide al menos `needed` y está alineado para QUERY_SERVICE_CONFIGW.
    let cfg = unsafe {
        if QueryServiceConfigW(h.0, buf.as_mut_ptr().cast(), (buf.len() * 8) as DWORD, &mut needed) == 0 {
            return None;
        }
        &*buf.as_ptr().cast::<QUERY_SERVICE_CONFIGW>()
    };
    if cfg.dwStartType != SERVICE_AUTO_START {
        return None;
    }
    // SAFETY: apunta dentro de `buf`, que sigue vivo.
    Some(unsafe { pwstr(cfg.lpBinaryPathName) })
}

/// Tareas programadas (también las ocultas) con disparador de arranque o de inicio de sesión:
/// `(ruta de la tarea, ejecutable)` por cada acción que lanza un programa.
pub fn boot_tasks() -> Vec<(String, String)> {
    let Some(sys32) = system32() else { return Vec::new() };
    with_com(|| {
        let clsid = TaskScheduler::uuidof();
        // SAFETY: CLSID e IID válidos; si tiene éxito deja un ITaskService nuestro.
        let Ok(svc) = (unsafe {
            Com::<ITaskService>::out(|p| {
                CoCreateInstance(&clsid, null_mut(), CLSCTX_INPROC_SERVER, &ITaskService::uuidof(), p.cast())
            })
        }) else {
            return Vec::new();
        };
        let e = Variant::empty();
        // SAFETY: cuatro VT_EMPTY: equipo local y usuario actual.
        if unsafe { svc.Connect(e.0, e.0, e.0, e.0) } < 0 {
            return Vec::new();
        }
        let root = Bstr::new("\\");
        // SAFETY: `root` vive durante la llamada.
        let Ok(folder) = (unsafe { Com::<ITaskFolder>::out(|p| svc.GetFolder(root.0, p)) }) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        walk(&folder, &sys32, &mut out);
        out
    })
}

/// Las tareas de `folder` y, después, las de sus subcarpetas.
fn walk(folder: &Com<ITaskFolder>, sys32: &str, out: &mut Vec<(String, String)>) {
    // SAFETY: si tiene éxito, deja una colección nuestra.
    if let Ok(tasks) = unsafe { Com::<IRegisteredTaskCollection>::out(|p| folder.GetTasks(TASK_ENUM_HIDDEN as i32, p)) } {
        let mut n = 0;
        // SAFETY: solo escribe en `n`.
        unsafe { tasks.get_Count(&mut n) };
        for i in 1..=n {
            // SAFETY: índice VT_I4 desde 1; si tiene éxito, deja una tarea nuestra.
            if let Ok(t) = unsafe { Com::<IRegisteredTask>::out(|p| tasks.get_Item(Variant::int(i), p)) } {
                task(&t, sys32, out);
            }
        }
    }
    // SAFETY: si tiene éxito, deja una colección nuestra.
    if let Ok(subs) = unsafe { Com::<ITaskFolderCollection>::out(|p| folder.GetFolders(0, p)) } {
        let mut n = 0;
        // SAFETY: solo escribe en `n`.
        unsafe { subs.get_Count(&mut n) };
        for i in 1..=n {
            // SAFETY: índice VT_I4 desde 1; si tiene éxito, deja una carpeta nuestra.
            if let Ok(f) = unsafe { Com::<ITaskFolder>::out(|p| subs.get_Item(Variant::int(i), p)) } {
                walk(&f, sys32, out);
            }
        }
    }
}

fn task(t: &Com<IRegisteredTask>, sys32: &str, out: &mut Vec<(String, String)>) {
    // SAFETY: si tiene éxito, deja una definición nuestra (con acceso denegado, falla y se salta).
    let Ok(def) = (unsafe { Com::<ITaskDefinition>::out(|p| t.get_Definition(p)) }) else { return };
    if !at_boot_or_logon(&def) {
        return;
    }
    // SAFETY: si tiene éxito, deja una colección nuestra.
    let Ok(actions) = (unsafe { Com::<IActionCollection>::out(|p| def.get_Actions(p)) }) else { return };
    let mut path = Bstr::null();
    // SAFETY: deja en `path` un BSTR nuestro (o nulo).
    unsafe { t.get_Path(&mut path.0) };
    let mut n = 0;
    // SAFETY: solo escribe en `n`.
    unsafe { actions.get_Count(&mut n) };
    for i in 1..=n {
        // SAFETY: índice desde 1; si tiene éxito, deja una acción nuestra.
        let Ok(a) = (unsafe { Com::<IAction>::out(|p| actions.get_Item(i, p)) }) else { continue };
        let mut kind = 0;
        // SAFETY: solo escribe en `kind`.
        if unsafe { a.get_Type(&mut kind) } < 0 || kind != TASK_ACTION_EXEC {
            continue;
        }
        // SAFETY: es una acción de ejecutar, así que da un IExecAction nuestro.
        let Ok(exec) = (unsafe { Com::<IExecAction>::out(|p| a.QueryInterface(&IExecAction::uuidof(), p.cast())) }) else {
            continue;
        };
        let mut exe = Bstr::null();
        // SAFETY: deja en `exe` un BSTR nuestro (o nulo).
        unsafe { exec.get_Path(&mut exe.0) };
        let exe = reg::expand_env(&exe.text());
        // Puede venir entre comillas; sin ruta absoluta (o nombre suelto, bajo System32) no se mira.
        if let Some(exe) = cmdline::resolve(exe.trim().trim_matches('"'), sys32) {
            out.push((path.text(), exe));
        }
    }
}

fn at_boot_or_logon(def: &Com<ITaskDefinition>) -> bool {
    // SAFETY: si tiene éxito, deja una colección nuestra.
    let Ok(triggers) = (unsafe { Com::<ITriggerCollection>::out(|p| def.get_Triggers(p)) }) else { return false };
    let mut n = 0;
    // SAFETY: solo escribe en `n`.
    unsafe { triggers.get_Count(&mut n) };
    (1..=n).any(|i| {
        // SAFETY: índice desde 1; si tiene éxito, deja un disparador nuestro.
        let Ok(tr) = (unsafe { Com::<ITrigger>::out(|p| triggers.get_Item(i, p)) }) else { return false };
        let mut kind = 0;
        // SAFETY: solo escribe en `kind`.
        (unsafe { tr.get_Type(&mut kind) }) >= 0 && (kind == TASK_TRIGGER_BOOT || kind == TASK_TRIGGER_LOGON)
    })
}
