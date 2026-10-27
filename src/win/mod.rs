//! Capa fina sobre Win32: entorno, permisos, papelera, unidades y Explorador.

pub mod defender;
pub mod games;
pub mod icon;
pub mod proc;
pub mod recycle;
pub mod reg;
pub mod rm;
pub mod services;
pub mod startup;
pub mod trust;
pub mod uninstall;
pub mod wmi;

use std::ffi::{OsStr, OsString};
use std::iter::once;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};

use winapi::Interface;
use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::{BOOL, DWORD};
use winapi::shared::ntdef::{PULARGE_INTEGER, PWSTR};
use winapi::shared::winerror::{E_UNEXPECTED, S_OK};
use winapi::um::combaseapi::{CoInitializeEx, CoTaskMemFree, CoUninitialize};
use winapi::um::fileapi::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetLongPathNameW, GetVolumeInformationW,
};
use winapi::um::knownfolders::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData,
    FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_ProgramData,
    FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, FOLDERID_Public, FOLDERID_RoamingAppData,
    FOLDERID_System, FOLDERID_Videos, FOLDERID_Windows,
};
use winapi::um::objbase::COINIT_APARTMENTTHREADED;
use winapi::um::shellapi::{
    SHEmptyRecycleBinW, SHQUERYRBINFO, SHQueryRecycleBinW, ShellExecuteW, SHERB_NOCONFIRMATION,
    SHERB_NOPROGRESSUI, SHERB_NOSOUND,
};
use winapi::um::shlobj::SHGetKnownFolderPath;
use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
use winapi::um::minwinbase::SYSTEMTIME;
use winapi::um::sysinfoapi::GetLocalTime;
use winapi::um::winbase::DRIVE_FIXED;
use winapi::um::unknwnbase::IUnknown;
use winapi::um::winnt::{HANDLE, HRESULT};
use winapi::um::winuser::SW_SHOWNORMAL;

use wincleaner::categories::Env;
use wincleaner::checks::{self, Places};
use wincleaner::paths::{is_valid, strip_verbatim};
use wincleaner::space::zones::Zones;
use wincleaner::space::{Open, explorer_target};

// winapi 0.3.9 no declara IsUserAnAdmin, aunque shell32 la exporta.
#[link(name = "shell32")]
unsafe extern "system" {
    fn IsUserAnAdmin() -> BOOL;
}

/// Un HANDLE propio (o nulo); se cierra al soltarlo.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // INVALID_HANDLE_VALUE (-1) es lo que devuelven CreateFileW y CreateToolhelp32Snapshot al fallar.
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: lo abrió Windows para nosotros y solo se cierra aquí.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Interfaz COM propia (nunca nula); se suelta al salir de su ámbito.
pub(crate) struct Com<T: Interface>(pub(crate) *mut T);

impl<T: Interface> Com<T> {
    /// Llama a `f` con un puntero de salida y se queda con lo que deje si `f` tiene éxito.
    ///
    /// # Safety
    /// `f` deja en el puntero nulo o una interfaz `T` válida con una referencia que pasa a ser nuestra.
    pub(crate) unsafe fn out(f: impl FnOnce(*mut *mut T) -> HRESULT) -> Result<Self, HRESULT> {
        let mut p: *mut T = null_mut();
        let hr = f(&mut p);
        if p.is_null() {
            // E_POINTER: «tuvo éxito» pero no dio nada.
            return Err(if hr < 0 { hr } else { 0x8000_4003_u32 as i32 });
        }
        let c = Com(p);
        if hr < 0 { Err(hr) } else { Ok(c) }
    }
}

impl<T: Interface> std::ops::Deref for Com<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: nunca es nulo y vive mientras tengamos la referencia.
        unsafe { &*self.0 }
    }
}

impl<T: Interface> Drop for Com<T> {
    fn drop(&mut self) {
        // SAFETY: `self.0` es una interfaz válida con una referencia nuestra, que se suelta aquí.
        unsafe { (*self.0.cast::<IUnknown>()).Release() };
    }
}

pub struct Drive {
    pub root: String,
    /// Nombre del volumen; vacío si no se pudo leer.
    pub label: String,
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

/// Ejecuta `f` con COM iniciado en este hilo: las funciones de la papelera lo usan por dentro.
pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: reservado nulo; si tiene éxito (S_OK o S_FALSE) se equilibra con CoUninitialize.
    let hr = unsafe { CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED) };
    let out = f();
    if hr >= 0 {
        // SAFETY: este hilo inició COM justo arriba.
        unsafe { CoUninitialize() };
    }
    out
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
            (ok != 0).then(|| Drive { label: volume_label(&w), root, free, total })
        })
        .collect()
}

/// Nombre del volumen de la raíz `w` (UTF-16 terminada en nulo), o vacío si falla.
fn volume_label(w: &[u16]) -> String {
    let mut buf = [0u16; 261];
    // SAFETY: `w` termina en nulo; `buf` mide los 261 que se le dicen; lo demás es opcional.
    let ok = unsafe {
        GetVolumeInformationW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as DWORD, null_mut(), null_mut(), null_mut(), null_mut(), 0)
    };
    if ok == 0 {
        return String::new();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Abre `path` en el Explorador: una carpeta se abre y un archivo se selecciona en la suya, según
/// `space::explorer_target`. `explorer.exe` se lanza por ruta absoluta: con solo `explorer.exe`,
/// Windows buscaría antes en la carpeta del `.exe` portable.
pub fn open_in_explorer(path: &Path, is_dir: bool, odd: bool) {
    match explorer_target(path, is_dir, odd) {
        Some(Open::Folder(p)) => open_folder(&p),
        Some(Open::Select(p)) => {
            let Some(windows) = known_folder(&FOLDERID_Windows) else { return };
            let _ = Command::new(windows.join("explorer.exe"))
                .raw_arg(format!("/select,\"{}\"", p.display()))
                .spawn();
        }
        None => {}
    }
}

/// `C:\\Windows`, para saber qué procesos son de Windows.
pub fn windows_dir() -> Option<String> {
    known_folder(&FOLDERID_Windows).map(|p| p.to_string_lossy().into_owned())
}

/// `C:\\Windows\\System32`, para `programs::uninstall_command`.
pub fn system32() -> Option<String> {
    known_folder(&FOLDERID_System).map(|p| p.to_string_lossy().into_owned())
}

/// Abre la carpeta `dir` en el Explorador, solo si existe. Con ShellExecute y la ruta
/// absoluta de una carpeta, no hay línea de comandos que partir ni programa que buscar.
pub fn open_folder(dir: &Path) {
    if !dir.is_absolute() || !dir.is_dir() {
        return;
    }
    let (verb, file) = (wide("open"), wide(dir));
    // SAFETY: cadenas UTF-16 terminadas en nulo que viven durante la llamada.
    unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}

/// Carpetas en las que un ejecutable da que pensar, para las comprobaciones de seguridad: las
/// temporales son TEMP, TMP y `%SystemRoot%\Temp`.
/// Las que no se pueden leer quedan vacías y no cuentan.
pub fn security_places() -> Places {
    let folder = |id: &GUID| known_folder(id).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mut temp: Vec<String> = ["TEMP", "TMP"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|t| long_path(Path::new(&t)).to_string_lossy().into_owned())
        .chain(known_folder(&FOLDERID_Windows).map(|w| w.join("Temp").to_string_lossy().into_owned()))
        .collect();
    temp.sort_by_key(|t| t.to_lowercase());
    temp.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Places {
        temp,
        downloads: folder(&FOLDERID_Downloads),
        app_data: folder(&FOLDERID_RoamingAppData),
        local_app_data: folder(&FOLDERID_LocalAppData),
        public: folder(&FOLDERID_Public),
        system_root: folder(&FOLDERID_Windows),
    }
}

/// Abre `url` con el programa predeterminado. Solo `https://`, `windowsdefender:` y las tres de
/// los lanzadores (`steam://uninstall/<id>`, `com.epicgames.launcher://` y
/// `ms-settings:appsfeatures`), según `checks::openable_url`: nada de `file:`, `http:`,
/// `javascript:` ni rutas.
pub fn open_url(url: &str) {
    if !checks::openable_url(url) {
        return;
    }
    let (verb, file) = (wide("open"), wide(url));
    // SAFETY: cadenas UTF-16 terminadas en nulo que viven durante la llamada.
    unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}

/// Abre `path` en el Bloc de notas, lanzado por ruta absoluta (`System32\notepad.exe`).
pub fn open_with_notepad(path: &Path) {
    let Some(system) = known_folder(&FOLDERID_System) else { return };
    let _ = Command::new(system.join("notepad.exe"))
        .raw_arg(format!("\"{}\"", path.display()))
        .spawn();
}

/// Hora local `(hora, minuto)`, para «Último análisis: hoy, 18:42».
pub fn local_hm() -> (u16, u16) {
    // SAFETY: SYSTEMTIME es de enteros, así que todo a cero es válido; GetLocalTime lo rellena.
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: `t` es nuestro y del tamaño que espera.
    unsafe { GetLocalTime(&mut t) };
    (t.wHour, t.wMinute)
}

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(once(0)).collect()
}

/// Lee y libera una cadena que ha dejado Windows con CoTaskMemAlloc; `None` si es nula.
///
/// # Safety
/// `p` es nulo o una cadena UTF-16 terminada en nulo de CoTaskMemAlloc que pasa a ser nuestra.
pub(crate) unsafe fn take_co_str(p: PWSTR) -> Option<PathBuf> {
    let out = (!p.is_null()).then(|| {
        // SAFETY: `p` es una cadena UTF-16 terminada en nulo.
        let s = unsafe {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            std::slice::from_raw_parts(p, len)
        };
        PathBuf::from(OsString::from_wide(s))
    });
    // SAFETY: es nuestra y solo se libera aquí; CoTaskMemFree acepta nulo.
    unsafe { CoTaskMemFree(p.cast()) };
    out
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut p: PWSTR = null_mut();
    // SAFETY: `p` recibe un búfer de CoTaskMemAlloc (o nulo) que hay que liberar también si falla.
    let hr = unsafe { SHGetKnownFolderPath(id, 0, null_mut(), &mut p) };
    // SAFETY: lo que deja SHGetKnownFolderPath es nuestro; con S_OK es una cadena terminada en nulo.
    let out = unsafe { take_co_str(p) };
    out.filter(|_| hr == S_OK)
}

/// Las seis carpetas del usuario con su nombre en español; las que no se pueden leer no salen.
pub fn known_dirs() -> Vec<(PathBuf, &'static str)> {
    [
        (&FOLDERID_Documents, "Documentos"),
        (&FOLDERID_Desktop, "Escritorio"),
        (&FOLDERID_Downloads, "Descargas"),
        (&FOLDERID_Pictures, "Imágenes"),
        (&FOLDERID_Videos, "Vídeos"),
        (&FOLDERID_Music, "Música"),
    ]
    .into_iter()
    .filter_map(|(id, label)| known_folder(id).map(|p| (p, label)))
    .collect()
}

/// Zonas protegidas de Espacio. `None` (no se puede borrar nada) si falta el perfil, Windows
/// o la ruta del `.exe`: fallo cerrado.
pub fn space_zones(games: &[String]) -> Option<Zones> {
    let text = |p: PathBuf| p.to_string_lossy().into_owned();
    let profile = text(known_folder(&FOLDERID_Profile)?);
    let windows = text(known_folder(&FOLDERID_Windows)?);
    // Nombre largo y sin `\\?\`, para que se pueda comparar con las rutas del árbol.
    let exe = text(long_path(&std::env::current_exe().ok()?));
    let exe = strip_verbatim(&exe).unwrap_or(&exe).to_string();
    if !is_valid(&exe) {
        return None;
    }
    let mut system = vec![windows];
    system.extend(
        [&FOLDERID_ProgramFiles, &FOLDERID_ProgramFilesX86, &FOLDERID_ProgramData]
            .into_iter()
            .filter_map(known_folder)
            .map(text),
    );
    let mut known: Vec<(String, &'static str)> = known_dirs().into_iter().map(|(p, l)| (text(p), l)).collect();
    // `AppData` es la carpeta padre de Roaming.
    if let Some(app_data) = known_folder(&FOLDERID_RoamingAppData).and_then(|r| r.parent().map(Path::to_path_buf)) {
        known.push((text(app_data), "AppData"));
    }
    Some(Zones::new(&drive_roots(), &system, &profile, &known, &exe, games))
}

pub(crate) fn long_path(p: &Path) -> PathBuf {
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
    // Uno de más: según la versión, `n` puede no contar el último nulo.
    let mut buf = vec![0u16; n as usize + 1];
    // SAFETY: `buf` tiene `n + 1` u16; si basta, devuelve la longitud sin el último nulo.
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
