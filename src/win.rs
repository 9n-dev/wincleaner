//! Capa fina sobre Win32: entorno, permisos, papelera, unidades y Explorador.

use std::ffi::{OsStr, OsString};
use std::iter::once;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};

use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::{BOOL, DWORD};
use winapi::shared::ntdef::{PULARGE_INTEGER, PWSTR};
use winapi::shared::windef::HWND;
use winapi::shared::winerror::{E_UNEXPECTED, S_OK};
use winapi::um::combaseapi::CoTaskMemFree;
use winapi::um::fileapi::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetLongPathNameW,
};
use winapi::um::knownfolders::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData,
    FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_ProgramData,
    FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, FOLDERID_System, FOLDERID_Videos,
    FOLDERID_Windows,
};
use winapi::um::shellapi::{
    SHEmptyRecycleBinW, SHQUERYRBINFO, SHQueryRecycleBinW, ShellExecuteW, SHERB_NOCONFIRMATION,
    SHERB_NOPROGRESSUI, SHERB_NOSOUND,
};
use winapi::um::shlobj::SHGetKnownFolderPath;
use winapi::um::winbase::DRIVE_FIXED;
use winapi::um::winuser::{IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW, SW_SHOWNORMAL};

use wincleaner::categories::Env;

// winapi 0.3.9 no declara IsUserAnAdmin, aunque shell32 la exporta.
#[link(name = "shell32")]
unsafe extern "system" {
    fn IsUserAnAdmin() -> BOOL;
}

pub struct Drive {
    pub root: String,
    pub free: u64,
    pub total: u64,
}

pub fn gather_env() -> Env {
    let folders = |ids: &[&GUID]| {
        ids.iter()
            .filter_map(|id| known_folder(id))
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    let mut protected = drive_roots();
    protected.extend(folders(&[
        &FOLDERID_Profile,
        &FOLDERID_Windows,
        &FOLDERID_System,
        &FOLDERID_ProgramFiles,
        &FOLDERID_ProgramFilesX86,
    ]));
    Env {
        temp: std::env::var_os("TEMP").map(|t| long_path(Path::new(&t))),
        system_root: known_folder(&FOLDERID_Windows),
        local_app_data: known_folder(&FOLDERID_LocalAppData),
        program_data: known_folder(&FOLDERID_ProgramData),
        protected,
        forbidden_trees: folders(&[
            &FOLDERID_Documents,
            &FOLDERID_Desktop,
            &FOLDERID_Downloads,
            &FOLDERID_Pictures,
            &FOLDERID_Music,
            &FOLDERID_Videos,
        ]),
        is_admin: is_admin(),
    }
}

/// Aviso con Aceptar/Cancelar y Cancelar por defecto, para que una doble pulsación
/// de Intro o Espacio no confirme nada. `true` solo con Aceptar.
pub fn confirm_warning(parent: HWND, title: &str, text: &str) -> bool {
    let (title, text) = (wide(title), wide(text));
    let flags = MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2;
    // SAFETY: cadenas UTF-16 terminadas en nulo que viven durante la llamada; `parent` puede ser nulo.
    unsafe { MessageBoxW(parent, text.as_ptr(), title.as_ptr(), flags) == IDOK }
}

pub fn is_admin() -> bool {
    // SAFETY: sin argumentos; solo consulta el token del proceso.
    unsafe { IsUserAnAdmin() != 0 }
}

pub fn relaunch_as_admin() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let (verb, file) = (wide("runas"), wide(&exe));
    // SAFETY: cadenas UTF-16 terminadas en nulo que viven durante la llamada.
    let h = unsafe {
        ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL)
    };
    h as isize > 32
}

/// `(bytes, elementos)` de la papelera de todas las unidades.
pub fn recycle_bin_size() -> (u64, u64) {
    let mut info = SHQUERYRBINFO {
        cbSize: size_of::<SHQUERYRBINFO>() as DWORD,
        i64Size: 0,
        i64NumItems: 0,
    };
    // SAFETY: raíz nula = todas las unidades; `info` lleva su cbSize.
    let hr = unsafe { SHQueryRecycleBinW(null(), &mut info) };
    if hr < 0 {
        return (0, 0);
    }
    (info.i64Size.max(0) as u64, info.i64NumItems.max(0) as u64)
}

pub fn empty_recycle_bin() -> bool {
    let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
    // SAFETY: sin ventana ni raíz (todas las unidades).
    let hr = unsafe { SHEmptyRecycleBinW(null_mut(), null(), flags) };
    // Con la papelera ya vacía, según la versión, devuelve S_OK o E_UNEXPECTED.
    hr == S_OK || hr == E_UNEXPECTED
}

pub fn fixed_drives() -> Vec<Drive> {
    drive_roots()
        .into_iter()
        .filter_map(|root| {
            let w = wide(&root);
            // SAFETY: `w` es una ruta UTF-16 terminada en nulo.
            if unsafe { GetDriveTypeW(w.as_ptr()) } != DRIVE_FIXED {
                return None;
            }
            let (mut free, mut total) = (0u64, 0u64);
            // SAFETY: ULARGE_INTEGER es un u64 (repr C); el tercer puntero puede ser nulo.
            let ok = unsafe {
                GetDiskFreeSpaceExW(
                    w.as_ptr(),
                    &mut free as *mut u64 as PULARGE_INTEGER,
                    &mut total as *mut u64 as PULARGE_INTEGER,
                    null_mut(),
                )
            };
            (ok != 0).then_some(Drive { root, free, total })
        })
        .collect()
}

/// Abre el Explorador con `path` seleccionado. Se lanza por ruta absoluta: con solo
/// `explorer.exe`, Windows buscaría antes en la carpeta del `.exe` portable.
pub fn open_in_explorer(path: &Path) {
    let Some(windows) = known_folder(&FOLDERID_Windows) else { return };
    let _ = Command::new(windows.join("explorer.exe"))
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
}

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(once(0)).collect()
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut p: PWSTR = null_mut();
    // SAFETY: `p` recibe un búfer de CoTaskMemAlloc (o nulo) que liberamos abajo.
    let hr = unsafe { SHGetKnownFolderPath(id, 0, null_mut(), &mut p) };
    let out = (hr == S_OK && !p.is_null()).then(|| {
        // SAFETY: con S_OK, `p` es una cadena UTF-16 terminada en nulo.
        let s = unsafe {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            std::slice::from_raw_parts(p, len)
        };
        PathBuf::from(OsString::from_wide(s))
    });
    // SAFETY: hay que liberarlo también si falla; CoTaskMemFree acepta nulo.
    unsafe { CoTaskMemFree(p.cast()) };
    out
}

fn long_path(p: &Path) -> PathBuf {
    let w = wide(p);
    // SAFETY: búfer nulo y tamaño 0: devuelve el tamaño necesario con el nulo.
    let n = unsafe { GetLongPathNameW(w.as_ptr(), null_mut(), 0) };
    if n == 0 {
        return p.to_path_buf();
    }
    let mut buf = vec![0u16; n as usize];
    // SAFETY: `buf` tiene `n` u16; si basta, devuelve la longitud sin el nulo.
    let m = unsafe { GetLongPathNameW(w.as_ptr(), buf.as_mut_ptr(), n) };
    if m == 0 || m >= n {
        return p.to_path_buf();
    }
    PathBuf::from(OsString::from_wide(&buf[..m as usize]))
}

/// Raíces de todas las unidades (`C:\`, `D:\`…), de cualquier tipo.
fn drive_roots() -> Vec<String> {
    // SAFETY: búfer nulo y tamaño 0: devuelve el tamaño necesario con el nulo final.
    let n = unsafe { GetLogicalDriveStringsW(0, null_mut()) };
    if n == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u16; n as usize];
    // SAFETY: `buf` tiene `n` u16; si basta, devuelve la longitud sin el último nulo.
    let m = unsafe { GetLogicalDriveStringsW(n, buf.as_mut_ptr()) };
    if m == 0 || m >= n {
        return Vec::new();
    }
    // Lista "C:\\\0D:\\\0\0": se parte por los nulos y se quitan los vacíos.
    buf[..m as usize]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}
