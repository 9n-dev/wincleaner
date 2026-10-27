//! Escaneo y borrado seguros: nunca se siguen enlaces y todo error de E/S cuenta como «saltado».

use crate::categories::{is_link, Category, Filter, Status};
use crate::paths::{is_inside, root_is_safe};
use crate::space::recycle::{os_reason, Reason};
use std::fs::Metadata;
#[cfg(not(windows))]
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub files: Vec<(PathBuf, u64)>,
    /// Candidatas a borrar si quedan vacías, de la más profunda a la menos.
    pub dirs: Vec<PathBuf>,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeleteReport {
    pub freed: u64,
    pub deleted: u64,
    pub skipped: u64,
    pub skipped_bytes: u64,
}

/// Por qué no se borró: el código del sistema si lo hay, `None` si fue por una regla.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Skip(pub Option<i32>);

/// Resultado de `purge`: lo liberado y cada archivo (o enlace, o carpeta) que no se pudo borrar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Purged {
    pub freed: u64,
    pub failed: Vec<(PathBuf, u64, Reason)>,
}

fn old_enough(t: SystemTime, filter: Filter, now: SystemTime) -> bool {
    match filter {
        Filter::OlderThan(d) => t.checked_add(d).is_some_and(|x| x <= now),
        _ => true,
    }
}

/// Antigüedad de un archivo: la fecha más reciente entre creación y modificación.
fn age_ok(md: &Metadata, filter: Filter, now: SystemTime) -> bool {
    let Ok(modified) = md.modified() else { return false };
    let newest = md.created().map_or(modified, |c| c.max(modified));
    old_enough(newest, filter, now)
}

fn in_any_root(p: &Path, roots: &[PathBuf]) -> bool {
    p.to_str().is_some_and(|s| roots.iter().any(|r| r.to_str().is_some_and(|r| is_inside(s, r))))
}

fn is_thumbcache_name(name: &std::ffi::OsStr) -> bool {
    let n = name.to_string_lossy().to_lowercase();
    n.starts_with("thumbcache_") && n.ends_with(".db")
}

pub fn scan(cat: &Category, now: SystemTime) -> ScanResult {
    let mut out = ScanResult::default();
    if cat.status != Status::Ready {
        return out;
    }
    for root in &cat.roots {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let path = e.path();
                let Ok(md) = std::fs::symlink_metadata(&path) else { continue };
                if is_link(&md) {
                    continue;
                }
                if md.is_file() {
                    let name_ok = cat.filter != Filter::Thumbcache || is_thumbcache_name(&e.file_name());
                    if name_ok && age_ok(&md, cat.filter, now) && in_any_root(&path, std::slice::from_ref(root)) {
                        out.bytes += md.len();
                        out.files.push((path, md.len()));
                    }
                } else if md.is_dir() && cat.filter != Filter::Thumbcache {
                    if md.modified().is_ok_and(|m| old_enough(m, cat.filter, now)) {
                        out.dirs.push(path.clone());
                    }
                    stack.push(path);
                }
            }
        }
    }
    out.dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    out
}

/// La raíz, con los enlaces de las carpetas de encima ya resueltos, sigue siendo segura: un
/// junction en `AppData` podría llevar `…\AppData\Local\Temp` dentro de Documentos.
fn resolved_root_safe(root: &Path, cat: &Category) -> bool {
    #[cfg(windows)]
    let resolved = handle::final_path_of(root);
    #[cfg(not(windows))]
    let resolved = std::fs::canonicalize(root).ok().and_then(|p| p.to_str().map(str::to_owned));
    resolved.is_some_and(|r| root_is_safe(&r, &cat.protected, &cat.forbidden_trees))
}

pub fn delete(cat: &Category, scan: &ScanResult, now: SystemTime) -> DeleteReport {
    let roots: Vec<PathBuf> = cat.roots.iter().filter(|r| resolved_root_safe(r, cat)).cloned().collect();
    #[cfg(windows)]
    let finals = handle::final_roots(&roots);
    let mut rep = DeleteReport::default();
    for (path, size) in &scan.files {
        #[cfg(windows)]
        let freed = handle::try_delete(&roots, cat.filter, path, now, &finals);
        #[cfg(not(windows))]
        let freed = try_delete(&roots, cat.filter, path, now);
        match freed {
            Ok(Some(n)) => {
                rep.freed += n;
                rep.deleted += 1;
            }
            Ok(None) => {}
            Err(Skip(_)) => {
                rep.skipped += 1;
                rep.skipped_bytes += size;
            }
        }
    }
    for dir in &scan.dirs {
        #[cfg(windows)]
        let _ = handle::remove_dir(dir, &roots, &finals);
        #[cfg(not(windows))]
        let _ = remove_dir(dir, &roots);
    }
    rep
}

/// Borra `target` (archivo o carpeta) con todo lo que tenga dentro, por el mismo camino que
/// `delete` y sin filtro de antigüedad. Lo de dentro se borra con `target` como raíz (si una
/// carpeta se cambia por un enlace a otra de al lado, se sale de la raíz y se salta); solo el
/// propio `target` usa como raíz su carpeta padre. Los enlaces no se siguen ni se borran:
/// salen en `failed` como `Link`, y la carpeta que los contiene se queda. Si `target` ya no existe,
/// no hay nada que hacer.
pub fn purge(target: &Path) -> Purged {
    let (failed, files, dirs) = purge_walk(target);
    purge_apply(target, failed, &files, &dirs)
}

type Failed = Vec<(PathBuf, u64, Reason)>;
type Walk = (Failed, Vec<(PathBuf, u64)>, Vec<PathBuf>);

/// Lo que hay que borrar, sin seguir enlaces: archivos y carpetas (de la más profunda a la
/// menos, con `target` al final). Los enlaces y lo ilegible van ya a `failed`.
fn purge_walk(target: &Path) -> Walk {
    let mut failed = Vec::new();
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    match std::fs::symlink_metadata(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (failed, files, dirs),
        Err(e) => failed.push((target.to_path_buf(), 0, os_reason(e.raw_os_error()))),
        Ok(md) if is_link(&md) => failed.push((target.to_path_buf(), 0, Reason::Link)),
        Ok(md) if md.is_dir() => dirs.push(target.to_path_buf()),
        Ok(md) => files.push((target.to_path_buf(), md.len())),
    }
    let mut stack = dirs.clone();
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                failed.push((dir, 0, os_reason(e.raw_os_error())));
                continue;
            }
        };
        for e in rd {
            let path = match e {
                Ok(e) => e.path(),
                Err(e) => {
                    failed.push((dir.clone(), 0, os_reason(e.raw_os_error())));
                    continue;
                }
            };
            match std::fs::symlink_metadata(&path) {
                Ok(md) if is_link(&md) => failed.push((path, 0, Reason::Link)),
                Ok(md) if md.is_dir() => {
                    dirs.push(path.clone());
                    stack.push(path);
                }
                Ok(md) if md.is_file() => files.push((path, md.len())),
                // Desaparecido entre listar y mirar: ya está hecho.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => failed.push((path, 0, os_reason(e.raw_os_error()))),
                // Ni archivo, ni carpeta, ni enlace (un dispositivo, un socket…).
                Ok(_) => failed.push((path, 0, os_reason(None))),
            }
        }
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    (failed, files, dirs)
}

fn purge_apply(target: &Path, mut failed: Failed, files: &[(PathBuf, u64)], dirs: &[PathBuf]) -> Purged {
    let mut rep = Purged::default();
    let Some(parent) = target.parent() else {
        rep.failed = failed;
        return rep;
    };
    let inner = [target.to_path_buf()];
    let outer = [parent.to_path_buf()];
    // `final_roots` descarta un `target` que ya sea un enlace: entonces no se borra nada dentro.
    #[cfg(windows)]
    let (inner_finals, outer_finals) = (handle::final_roots(&inner), handle::final_roots(&outer));
    let now = SystemTime::now();
    for (path, size) in files {
        let roots: &[PathBuf] = if path == target { &outer } else { &inner };
        #[cfg(windows)]
        let freed = handle::try_delete(roots, Filter::All, path, now, if path == target { &outer_finals } else { &inner_finals });
        #[cfg(not(windows))]
        let freed = try_delete(roots, Filter::All, path, now);
        match freed {
            Ok(Some(n)) => rep.freed += n,
            Ok(None) => {}
            Err(Skip(code)) => failed.push((path.clone(), *size, skip_reason(path, code))),
        }
    }
    // La primera carpeta que no se ha podido quitar (la más honda): las de arriba fallan por no estar vacías.
    let mut stuck = None;
    for dir in dirs {
        let roots: &[PathBuf] = if dir == target { &outer } else { &inner };
        #[cfg(windows)]
        let r = handle::remove_dir(dir, roots, if dir == target { &outer_finals } else { &inner_finals });
        #[cfg(not(windows))]
        let r = remove_dir(dir, roots);
        if let (Err(code), None) = (r, &stuck) {
            stuck = Some((dir.clone(), 0, skip_reason(dir, code)));
        }
    }
    // Sigue en pie sin un archivo que lo explique: se dice qué carpeta y por qué (p. ej. en uso).
    if failed.is_empty() && std::fs::symlink_metadata(target).is_ok() {
        failed.push(stuck.unwrap_or((target.to_path_buf(), 0, Reason::Other(0))));
    }
    rep.failed = failed;
    rep
}

/// Motivo de un salto: sin código lo para una regla; si ahora es un enlace (cambiado tras mirar), se dice.
fn skip_reason(path: &Path, code: Option<i32>) -> Reason {
    let link = code.is_none() && std::fs::symlink_metadata(path).is_ok_and(|md| is_link(&md));
    if link { Reason::Link } else { os_reason(code) }
}

/// Ruta final (`C:\…`) de `path`, abierto sin seguir un enlace en el último componente.
#[cfg(windows)]
pub fn final_path_of(path: &Path) -> Option<String> {
    handle::final_path_of(path)
}

/// Borra `dir` si está vacía. `Err` lleva el código del sistema, o `None` si la para una regla.
#[cfg(not(windows))]
fn remove_dir(dir: &Path, roots: &[PathBuf]) -> Result<(), Option<i32>> {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.raw_os_error()),
        Ok(md) if !md.is_dir() || is_link(&md) || !safe_to_touch(dir, roots) => return Err(None),
        Ok(_) => {}
    }
    match std::fs::remove_dir(dir) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.raw_os_error()),
        _ => Ok(()),
    }
}

/// Todos los ancestros de `path` desde `root` (inclusive) son carpetas reales, sin enlaces.
#[cfg(not(windows))]
fn no_link_ancestors(path: &Path, root: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .take_while(|a| a.starts_with(root))
        .all(|a| std::fs::symlink_metadata(a).is_ok_and(|md| md.is_dir() && !is_link(&md)))
}

/// Dentro de una raíz (léxico) y sin enlaces en las carpetas intermedias.
#[cfg(not(windows))]
fn safe_to_touch(path: &Path, roots: &[PathBuf]) -> bool {
    in_any_root(path, roots) && roots.iter().any(|r| path.starts_with(r) && no_link_ancestors(path, r))
}

/// `Ok(Some(bytes))` borrado, `Ok(None)` ya no existe, `Err` saltado.
#[cfg(not(windows))]
fn try_delete(roots: &[PathBuf], filter: Filter, path: &Path, now: SystemTime) -> Result<Option<u64>, Skip> {
    if !in_any_root(path, roots) {
        return Err(Skip(None));
    }
    let md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Skip(e.raw_os_error())),
    };
    if is_link(&md) || !md.is_file() || !age_ok(&md, filter, now) || !safe_to_touch(path, roots) {
        return Err(Skip(None));
    }
    // ponytail: este camino por ruta solo existe para las pruebas en Linux y deja una ventana
    // TOCTOU entre comprobar y borrar; en Windows se borra por handle (mod `handle`).
    if md.permissions().readonly() {
        let mut perm = md.permissions();
        #[expect(clippy::permissions_set_readonly_false, reason = "solo en las pruebas, sobre un archivo que se borra justo después")]
        perm.set_readonly(false);
        let _ = std::fs::set_permissions(path, perm);
    }
    std::fs::remove_file(path).map(|_| Some(md.len())).map_err(|e| Skip(e.raw_os_error()))
}

/// Borrado por handle: se abre el objeto sin seguir enlaces en el último componente, se
/// comprueba por ese handle (tipo, antigüedad y ruta final dentro de la raíz) y se borra
/// por ese mismo handle. No hay ninguna resolución de ruta entre la comprobación y el
/// borrado, así que cambiar una carpeta intermedia por un junction no sirve de nada.
/// Cualquier fallo es «saltado»; nunca se recurre al borrado por ruta.
#[cfg(windows)]
mod handle {
    use super::{age_ok, in_any_root, Skip};
    use crate::categories::{is_link, Filter};
    use crate::paths::{is_inside, strip_verbatim};
    use std::fs::{File, OpenOptions};
    use std::io::ErrorKind;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;
    use winapi::shared::minwindef::DWORD;
    use winapi::shared::winerror::{ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED};
    use winapi::um::fileapi::{FILE_DISPOSITION_INFO, GetFinalPathNameByHandleW, SetFileInformationByHandle};
    use winapi::um::minwinbase::{FileDispositionInfo, FileDispositionInfoEx};
    use winapi::um::winbase::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, VOLUME_NAME_DOS};
    use winapi::um::winnt::{DELETE, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, HANDLE};

    // winapi 0.3.9 no declara FILE_NAME_NORMALIZED ni FILE_DISPOSITION_INFO_EX con sus flags.
    // FILE_DISPOSITION_INFO_EX es un único ULONG `Flags`, así que se pasa un u32.
    const FILE_NAME_NORMALIZED: DWORD = 0x0;
    const FILE_DISPOSITION_FLAG_DELETE: u32 = 0x1;
    const FILE_DISPOSITION_FLAG_POSIX_SEMANTICS: u32 = 0x2;
    const FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE: u32 = 0x10;
    // Valores de FILE_INFO_BY_HANDLE_CLASS según la documentación de Microsoft.
    const _: () = assert!(FileDispositionInfo == 4 && FileDispositionInfoEx == 21);

    /// Abre `path` sin seguir un enlace en el último componente (los intermedios sí se
    /// siguen: por eso luego se mira la ruta final). También abre carpetas.
    fn open(path: &Path, access: DWORD) -> std::io::Result<File> {
        OpenOptions::new()
            .access_mode(access)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    }

    /// Ruta real del objeto abierto (`C:\…`); `None` si es de red o no se puede leer.
    fn final_path(f: &File) -> Option<String> {
        let h = f.as_raw_handle() as HANDLE;
        let mut buf = vec![0u16; 260];
        for _ in 0..2 {
            // SAFETY: `h` vive mientras `f`; `buf` tiene `buf.len()` u16. Si no cabe,
            // devuelve el tamaño necesario con el nulo; si cabe, la longitud sin él.
            let n = unsafe {
                GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as DWORD, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS)
            } as usize;
            if n == 0 {
                return None;
            }
            if n < buf.len() {
                let s = String::from_utf16(&buf[..n]).ok()?;
                return strip_verbatim(&s).map(str::to_owned);
            }
            buf.resize(n, 0);
        }
        None
    }

    pub fn final_path_of(path: &Path) -> Option<String> {
        open(path, FILE_READ_ATTRIBUTES).ok().and_then(|f| final_path(&f))
    }

    fn inside(f: &File, finals: &[String]) -> bool {
        final_path(f).is_some_and(|p| finals.iter().any(|r| is_inside(&p, r)))
    }

    /// Ruta final de cada raíz que sea una carpeta real (no un enlace). Se abre solo con
    /// FILE_READ_ATTRIBUTES, que no choca con los modos de compartir de otros procesos.
    pub fn final_roots(roots: &[PathBuf]) -> Vec<String> {
        let real_dir = |f: &File| f.metadata().is_ok_and(|md| md.is_dir() && !is_link(&md));
        roots
            .iter()
            .filter_map(|r| open(r, FILE_READ_ATTRIBUTES).ok().filter(real_dir))
            .filter_map(|f| final_path(&f))
            .collect()
    }

    /// Marca el objeto del handle para borrarse al cerrarlo (al soltar `f`).
    fn mark_deleted(f: &File) -> bool {
        let h = f.as_raw_handle() as HANDLE;
        let mut flags = FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE;
        // SAFETY: `h` vive mientras `f`; el búfer es el u32 de FILE_DISPOSITION_INFO_EX.
        let ok = unsafe { SetFileInformationByHandle(h, FileDispositionInfoEx, (&raw mut flags).cast(), size_of_val(&flags) as DWORD) };
        if ok != 0 {
            return true;
        }
        // Sin FileDispositionInfoEx (Windows 10 anterior a 1709, FAT…) se usa la clásica.
        let e = std::io::Error::last_os_error().raw_os_error();
        if !matches!(e, Some(c) if c as u32 == ERROR_INVALID_PARAMETER || c as u32 == ERROR_NOT_SUPPORTED) {
            return false;
        }
        let mut info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
        let size = size_of::<FILE_DISPOSITION_INFO>() as DWORD;
        // SAFETY: `h` vive mientras `f`; `info` es un FILE_DISPOSITION_INFO de `size` bytes.
        unsafe { SetFileInformationByHandle(h, FileDispositionInfo, (&raw mut info).cast(), size) != 0 }
    }

    /// `Ok(Some(bytes))` borrado, `Ok(None)` ya no existe, `Err` saltado.
    pub fn try_delete(
        roots: &[PathBuf],
        filter: Filter,
        path: &Path,
        now: SystemTime,
        finals: &[String],
    ) -> Result<Option<u64>, Skip> {
        if !in_any_root(path, roots) {
            return Err(Skip(None));
        }
        let f = match open(path, DELETE | FILE_READ_ATTRIBUTES) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Skip(e.raw_os_error())),
        };
        let md = f.metadata().map_err(|e| Skip(e.raw_os_error()))?;
        if is_link(&md) || !md.is_file() || !age_ok(&md, filter, now) || !inside(&f, finals) {
            return Err(Skip(None));
        }
        if mark_deleted(&f) { Ok(Some(md.len())) } else { Err(Skip(std::io::Error::last_os_error().raw_os_error())) }
    }

    /// Borra `dir` si está vacía. `Err` lleva el código del sistema (32 si es la carpeta actual
    /// de una consola), o `None` si la para una regla.
    pub fn remove_dir(dir: &Path, roots: &[PathBuf], finals: &[String]) -> Result<(), Option<i32>> {
        if !in_any_root(dir, roots) {
            return Err(None);
        }
        let f = match open(dir, DELETE | FILE_READ_ATTRIBUTES) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.raw_os_error()),
        };
        let md = f.metadata().map_err(|e| e.raw_os_error())?;
        if !md.is_dir() || is_link(&md) || !inside(&f, finals) {
            return Err(None);
        }
        if mark_deleted(&f) { Ok(()) } else { Err(std::io::Error::last_os_error().raw_os_error()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::categories::{Category, CategoryId, Filter, Status, DAY};
    use crate::testutil::temp_dir;
    use std::fs::{self, File};
    use std::path::Path;

    fn cat(root: &Path, filter: Filter, status: Status) -> Category {
        let (protected, forbidden_trees) = (Vec::new(), Vec::new());
        Category { id: CategoryId::UserTemp, name: "t", filter, roots: vec![root.to_path_buf()], status, protected, forbidden_trees }
    }
    fn ready(root: &Path, filter: Filter) -> Category {
        cat(root, filter, Status::Ready)
    }
    fn write(p: &Path, n: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0u8; n]).unwrap();
    }
    fn future() -> SystemTime {
        SystemTime::now() + 2 * DAY
    }
    fn names(r: &ScanResult) -> Vec<String> {
        let mut v: Vec<String> =
            r.files.iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn scan_counts_files_and_bytes() {
        let root = temp_dir("scan_counts");
        write(&root.join("a.bin"), 10);
        write(&root.join("sub/b.bin"), 20);
        let r = scan(&ready(&root, Filter::All), SystemTime::now());
        assert_eq!(r.files.len(), 2);
        assert_eq!(r.bytes, 30);
    }

    #[test]
    fn scan_skips_non_ready() {
        let root = temp_dir("scan_nonready");
        write(&root.join("a.bin"), 10);
        let r = scan(&cat(&root, Filter::All, Status::NeedsAdmin), SystemTime::now());
        assert!(r.files.is_empty() && r.dirs.is_empty() && r.bytes == 0);
    }

    #[test]
    fn older_than_filter_uses_newest_date() {
        let root = temp_dir("older_than");
        write(&root.join("viejo.tmp"), 1);
        write(&root.join("nuevo.tmp"), 1);
        let real = SystemTime::now();
        File::options().write(true).open(root.join("nuevo.tmp")).unwrap().set_modified(real + 5 * DAY).unwrap();
        let r = scan(&ready(&root, Filter::OlderThan(DAY)), real + 2 * DAY);
        assert_eq!(names(&r), vec!["viejo.tmp"]);
    }

    #[test]
    fn thumbcache_filter() {
        let root = temp_dir("thumbcache");
        for f in ["thumbcache_32.db", "Thumbcache_IDX.DB", "iconcache_16.db", "sub/thumbcache_x.db"] {
            write(&root.join(f), 1);
        }
        let r = scan(&ready(&root, Filter::Thumbcache), SystemTime::now());
        assert_eq!(names(&r), vec!["Thumbcache_IDX.DB", "thumbcache_32.db"]);
        assert!(r.dirs.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed() {
        use std::os::unix::fs::symlink;
        let base = temp_dir("symlinks");
        let (root, fuera) = (base.join("root"), base.join("fuera"));
        write(&fuera.join("importante.txt"), 5);
        fs::create_dir_all(&root).unwrap();
        symlink(&fuera, root.join("enlace")).unwrap();
        symlink(fuera.join("importante.txt"), root.join("enlace_archivo")).unwrap();
        let c = ready(&root, Filter::All);
        let r = scan(&c, future());
        assert!(r.files.is_empty() && r.dirs.is_empty());
        let hand = ScanResult {
            files: vec![(root.join("enlace_archivo"), 5), (root.join("enlace/importante.txt"), 5)],
            dirs: vec![root.join("enlace")],
            bytes: 10,
        };
        let rep = delete(&c, &hand, future());
        assert_eq!(rep.deleted, 0);
        assert!(fuera.join("importante.txt").exists());
        assert!(fs::symlink_metadata(root.join("enlace")).is_ok());
        assert!(fs::symlink_metadata(root.join("enlace_archivo")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn delete_rechecks_root_resolved_through_link_above_it() {
        let u = fs::canonicalize(temp_dir("link_above")).unwrap().join("U");
        let docs = u.join("Documents");
        let s = |p: &Path| p.to_str().unwrap().to_string();
        let guarded = |root: &Path| Category {
            protected: vec![s(&u)],
            forbidden_trees: vec![s(&docs)],
            ..ready(root, Filter::All)
        };
        // Sin enlace: se borra con normalidad.
        let real = u.join("Real/Local/Temp");
        write(&real.join("a.tmp"), 5);
        let c = guarded(&real);
        assert_eq!(delete(&c, &scan(&c, future()), future()).deleted, 1);
        // `AppData` cambiado por un enlace a Documentos tras validar la raíz.
        write(&docs.join("x/Local/Temp/importante.txt"), 5);
        std::os::unix::fs::symlink(docs.join("x"), u.join("AppData")).unwrap();
        let c = guarded(&u.join("AppData/Local/Temp"));
        let rep = delete(&c, &scan(&c, future()), future());
        assert_eq!((rep.deleted, rep.skipped), (0, 1));
        assert!(docs.join("x/Local/Temp/importante.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn delete_does_not_follow_linked_intermediate_dir() {
        let base = temp_dir("linked_mid");
        let (root, fuera) = (base.join("root"), base.join("fuera"));
        write(&fuera.join("importante.txt"), 5);
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&fuera, root.join("enlace")).unwrap();
        let s = ScanResult { files: vec![(root.join("enlace/importante.txt"), 5)], dirs: vec![], bytes: 5 };
        let rep = delete(&ready(&root, Filter::All), &s, future());
        assert!(fuera.join("importante.txt").exists());
        assert_eq!((rep.skipped, rep.skipped_bytes, rep.deleted), (1, 5, 0));
    }

    #[cfg(unix)]
    #[test]
    fn delete_detects_dir_swapped_for_link_after_scan() {
        let base = temp_dir("swapped");
        let (root, fuera) = (base.join("root"), base.join("fuera"));
        write(&root.join("sub/victima.txt"), 7);
        write(&fuera.join("victima.txt"), 7);
        let c = ready(&root, Filter::All);
        let s = scan(&c, future());
        fs::remove_dir_all(root.join("sub")).unwrap();
        std::os::unix::fs::symlink(&fuera, root.join("sub")).unwrap();
        let rep = delete(&c, &s, future());
        assert!(fuera.join("victima.txt").exists());
        assert_eq!((rep.skipped, rep.skipped_bytes, rep.deleted), (1, 7, 0));
    }

    #[test]
    fn delete_rechecks_age() {
        let root = temp_dir("recheck_age");
        let f = root.join("a.tmp");
        write(&f, 4);
        let real = SystemTime::now();
        let c = ready(&root, Filter::OlderThan(DAY));
        let now = real + 2 * DAY;
        let s = scan(&c, now);
        assert_eq!(s.files.len(), 1);
        File::options().write(true).open(&f).unwrap().set_modified(real + 5 * DAY).unwrap();
        let rep = delete(&c, &s, now);
        assert!(f.exists());
        assert_eq!((rep.skipped, rep.skipped_bytes, rep.deleted), (1, 4, 0));
    }

    #[test]
    fn delete_frees_and_keeps_root() {
        let root = temp_dir("delete_frees");
        write(&root.join("sub/a.bin"), 10);
        write(&root.join("sub/b.bin"), 20);
        let c = ready(&root, Filter::All);
        let rep = delete(&c, &scan(&c, future()), future());
        assert_eq!((rep.freed, rep.deleted), (30, 2));
        assert!(!root.join("sub").exists());
        assert!(root.exists());
    }

    #[test]
    fn delete_refuses_paths_outside_roots() {
        let base = temp_dir("outside");
        let (root, otra) = (base.join("root"), base.join("otra"));
        fs::create_dir_all(&root).unwrap();
        write(&otra.join("x.bin"), 10);
        let s = ScanResult { files: vec![(otra.join("x.bin"), 10)], dirs: vec![], bytes: 10 };
        let rep = delete(&ready(&root, Filter::All), &s, future());
        assert!(otra.join("x.bin").exists());
        assert_eq!((rep.skipped, rep.skipped_bytes, rep.deleted), (1, 10, 0));
    }

    #[test]
    fn delete_removes_readonly_files() {
        let root = temp_dir("readonly");
        let f = root.join("ro.bin");
        write(&f, 3);
        let mut perm = fs::metadata(&f).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&f, perm).unwrap();
        let c = ready(&root, Filter::All);
        let rep = delete(&c, &scan(&c, future()), future());
        assert!(!f.exists());
        assert_eq!(rep.deleted, 1);
    }

    // Requiere ejecutar como usuario no root: root ignora los permisos de carpeta.
    #[cfg(unix)]
    #[test]
    fn delete_counts_permission_denied_as_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("denied");
        write(&root.join("sub/a.bin"), 4);
        let c = ready(&root, Filter::All);
        let s = scan(&c, future());
        fs::set_permissions(root.join("sub"), fs::Permissions::from_mode(0o555)).unwrap();
        let rep = delete(&c, &s, future());
        fs::set_permissions(root.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(rep.skipped, 1);
    }

    #[test]
    fn unicode_names_are_deleted() {
        let root = temp_dir("unicode");
        let f = root.join("año ñandú (copia).tmp");
        write(&f, 2);
        let c = ready(&root, Filter::All);
        let s = scan(&c, future());
        assert_eq!(s.files.len(), 1);
        assert_eq!(delete(&c, &s, future()).deleted, 1);
        assert!(!f.exists());
    }

    #[test]
    fn vanished_files_are_ignored() {
        let root = temp_dir("vanished");
        write(&root.join("a.bin"), 4);
        let c = ready(&root, Filter::All);
        let s = scan(&c, future());
        fs::remove_file(root.join("a.bin")).unwrap();
        let rep = delete(&c, &s, future());
        assert_eq!((rep.skipped, rep.deleted), (0, 0));
    }

    #[test]
    fn purge_removes_tree_and_target() {
        let base = temp_dir("purge_tree");
        let t = base.join("carpeta");
        write(&t.join("a.bin"), 100);
        write(&t.join("sub/deep/b.bin"), 50);
        let rep = purge(&t);
        assert_eq!((rep.freed, rep.failed.len()), (150, 0));
        assert!(!t.exists());
        assert!(base.exists());
    }

    #[test]
    fn purge_file_target() {
        let base = temp_dir("purge_file");
        let f = base.join("video.mov");
        write(&f, 42);
        let rep = purge(&f);
        assert_eq!(rep.freed, 42);
        assert!(!f.exists());
        assert!(base.exists());
    }

    #[cfg(unix)]
    #[test]
    fn purge_does_not_follow_links() {
        let base = temp_dir("purge_links");
        let (t, fuera) = (base.join("carpeta"), base.join("fuera"));
        write(&fuera.join("x.bin"), 5);
        write(&t.join("a.bin"), 3);
        std::os::unix::fs::symlink(&fuera, t.join("enlace")).unwrap();
        let rep = purge(&t);
        assert!(fuera.join("x.bin").exists());
        assert!(rep.failed.iter().any(|(f, _, r)| f == &t.join("enlace") && *r == Reason::Link));
        assert!(t.exists());
        assert!(!t.join("a.bin").exists());
    }

    #[test]
    fn purge_missing_target_is_ok() {
        let base = temp_dir("purge_missing");
        assert_eq!(purge(&base.join("no_existe")), Purged::default());
    }

    #[test]
    fn purge_unicode_and_deep() {
        let base = temp_dir("purge_unicode");
        let t = base.join("ñandú 🎉 raíz");
        let mut deep = t.clone();
        for i in 0..30 {
            deep = deep.join(format!("ñandú 🎉 {i}"));
        }
        write(&deep.join("fondo 🎉.bin"), 7);
        let rep = purge(&t);
        assert_eq!((rep.freed, rep.failed.len()), (7, 0));
        assert!(!t.exists());
    }

    #[test]
    fn purge_readonly_files() {
        let base = temp_dir("purge_readonly");
        let t = base.join("carpeta");
        let f = t.join("ro.bin");
        write(&f, 3);
        let mut perm = fs::metadata(&f).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&f, perm).unwrap();
        let rep = purge(&t);
        assert_eq!(rep.freed, 3);
        assert!(!t.exists());
    }

    #[cfg(unix)]
    #[test]
    fn purge_swapped_subdir_stays_inside() {
        let base = temp_dir("purge_swapped");
        let (t, otra) = (base.join("carpeta"), base.join("otra"));
        write(&t.join("sub/g"), 5);
        write(&otra.join("g"), 5);
        let (failed, files, dirs) = purge_walk(&t);
        fs::remove_dir_all(t.join("sub")).unwrap();
        std::os::unix::fs::symlink("../otra", t.join("sub")).unwrap();
        let rep = purge_apply(&t, failed, &files, &dirs);
        assert!(otra.join("g").exists());
        assert_eq!((rep.freed, rep.failed.len(), rep.failed[0].1), (0, 1, 5));
        assert!(t.exists());
        // En Linux lo de arriba ya lo para `no_link_ancestors`. En Windows el handle sigue el
        // junction y la ruta final cae en `otra/g`, dentro del padre: con esa ruta, lo de
        // dentro tiene que quedarse fuera de la raíz `target`.
        let rep = purge_apply(&t, Vec::new(), &[(otra.join("g"), 5)], &[]);
        assert!(otra.join("g").exists());
        assert_eq!((rep.freed, rep.failed.len()), (0, 1));
    }

    #[cfg(unix)]
    #[test]
    fn purge_reports_link_and_denied() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("pg-reasons");
        let out = temp_dir("pg-reasons-out");
        fs::write(out.join("keep.bin"), b"x").unwrap();
        let t = root.join("t");
        fs::create_dir_all(t.join("locked")).unwrap();
        fs::write(t.join("locked/a.bin"), b"abc").unwrap();
        fs::write(t.join("ok.bin"), b"12345").unwrap();
        std::os::unix::fs::symlink(&out, t.join("l")).unwrap();
        fs::set_permissions(t.join("locked"), fs::Permissions::from_mode(0o500)).unwrap();
        let p = purge(&t);
        fs::set_permissions(t.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(p.freed, 5);
        assert!(p.failed.iter().any(|(f, _, r)| f == &t.join("l") && *r == Reason::Link));
        // Como root el modo 500 no impide borrar.
        if fs::write(t.join("locked/prueba"), b"").is_err() {
            assert!(p.failed.iter().any(|(f, s, _)| f == &t.join("locked/a.bin") && *s == 3));
        }
        assert!(out.join("keep.bin").exists());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&out);
    }

    // Sin fallos de archivos, la carpeta que no se puede quitar sale con el error real, no con 0.
    #[cfg(unix)]
    #[test]
    fn purge_reports_stuck_dir_with_os_error() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("pg-stuck");
        let t = root.join("t");
        fs::create_dir_all(t.join("sub")).unwrap();
        fs::set_permissions(&t, fs::Permissions::from_mode(0o500)).unwrap();
        // Como root el modo 500 no impide borrar.
        let as_root = fs::create_dir(t.join("prueba")).is_ok();
        let p = purge(&t);
        let _ = fs::set_permissions(&t, fs::Permissions::from_mode(0o755));
        if !as_root {
            assert_eq!(p.failed, vec![(t.join("sub"), 0, Reason::Other(13))]);
        }
        let _ = fs::remove_dir_all(&root);
    }
}
