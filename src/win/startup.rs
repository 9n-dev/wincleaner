//! Programas que arrancan con Windows: valores `Run` del registro y carpetas Inicio. Activar o
//! desactivar solo escribe su valor en `StartupApproved`, como el Administrador de tareas: nunca
//! se toca el valor `Run` ni el acceso directo.

use std::path::Path;
use std::ptr::null_mut;

use winapi::Interface;
use winapi::ctypes::c_void;
use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::FILETIME;
use winapi::shared::winerror::ERROR_ACCESS_DENIED;
use winapi::shared::wtypesbase::CLSCTX_INPROC_SERVER;
use winapi::um::combaseapi::CoCreateInstance;
use winapi::um::knownfolders::{FOLDERID_CommonStartup, FOLDERID_Startup};
use winapi::um::objidl::IPersistFile;
use winapi::um::shobjidl_core::IShellLinkW;
use winapi::um::sysinfoapi::GetSystemTimeAsFileTime;
use wincleaner::cmdline;
use wincleaner::programs::Hive;
use wincleaner::startup::{self, Source};

use super::{Com, is_admin, known_folder, reg, wide};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// `CLSID_ShellLink`, que winapi 0.3.9 no trae.
const CLSID_SHELL_LINK: GUID =
    GUID { Data1: 0x0002_1401, Data2: 0, Data3: 0, Data4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46] };

#[derive(Clone, Debug)]
pub struct Item {
    pub source: Source,
    /// El nombre del valor del `Run` o el del archivo sin extensión.
    pub name: String,
    /// El nombre del valor en `StartupApproved`: en las carpetas, el archivo con extensión.
    pub approved_name: String,
    /// Ruta absoluta del ejecutable (solo para enseñarlo: icono y firma), si se sabe.
    pub exe: Option<String>,
    pub enabled: bool,
}

/// Todo lo que arranca con Windows, en este orden: registro del usuario, del equipo (64 y 32
/// bits) y carpetas Inicio (del usuario y de todos).
pub fn list() -> Vec<Item> {
    let system32 = super::system32().unwrap_or_default();
    // Sin ruta absoluta con letra (UNC, relativa…), no se enseña ni se mira: nada de red.
    let abs = |e: &str| cmdline::resolve(e, &system32);
    let mut out = Vec::new();
    for (hive, source) in [(Hive::User, Source::RunUser), (Hive::Machine64, Source::RunMachine), (Hive::Machine32, Source::RunMachine32)] {
        for (name, cmd) in reg::values(hive, RUN) {
            let exe = cmdline::split_exe(&reg::expand_env(&cmd)).and_then(|(e, _)| abs(&e));
            out.push(item(source, name.clone(), name, exe));
        }
    }
    super::with_com(|| {
        for (id, source) in [(&FOLDERID_Startup, Source::FolderUser), (&FOLDERID_CommonStartup, Source::FolderCommon)] {
            let Some(dir) = known_folder(id) else { continue };
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<_> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).collect();
            files.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
            for f in files {
                let file = f.file_name().to_string_lossy().into_owned();
                if file.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                let path = f.path();
                let name = path.file_stem().map_or(file.clone(), |s| s.to_string_lossy().into_owned());
                let is_lnk = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"));
                let target = if is_lnk { link_target(&path) } else { Some(path.to_string_lossy().into_owned()) };
                out.push(item(source, name, file, target.and_then(|t| abs(&t))));
            }
        }
    });
    out
}

fn item(source: Source, name: String, approved_name: String, exe: Option<String>) -> Item {
    let (hive, key) = startup::approved_key(source);
    let enabled = startup::is_enabled(reg::read_binary(hive, key, &approved_name).as_deref());
    Item { source, name, approved_name, exe, enabled }
}

/// Activa o desactiva `item`; el error es el código de Win32. Los del equipo necesitan
/// administrador: sin serlo, no se intenta.
pub fn set_enabled(item: &Item, on: bool) -> Result<(), u32> {
    if startup::needs_admin(item.source) && !is_admin() {
        return Err(ERROR_ACCESS_DENIED);
    }
    let mut ft = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    // SAFETY: solo escribe en `ft`.
    unsafe { GetSystemTimeAsFileTime(&mut ft) };
    let now = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    let (hive, key) = startup::approved_key(item.source);
    reg::write_binary(hive, key, &item.approved_name, &startup::encode(on, now))
}

/// Destino del acceso directo `lnk`, tal cual lo guarda (sin `Resolve`: no se busca nada).
/// Necesita COM iniciado en el hilo.
fn link_target(lnk: &Path) -> Option<String> {
    let mut p: *mut c_void = null_mut();
    // SAFETY: CLSID e IID válidos; si tiene éxito, `p` es un IShellLinkW nuestro.
    let hr = unsafe { CoCreateInstance(&CLSID_SHELL_LINK, null_mut(), CLSCTX_INPROC_SERVER, &IShellLinkW::uuidof(), &mut p) };
    if hr < 0 || p.is_null() {
        return None;
    }
    let link = Com(p.cast::<IShellLinkW>());
    let mut q: *mut c_void = null_mut();
    // SAFETY: `link` es válido; si tiene éxito, `q` es un IPersistFile nuestro.
    let hr = unsafe { (*link.0).QueryInterface(&IPersistFile::uuidof(), &mut q) };
    if hr < 0 || q.is_null() {
        return None;
    }
    let file = Com(q.cast::<IPersistFile>());
    let w = wide(lnk);
    // SAFETY: ruta UTF-16 terminada en nulo; 0 = STGM_READ.
    if unsafe { (*file.0).Load(w.as_ptr(), 0) } < 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    // SAFETY: `buf` mide lo que se le dice; sin WIN32_FIND_DATAW; 0 = la ruta normal.
    let hr = unsafe { (*link.0).GetPath(buf.as_mut_ptr(), buf.len() as i32, null_mut(), 0) };
    // S_FALSE (1) es «sin ruta» (p. ej. un acceso directo a algo que no es un archivo).
    if hr != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    (len > 0).then(|| String::from_utf16_lossy(&buf[..len]))
}
