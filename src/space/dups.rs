//! Búsqueda de duplicados en tres pasadas: tamaño, hash de los primeros 64 KB (solo reparte) y
//! comparación byte a byte (lo único que decide).

use crate::categories::is_link;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

const BLOCK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Document,
    Video,
    Music,
    Other,
}

pub fn kind(name: &str) -> FileKind {
    let ext = match name.rsplit_once('.') {
        Some((_, e)) => e.to_lowercase(),
        None => return FileKind::Other,
    };
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" | "bmp" | "tif" | "tiff" | "raw" | "cr2" | "nef"
        | "arw" | "dng" | "svg" | "ico" => FileKind::Image,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "rtf" | "txt" | "md"
        | "csv" | "epub" => FileKind::Document,
        "mp4" | "mkv" | "mov" | "avi" | "wmv" | "webm" | "m4v" | "mpg" | "mpeg" | "flv" | "3gp" => FileKind::Video,
        "mp3" | "flac" | "wav" | "m4a" | "aac" | "ogg" | "opus" | "wma" => FileKind::Music,
        _ => FileKind::Other,
    }
}

pub fn kind_label(k: FileKind) -> &'static str {
    match k {
        FileKind::Image => "Imagen",
        FileKind::Document => "Documento",
        FileKind::Video => "Vídeo",
        FileKind::Music => "Música",
        FileKind::Other => "Archivo",
    }
}

#[derive(Clone, Debug)]
pub struct DupFile {
    pub path: PathBuf,
    pub modified: SystemTime,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub size: u64,
    pub files: Vec<DupFile>,
}

#[derive(Clone, Copy, Debug)]
pub enum Phase {
    Listing(u64),
    Sizes(u64),
    Content(u64, u64),
}

#[cfg(windows)]
fn hidden(md: &std::fs::Metadata, _name: &std::ffi::OsStr) -> bool {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes() & 0x6 != 0 // HIDDEN | SYSTEM
}

#[cfg(not(windows))]
fn hidden(_md: &std::fs::Metadata, name: &std::ffi::OsStr) -> bool {
    std::os::unix::ffi::OsStrExt::as_bytes(name).first() == Some(&b'.')
}

/// Lee hasta llenar `buf` o hasta el fin del archivo; devuelve los bytes leídos.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

pub fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut fa, mut fb) = (std::fs::File::open(a)?, std::fs::File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; BLOCK], vec![0u8; BLOCK]);
    loop {
        let (na, nb) = (read_full(&mut fa, &mut ba)?, read_full(&mut fb, &mut bb)?);
        if na != nb || ba[..na] != bb[..nb] {
            return Ok(false);
        }
        if na < BLOCK {
            return Ok(true);
        }
    }
}

fn head_hash(p: &Path) -> io::Result<u64> {
    let mut buf = vec![0u8; BLOCK];
    let n = read_full(&mut std::fs::File::open(p)?, &mut buf)?;
    let mut h = DefaultHasher::new();
    buf[..n].hash(&mut h);
    Ok(h.finish())
}

pub fn find(
    roots: &[PathBuf],
    min_size: u64,
    skip_dir: &dyn Fn(&Path) -> bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Phase),
) -> Option<Vec<Group>> {
    // Pasada 1: lista y agrupa por tamaño.
    // ponytail: se guardan todas las rutas de `min_size` o más (unos 100 MB con un millón de
    // archivos). Si pesa, contar primero por tamaño y guardar después solo las rutas candidatas.
    let mut by_size: HashMap<u64, Vec<DupFile>> = HashMap::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    let mut seen = 0u64;
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue }; // DirEntry::metadata no sigue enlaces
            if is_link(&md) || hidden(&md, &e.file_name()) {
                continue;
            }
            let path = e.path();
            if md.is_dir() {
                if !skip_dir(&path) {
                    stack.push(path);
                }
            } else if md.is_file() && md.len() > 0 && md.len() >= min_size {
                let modified = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                by_size.entry(md.len()).or_default().push(DupFile { path, modified });
                seen += 1;
                if seen.is_multiple_of(1000) {
                    progress(Phase::Listing(seen));
                }
            }
        }
    }
    progress(Phase::Listing(seen));
    by_size.retain(|_, v| v.len() > 1);
    let total: u64 = by_size.values().map(|v| v.len() as u64).sum();
    progress(Phase::Sizes(total));

    // Pasadas 2 y 3.
    let mut groups = Vec::new();
    let mut done = 0u64;
    for (size, files) in by_size {
        let mut buckets: HashMap<u64, Vec<DupFile>> = HashMap::new();
        for f in files {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if let Ok(h) = head_hash(&f.path) {
                buckets.entry(h).or_default().push(f);
            }
        }
        for (_, bucket) in buckets {
            let mut classes: Vec<Vec<DupFile>> = Vec::new();
            for f in bucket {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                done += 1;
                if done.is_multiple_of(16) {
                    progress(Phase::Content(done, total));
                }
                let mut placed = false;
                for c in classes.iter_mut() {
                    match same_content(&c[0].path, &f.path) {
                        Ok(true) => {
                            c.push(f.clone());
                            placed = true;
                            break;
                        }
                        Ok(false) => {}
                        Err(_) => {
                            placed = true; // ilegible: se salta
                            break;
                        }
                    }
                }
                if !placed {
                    classes.push(vec![f]);
                }
            }
            groups.extend(classes.into_iter().filter(|c| c.len() > 1).map(|files| Group { size, files }));
        }
    }
    progress(Phase::Content(total, total));
    groups.sort_by_key(|g| std::cmp::Reverse(g.size.saturating_mul(g.files.len() as u64 - 1)));
    Some(groups)
}

/// Quita las raíces repetidas o que están dentro de otra.
pub fn dedupe_roots(v: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for (i, p) in v.iter().enumerate() {
        let dup = v.iter().enumerate().any(|(j, q)| j != i && p.starts_with(q) && (p != q || j < i));
        if !dup {
            out.push(p.clone());
        }
    }
    out
}

pub struct Keep {
    /// Rutas completas de Documentos, Imágenes, Vídeos y Música.
    pub natural: Vec<String>,
    /// Rutas completas de Descargas y Escritorio.
    pub loose: Vec<String>,
}

fn rank(p: &Path, k: &Keep) -> u8 {
    let s = p.to_string_lossy();
    let inside = |dirs: &[String]| dirs.iter().any(|d| crate::paths::is_inside(&s, d));
    if inside(&k.natural) {
        0
    } else if inside(&k.loose) {
        2
    } else {
        1
    }
}

/// Índice de la copia que se conserva: mejor rango, la más antigua y, si siguen empatadas, la
/// ruta menor (para que el resultado no dependa del orden).
pub fn keeper(g: &Group, k: &Keep) -> usize {
    (0..g.files.len()).min_by_key(|&i| (rank(&g.files[i].path, k), g.files[i].modified, &g.files[i].path)).unwrap_or(0)
}

/// `false` si marcar `i` deja el grupo sin ninguna copia.
pub fn can_mark(marked: &[bool], i: usize) -> bool {
    marked.iter().enumerate().any(|(j, &m)| j != i && !m)
}

/// Justo antes de enviar: las dos copias existen, pesan `size` y siguen siendo iguales.
pub fn still_duplicate(kept: &Path, marked: &Path, size: u64) -> bool {
    let len_ok = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && m.len() == size);
    len_ok(kept) && len_ok(marked) && same_content(kept, marked).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::time::Duration;

    fn run(roots: &[PathBuf], min: u64) -> Vec<Group> {
        find(roots, min, &|_| false, &AtomicBool::new(false), &mut |_| {}).unwrap()
    }
    fn put(d: &Path, name: &str, b: &[u8]) -> PathBuf {
        let p = d.join(name);
        std::fs::write(&p, b).unwrap();
        p
    }

    #[test]
    fn same_size_different_content_never_grouped() {
        let d = temp_dir("dups-big");
        let mut a = vec![7u8; 200_000];
        put(&d, "a", &a);
        a[199_999] = 8;
        put(&d, "b", &a);
        assert!(run(&[d], 1).is_empty());
    }

    #[test]
    fn small_files_differing_one_byte_never_grouped() {
        let d = temp_dir("dups-small");
        let mut a = vec![1u8; 20_000];
        put(&d, "a", &a);
        a[10_000] = 2;
        put(&d, "b", &a);
        assert!(run(&[d], 1).is_empty());
    }

    #[test]
    fn identical_files_grouped_any_type() {
        let d = temp_dir("dups-ok");
        for n in ["a.pdf", "b.pdf"] {
            put(&d, n, &[1u8; 20_000]);
        }
        for n in ["a.png", "b.png", "c.png"] {
            put(&d, n, &[2u8; 30_000]);
        }
        let g = run(&[d], 1);
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].size, g[0].files.len()), (30_000, 3));
        assert_eq!((g[1].size, g[1].files.len()), (20_000, 2));
    }

    #[test]
    fn min_size_respected() {
        let d = temp_dir("dups-min");
        put(&d, "a", &[1u8; 5_000]);
        put(&d, "b", &[1u8; 5_000]);
        assert!(run(&[d], 10_000).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn zero_bytes_links_hidden_ignored() {
        let d = temp_dir("dups-ign");
        put(&d, "e1", b"");
        put(&d, "e2", b"");
        let real = put(&d, "real", &[3u8; 100]);
        put(&d, "copy", &[3u8; 100]);
        std::os::unix::fs::symlink(&real, d.join("link")).unwrap();
        put(&d, ".a", &[9u8; 100]);
        put(&d, ".b", &[9u8; 100]);
        let g = run(&[d], 1);
        assert_eq!(g.len(), 1);
        let mut names: Vec<_> = g[0].files.iter().map(|f| f.path.file_name().unwrap().to_owned()).collect();
        names.sort();
        assert_eq!(names, ["copy", "real"]);
    }

    #[test]
    fn skip_dir_respected() {
        let d = temp_dir("dups-skip");
        std::fs::create_dir(d.join("skip")).unwrap();
        put(&d, "a", &[1u8; 50]);
        put(&d.join("skip"), "b", &[1u8; 50]);
        let g = find(&[d], 1, &|p| p.ends_with("skip"), &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(g.is_empty());
    }

    #[test]
    fn cancel_returns_none() {
        let d = temp_dir("dups-cancel");
        put(&d, "a", &[1u8; 50]);
        put(&d, "b", &[1u8; 50]);
        assert!(find(&[d], 1, &|_| false, &AtomicBool::new(true), &mut |_| {}).is_none());
    }

    #[test]
    fn kind_by_extension() {
        assert_eq!(kind("IMG_4821.PNG"), FileKind::Image);
        assert_eq!(kind("Contrato.pdf"), FileKind::Document);
        assert_eq!(kind("a.MKV"), FileKind::Video);
        assert_eq!(kind("b.flac"), FileKind::Music);
        assert_eq!(kind("c.zip"), FileKind::Other);
        assert_eq!(kind("sin_ext"), FileKind::Other);
        assert_eq!(kind_label(FileKind::Video), "Vídeo");
    }

    #[test]
    fn keeper_prefers_natural_then_oldest() {
        let t = |y: u64| SystemTime::UNIX_EPOCH + Duration::from_secs((y - 1970) * 31_536_000);
        let f = |p: &str, y| DupFile { path: p.into(), modified: t(y) };
        let k = Keep { natural: vec![r"C:\Users\x\Videos".into()], loose: vec![r"C:\Users\x\Downloads".into()] };
        let g = Group { size: 1, files: vec![f(r"C:\Users\x\Downloads\v.mp4", 2022), f(r"C:\Users\x\Videos\v.mp4", 2023), f(r"D:\Copia\v.mp4", 2024)] };
        assert_eq!(keeper(&g, &k), 1);
        let g = Group { size: 1, files: vec![f(r"D:\Copia\v.mp4", 2024), f(r"D:\Otra\v.mp4", 2023)] };
        assert_eq!(keeper(&g, &k), 1);
    }

    #[test]
    fn backup_named_like_known_is_not_natural() {
        let t = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let f = |p: &str, y| DupFile { path: p.into(), modified: t(y) };
        let k = Keep { natural: vec![r"C:\Users\ana\Videos".into()], loose: vec![] };
        let g = Group { size: 1, files: vec![f(r"D:\Copia\Videos\v.mp4", 1_000), f(r"C:\Users\ana\Videos\v.mp4", 2_000)] };
        assert_eq!(keeper(&g, &k), 1);
    }

    #[test]
    fn cannot_mark_last_copy() {
        assert!(!can_mark(&[false, true, true], 0));
        assert!(can_mark(&[false, false, true], 0));
    }

    #[test]
    fn changed_file_is_not_sent() {
        let d = temp_dir("dups-changed");
        let k = put(&d, "k", &[5u8; 1000]);
        let m = put(&d, "m", &[5u8; 1000]);
        assert!(still_duplicate(&k, &m, 1000));
        let mut b = vec![5u8; 1000];
        b[500] = 6;
        std::fs::write(&m, &b).unwrap();
        assert!(!still_duplicate(&k, &m, 1000));
        std::fs::write(&m, [5u8; 1000]).unwrap();
        std::fs::remove_file(&k).unwrap();
        assert!(!still_duplicate(&k, &m, 1000));
    }

    #[test]
    fn dedupe_roots_drops_nested() {
        let v = vec![PathBuf::from("/a/b"), PathBuf::from("/a"), PathBuf::from("/c"), PathBuf::from("/c")];
        assert_eq!(dedupe_roots(v), [PathBuf::from("/a"), PathBuf::from("/c")]);
    }
}
