pub mod zones;
pub mod tree;
pub mod recycle;
pub mod parse;
pub mod games;

use tree::{Kind, Node};
use zones::Block;

/// La fila se puede borrar desde Carpetas: se conocen las zonas, no cae en ninguna, la ruta es
/// exacta y es una carpeta o un archivo de verdad (no un enlace, un `Rest` ni algo sin acceso).
pub fn deletable(node: &Node, block: Option<Block>, zones_known: bool) -> bool {
    zones_known && block.is_none() && !node.odd && matches!(node.kind, Kind::Dir | Kind::File)
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
}
