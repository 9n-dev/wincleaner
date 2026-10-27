pub mod zones;
pub mod tree;
pub mod recycle;
pub mod notice;
pub mod parse;
pub mod games;
pub mod dups;
pub mod installers;

use tree::{Kind, Node};
use std::path::{Path, PathBuf};

use crate::paths::is_valid;
use zones::{Block, Lock};

/// Filas que se pintan como mucho en la lista de Duplicados, que no está virtualizada: con
/// miles, la ventana se congela y la memoria se dispara. Carpetas no tiene tope.
pub const MAX_ROWS: usize = 200;

/// La fila se puede borrar desde Carpetas: se conocen las zonas, no cae en ninguna, la ruta es
/// exacta y es una carpeta o un archivo de verdad (no un enlace, un `Rest` ni algo sin acceso).
pub fn deletable(node: &Node, block: Option<Block>, zones_known: bool) -> bool {
    zones_known && block.is_none() && !node.odd && matches!(node.kind, Kind::Dir | Kind::File)
}

/// Por qué una fila no se puede borrar desde Carpetas. Solo explica: la puerta es `deletable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    NoZones,
    Odd,
    Link,
    Denied,
    Rest,
    Zone(Lock),
}

pub fn why(node: &Node, lock: Option<Lock>, zones_known: bool) -> Option<Why> {
    if !zones_known {
        return Some(Why::NoZones);
    }
    if node.odd {
        return Some(Why::Odd);
    }
    match node.kind {
        Kind::Link => Some(Why::Link),
        Kind::Denied => Some(Why::Denied),
        Kind::Rest | Kind::Opened => Some(Why::Rest),
        Kind::Dir | Kind::File => lock.map(Why::Zone),
    }
}

/// «4 carpetas y 1 284 archivos · 41,2 GB»: lo que hay en la carpeta. `size` va fuera de la
/// raíz (en la raíz ya lo dice el subtítulo).
pub fn count_text(dirs: u64, files: u64, size: Option<u64>) -> String {
    let n = |k: u64, one: &str, many: &str| if k == 1 { format!("1 {one}") } else { format!("{} {many}", crate::format::count(k)) };
    let mut parts = Vec::new();
    if dirs > 0 {
        parts.push(n(dirs, "carpeta", "carpetas"));
    }
    if files > 0 {
        parts.push(n(files, "archivo", "archivos"));
    }
    let mut s = parts.join(" y ");
    if let Some(b) = size {
        s.push_str(&format!(" · {}", crate::format::bytes(b)));
    }
    s
}

/// Qué abrir en el Explorador.
#[derive(Debug, PartialEq, Eq)]
pub enum Open {
    Folder(PathBuf),
    Select(PathBuf),
}

/// Solo se selecciona un archivo si `explorer.exe /select` lo entiende: ruta exacta y de menos de 260.
pub fn explorer_target(path: &Path, is_dir: bool, odd: bool) -> Option<Open> {
    let s = path.to_str()?;
    if !is_valid(s) {
        return None;
    }
    // Léxico, como las zonas: en Linux `Path::parent` no parte por `\`.
    let parent = || s.rfind(['\\', '/']).map(|i| PathBuf::from(&s[..if i <= 2 { i + 1 } else { i }]));
    if odd || (!is_dir && s.chars().count() >= 260) {
        return parent().map(Open::Folder);
    }
    Some(if is_dir { Open::Folder(path.into()) } else { Open::Select(path.into()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: Kind, odd: bool) -> Node {
        Node { name: "x".into(), parent: 0, first: 0, len: 0, size: 1, mtime: 0, kind, gone: false, odd }
    }

    #[test]
    fn can_enter_only_real_dirs() {
        assert!(node(Kind::Dir, false).can_enter());
        assert!(!node(Kind::Dir, true).can_enter());
        for k in [Kind::File, Kind::Rest, Kind::Denied, Kind::Link] {
            assert!(!node(k, false).can_enter(), "{k:?}");
        }
    }

    #[test]
    fn deletable_gate() {
        assert!(deletable(&node(Kind::Dir, false), None, true));
        assert!(deletable(&node(Kind::File, false), None, true));
        // Nombre no Unicode: la ruta no es exacta.
        assert!(!deletable(&node(Kind::File, true), None, true));
        assert!(!deletable(&node(Kind::Dir, true), None, true));
        for k in [Kind::Rest, Kind::Denied, Kind::Link] {
            assert!(!deletable(&node(k, false), None, true), "{k:?}");
        }
        for b in [Block::Protected, Block::Known, Block::System, Block::Game] {
            assert!(!deletable(&node(Kind::Dir, false), Some(b), true), "{b:?}");
        }
        // Sin zonas, nada se borra.
        assert!(!deletable(&node(Kind::File, false), None, false));
    }

    #[test]
    fn why_agrees_with_deletable() {
        let locks = [None, Some(Lock::Windows), Some(Lock::Known("Vídeos")), Some(Lock::Game), Some(Lock::SystemFile)];
        for kind in [Kind::Dir, Kind::File, Kind::Rest, Kind::Opened, Kind::Denied, Kind::Link] {
            for odd in [false, true] {
                for l in locks {
                    for zk in [false, true] {
                        let n = node(kind, odd);
                        assert_eq!(why(&n, l, zk).is_none(), deletable(&n, l.map(Lock::block), zk), "{kind:?} {odd} {l:?} {zk}");
                    }
                }
            }
        }
        assert_eq!(why(&node(Kind::Link, false), None, true), Some(Why::Link));
        assert_eq!(why(&node(Kind::Denied, true), None, true), Some(Why::Odd));
        assert_eq!(why(&node(Kind::File, false), None, false), Some(Why::NoZones));
    }

    #[test]
    fn count_texts() {
        assert_eq!(count_text(9, 6, None), "9 carpetas y 6 archivos");
        assert_eq!(count_text(4, 1284, Some(44_258_525_184)), "4 carpetas y 1 284 archivos · 41,2 GB");
        assert_eq!(count_text(1, 1, None), "1 carpeta y 1 archivo");
        assert_eq!(count_text(0, 3, None), "3 archivos");
        assert_eq!(count_text(2, 0, None), "2 carpetas");
    }

    #[test]
    fn explorer_target_rules() {
        let p = PathBuf::from;
        assert_eq!(explorer_target(&p(r"C:\a b,c\d"), true, false), Some(Open::Folder(p(r"C:\a b,c\d"))));
        assert_eq!(explorer_target(&p(r"C:\"), true, false), Some(Open::Folder(p(r"C:\"))));
        assert_eq!(explorer_target(&p(r"C:\a b,c\x.txt"), false, false), Some(Open::Select(p(r"C:\a b,c\x.txt"))));
        let long = format!(r"C:\{}\x.txt", "a".repeat(260));
        let up = format!(r"C:\{}", "a".repeat(260));
        assert_eq!(explorer_target(&p(&long), false, false), Some(Open::Folder(p(&up))));
        // Nombre no Unicode: la carpeta de arriba, que sí es exacta.
        assert_eq!(explorer_target(&p(r"C:\a\x"), false, true), Some(Open::Folder(p(r"C:\a"))));
        assert_eq!(explorer_target(&p(r"C:\a\x"), true, true), Some(Open::Folder(p(r"C:\a"))));
        assert_eq!(explorer_target(&p("relativa"), true, false), None);
    }
}
