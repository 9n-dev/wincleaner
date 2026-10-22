//! Escaneo y borrado seguros: nunca se siguen enlaces y todo error de E/S cuenta como «saltado».

use crate::categories::{is_link, Category, Filter, Status};
use crate::paths::is_inside;
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

pub fn delete(cat: &Category, scan: &ScanResult, now: SystemTime) -> DeleteReport {
    #[cfg(windows)]
    let finals = handle::final_roots(&cat.roots);
    let mut rep = DeleteReport::default();
    for (path, size) in &scan.files {
        #[cfg(windows)]
        let freed = handle::try_delete(cat, path, now, &finals);
        #[cfg(not(windows))]
        let freed = try_delete(cat, path, now);
        match freed {
            Ok(Some(n)) => {
                rep.freed += n;
                rep.deleted += 1;
            }
            Ok(None) => {}
            Err(()) => {
                rep.skipped += 1;
                rep.skipped_bytes += size;
            }
        }
    }
    for dir in &scan.dirs {
        #[cfg(windows)]
        handle::remove_dir(dir, &cat.roots, &finals);
        #[cfg(not(windows))]
        if safe_to_touch(dir, &cat.roots) && std::fs::symlink_metadata(dir).is_ok_and(|md| !is_link(&md)) {
            let _ = std::fs::remove_dir(dir);
        }
    }
    rep
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
fn try_delete(cat: &Category, path: &Path, now: SystemTime) -> Result<Option<u64>, ()> {
    if !in_any_root(path, &cat.roots) {
        return Err(());
    }
    let md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    if is_link(&md) || !md.is_file() || !age_ok(&md, cat.filter, now) || !safe_to_touch(path, &cat.roots) {
        return Err(());
    }
    // ponytail: este camino por ruta solo existe para las pruebas en Linux y deja una ventana
    // TOCTOU entre comprobar y borrar; en Windows se borra por handle (mod `handle`).
    if md.permissions().readonly() {
        let mut perm = md.permissions();
        perm.set_readonly(false);
        let _ = std::fs::set_permissions(path, perm);
    }
    std::fs::remove_file(path).map(|_| Some(md.len())).map_err(|_| ())
}

/// Borrado por handle: se abre el objeto sin seguir enlaces en el último componente, se
/// comprueba por ese handle (tipo, antigüedad y ruta final dentro de la raíz) y se borra
/// por ese mismo handle. No hay ninguna resolución de ruta entre la comprobación y el
/// borrado, así que cambiar una carpeta intermedia por un junction no sirve de nada.
/// Cualquier fallo es «saltado»; nunca se recurre al borrado por ruta.
#[cfg(windows)]
mod handle {
    use super::{age_ok, in_any_root};
    use crate::categories::{is_link, Category};
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
    pub fn try_delete(cat: &Category, path: &Path, now: SystemTime, finals: &[String]) -> Result<Option<u64>, ()> {
        if !in_any_root(path, &cat.roots) {
            return Err(());
        }
        let f = match open(path, DELETE | FILE_READ_ATTRIBUTES) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(()),
        };
        let md = f.metadata().map_err(|_| ())?;
        if is_link(&md) || !md.is_file() || !age_ok(&md, cat.filter, now) || !inside(&f, finals) {
            return Err(());
        }
        if mark_deleted(&f) { Ok(Some(md.len())) } else { Err(()) }
    }

    /// Borra `dir` si está vacía; si no, falla y se ignora.
    pub fn remove_dir(dir: &Path, roots: &[PathBuf], finals: &[String]) {
        if !in_any_root(dir, roots) {
            return;
        }
        let Ok(f) = open(dir, DELETE | FILE_READ_ATTRIBUTES) else { return };
        if f.metadata().is_ok_and(|md| md.is_dir() && !is_link(&md)) && inside(&f, finals) {
            mark_deleted(&f);
        }
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
        Category { id: CategoryId::UserTemp, name: "t", filter, roots: vec![root.to_path_buf()], status }
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

}
