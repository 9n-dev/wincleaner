//! Análisis del uso de disco: los archivos y las carpetas que más ocupan, sin seguir enlaces.

use crate::categories::is_link;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const TOP_N: usize = 50;

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub top_files: Vec<(PathBuf, u64)>,
    pub top_dirs: Vec<(PathBuf, u64)>,
    pub files_scanned: u64,
    pub cancelled: bool,
}

/// Elige hasta `n` carpetas sin que una contenga a otra, priorizando donde se concentra el espacio.
pub fn pick_top_dirs(mut dirs: Vec<(PathBuf, u64)>, n: usize) -> Vec<(PathBuf, u64)> {
    dirs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.components().count().cmp(&a.0.components().count())));
    let mut selected: Vec<(PathBuf, u64)> = Vec::new();
    for c in dirs {
        if selected.iter().any(|s| s.0.starts_with(&c.0)) {
            continue;
        }
        selected.retain(|s| !c.0.starts_with(&s.0));
        selected.push(c);
        if selected.len() == n {
            break;
        }
    }
    selected.sort_by_key(|s| Reverse(s.1));
    selected
}

pub fn analyze(root: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Analysis {
    // (ruta, índice del padre, bytes de archivos directos); los hijos siempre van después del padre.
    // ponytail: un PathBuf por carpeta (≈100+ MB en discos enormes); si pesa, guardar índice del padre + nombre.
    let mut dirs: Vec<(PathBuf, usize, u64)> = vec![(root.to_path_buf(), 0, 0)];
    let mut heap: BinaryHeap<Reverse<(u64, PathBuf)>> = BinaryHeap::new();
    let mut out = Analysis::default();
    let mut i = 0;
    while i < dirs.len() {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        let dir = dirs[i].0.clone();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let Ok(md) = std::fs::symlink_metadata(entry.path()) else { continue };
                if is_link(&md) {
                    continue;
                }
                if md.is_dir() {
                    dirs.push((entry.path(), i, 0));
                } else if md.is_file() {
                    dirs[i].2 += md.len();
                    heap.push(Reverse((md.len(), entry.path())));
                    if heap.len() > TOP_N {
                        heap.pop();
                    }
                    out.files_scanned += 1;
                    if out.files_scanned.is_multiple_of(10_000) {
                        progress(out.files_scanned);
                    }
                }
            }
        }
        i += 1;
    }
    let mut total: Vec<u64> = dirs.iter().map(|d| d.2).collect();
    for j in (1..dirs.len()).rev() {
        total[dirs[j].1] += total[j];
    }
    out.top_files = heap.into_sorted_vec().into_iter().map(|Reverse((s, p))| (p, s)).collect();
    let all = dirs.into_iter().zip(total).skip(1).map(|((p, _, _), t)| (p, t)).collect();
    out.top_dirs = pick_top_dirs(all, TOP_N);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::fs;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn write(path: &Path, size: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; size]).unwrap();
    }

    fn run(root: &Path) -> Analysis {
        analyze(root, &AtomicBool::new(false), &mut |_| {})
    }

    #[test]
    fn pick_prefers_where_space_concentrates() {
        let dirs = vec![(p("/r/a"), 100), (p("/r/a/b"), 90), (p("/r/a/b/c"), 80), (p("/r/d"), 50), (p("/r/a/e"), 5)];
        assert_eq!(pick_top_dirs(dirs.clone(), 2), vec![(p("/r/a/b/c"), 80), (p("/r/d"), 50)]);
        assert_eq!(
            pick_top_dirs(dirs, 3),
            vec![(p("/r/a/b/c"), 80), (p("/r/d"), 50), (p("/r/a/e"), 5)]
        );
    }

    #[test]
    fn pick_tie_keeps_deepest() {
        assert_eq!(pick_top_dirs(vec![(p("/r/x"), 10), (p("/r/x/y"), 10)], 5), vec![(p("/r/x/y"), 10)]);
    }

    #[test]
    fn analyze_sizes_and_top_files() {
        let root = temp_dir("an-sizes");
        write(&root.join("a/1.bin"), 100);
        write(&root.join("a/b/2.bin"), 50);
        write(&root.join("c/3.bin"), 10);
        let r = run(&root);
        assert_eq!(r.files_scanned, 3);
        assert_eq!(r.top_files[0], (root.join("a/1.bin"), 100));
        assert!(r.top_dirs.iter().all(|(d, _)| d != &root));
        assert!(r.top_dirs.contains(&(root.join("a/b"), 50)));
        assert!(r.top_dirs.contains(&(root.join("c"), 10)));
        assert!(!r.top_dirs.iter().any(|(d, _)| d == &root.join("a")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn analyze_keeps_only_top_50_files() {
        let root = temp_dir("an-top50");
        for i in 1..=55 {
            write(&root.join(format!("f{i}.bin")), i);
        }
        let r = run(&root);
        assert_eq!(r.top_files.len(), 50);
        assert_eq!(r.top_files[0].1, 55);
        assert_eq!(r.top_files[49].1, 6);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn analyze_does_not_follow_symlinks() {
        let root = temp_dir("an-link");
        let outside = temp_dir("an-link-out");
        write(&outside.join("big.bin"), 1000);
        std::os::unix::fs::symlink(&outside, root.join("l")).unwrap();
        let r = run(&root);
        assert_eq!(r.files_scanned, 0);
        assert!(r.top_files.is_empty());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn analyze_skips_unreadable_dirs() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("an-perm");
        let locked = root.join("locked");
        fs::create_dir_all(&locked).unwrap();
        write(&root.join("ok/1.bin"), 5);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let r = run(&root);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!r.cancelled);
        assert_eq!(r.files_scanned, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn analyze_honours_cancel() {
        let root = temp_dir("an-cancel");
        write(&root.join("1.bin"), 5);
        let r = analyze(&root, &AtomicBool::new(true), &mut |_| {});
        assert!(r.cancelled);
        assert_eq!(r.files_scanned, 0);
        let _ = fs::remove_dir_all(&root);
    }
}
