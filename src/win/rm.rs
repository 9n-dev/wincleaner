//! Restart Manager: qué programas tienen abiertos unos archivos.

use std::path::PathBuf;
use std::ptr::null_mut;

use winapi::shared::minwindef::{DWORD, UINT};
use winapi::shared::winerror::{ERROR_MORE_DATA, ERROR_SUCCESS};
use winapi::um::restartmanager::{
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources, RmStartSession,
};
use winapi::um::winnt::LPCWSTR;

use super::wide;

/// PID y nombre de los programas que tienen abierto alguno de `files`, sin repetir nombre.
/// Vacío si algo falla. Bloquea: va en el hilo de trabajo.
pub fn who(files: &[PathBuf]) -> Vec<(u32, String)> {
    if files.is_empty() {
        return Vec::new();
    }
    let mut session: DWORD = 0;
    let mut key = [0u16; CCH_RM_SESSION_KEY + 1];
    // SAFETY: `session` es un DWORD válido y `key` mide CCH_RM_SESSION_KEY + 1 u16, lo que pide.
    if unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let out = list(session, files);
    // SAFETY: la sesión se abrió arriba y se cierra una sola vez, haya ido bien `list` o no.
    unsafe { RmEndSession(session) };
    out
}

fn list(session: DWORD, files: &[PathBuf]) -> Vec<(u32, String)> {
    let names: Vec<Vec<u16>> = files.iter().map(wide).collect();
    let mut ptrs: Vec<LPCWSTR> = names.iter().map(|w| w.as_ptr()).collect();
    let Ok(n) = UINT::try_from(ptrs.len()) else { return Vec::new() };
    // SAFETY: `ptrs` tiene `n` punteros a cadenas UTF-16 terminadas en nulo que viven en `names`
    // hasta el final de la función; sin aplicaciones ni servicios (0 y nulo).
    if unsafe { RmRegisterResources(session, n, ptrs.as_mut_ptr(), 0, null_mut(), 0, null_mut()) } != ERROR_SUCCESS {
        return Vec::new();
    }
    // Primero sin búfer, para saber cuántos hay; después, con el tamaño que pide. Si entre
    // medias abre el archivo otro programa más, vuelve a faltar sitio y se da por perdido.
    let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
    for _ in 0..2 {
        let (mut needed, mut reasons): (UINT, DWORD) = (0, 0);
        let Ok(mut got) = UINT::try_from(infos.len()) else { return Vec::new() };
        let buf = if infos.is_empty() { null_mut() } else { infos.as_mut_ptr() };
        // SAFETY: `buf` es nulo con `got == 0` o apunta a `infos`, que tiene `got` elementos;
        // los demás punteros son variables locales válidas.
        match unsafe { RmGetList(session, &mut needed, &mut got, buf, &mut reasons) } {
            ERROR_SUCCESS => {
                infos.truncate(got as usize);
                return programs(&infos);
            }
            ERROR_MORE_DATA => {
                // SAFETY: RM_PROCESS_INFO solo tiene enteros y arrays de u16: todo ceros es válido.
                let zero: RM_PROCESS_INFO = unsafe { std::mem::zeroed() };
                infos = vec![zero; needed as usize];
            }
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// `(pid, nombre)` de cada programa, con el nombre hasta el primer nulo y sin repetirlo.
fn programs(infos: &[RM_PROCESS_INFO]) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    for i in infos {
        let len = i.strAppName.iter().position(|&c| c == 0).unwrap_or(i.strAppName.len());
        let name = String::from_utf16_lossy(&i.strAppName[..len]);
        if !name.is_empty() && !out.iter().any(|o| o.1 == name) {
            out.push((i.Process.dwProcessId, name));
        }
    }
    out
}
