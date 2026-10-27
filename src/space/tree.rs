//! Árbol compacto del disco: un `Vec<Node>` donde los hijos de cada carpeta son contiguos.

use crate::categories::is_link;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub type Id = u32;

/// Archivos que se guardan por carpeta; el resto se agrupa en un `Rest`.
pub const FILES_PER_DIR: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    /// «N archivos más pequeños»: N va en `len` y no tiene hijos.
    Rest,
    /// `Rest` ya leído: sus archivos van en `first..first + len`.
    Opened,
    /// Carpeta que no se pudo leer; su tamaño es desconocido.
    Denied,
    /// Enlace o junction: tamaño 0 y no se entra.
    Link,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: Box<str>,
    pub parent: Id,
    /// Los hijos de una carpeta son `first..first + len`.
    pub first: Id,
    pub len: u32,
    pub size: u64,
    /// Segundos desde 1970.
    pub mtime: u32,
    pub kind: Kind,
    pub gone: bool,
    /// El nombre no es Unicode válido y `name` es una versión con U+FFFD: se muestra, pero la ruta
    /// no es exacta, así que nunca se borra ni se entra (una carpeta así es `Denied`).
    pub odd: bool,
}

impl Node {
    /// Se puede entrar: una carpeta leída con nombre Unicode (si no, la ruta no sería exacta).
    pub fn can_enter(&self) -> bool {
        self.kind == Kind::Dir && !self.odd
    }
}

pub struct Tree {
    nodes: Vec<Node>,
    base: PathBuf,
    files: u64,
}

fn rest_name(n: u32) -> Box<str> {
    format!("{n} archivos más pequeños").into()
}

fn mtime(md: &std::fs::Metadata) -> u32 {
    let s = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
    s.map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32)
}

type Kid = (Box<str>, Kind, u64, u32);
/// Hijos de una carpeta y posiciones de los de nombre no Unicode.
/// (tamaño, nombre, mtime, nombre no Unicode).
type Small = (u64, Box<str>, u32, bool);
type Read = (Vec<Kid>, Vec<usize>);

impl Tree {
    pub fn new(root_name: &str) -> Tree {
        let root = Node { name: root_name.into(), parent: 0, first: 0, len: 0, size: 0, mtime: 0, kind: Kind::Dir, gone: false, odd: false };
        Tree { nodes: vec![root], base: PathBuf::from(root_name), files: 0 }
    }

    /// Recorre `root` en anchura, sin seguir enlaces. `progress` recibe cada 10 000 archivos
    /// el total, los bytes y la carpeta que se lee. `None` si se cancela.
    pub fn scan(root: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64, &Path)) -> Option<Tree> {
        let mut t = Tree::new(&root.to_string_lossy());
        t.base = root.to_path_buf();
        let (mut files, mut bytes) = (0u64, 0u64);
        let mut i = 0;
        while i < t.nodes.len() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if t.nodes[i].kind == Kind::Dir {
                let dir = t.path(i as Id);
                match read_kids(&dir, &mut files, &mut bytes, progress) {
                    Ok((kids, odd)) => {
                        let first = t.add_children(i as Id, kids);
                        for k in odd {
                            t.nodes[first as usize + k].odd = true;
                        }
                    }
                    Err(true) => t.nodes[i].kind = Kind::Denied,
                    Err(false) => {}
                }
            }
            i += 1;
        }
        t.finish();
        Some(t)
    }

    /// Añade los hijos de `parent` de una vez (quedan contiguos) y devuelve el primero.
    /// Cada hijo es (nombre, tipo, tamaño, mtime); en un `Rest` el último campo es N.
    pub fn add_children(&mut self, parent: Id, kids: Vec<Kid>) -> Id {
        let first = self.nodes.len() as Id;
        let p = &mut self.nodes[parent as usize];
        (p.first, p.len) = (first, kids.len() as u32);
        self.nodes.reserve(kids.len());
        for (name, kind, size, m) in kids {
            let (len, mtime) = if kind == Kind::Rest { (m, 0) } else { (0, m) };
            self.files += match kind {
                Kind::File => 1,
                Kind::Rest => m as u64,
                _ => 0,
            };
            self.nodes.push(Node { name, parent, first: 0, len, size, mtime, kind, gone: false, odd: false });
        }
        first
    }

    /// Suma los tamaños hacia arriba (los hijos siempre van después de su padre).
    pub fn finish(&mut self) {
        for i in (1..self.nodes.len()).rev() {
            let (p, s) = (self.nodes[i].parent as usize, self.nodes[i].size);
            self.nodes[p].size += s;
        }
    }

    pub fn node(&self, id: Id) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn root(&self) -> Id {
        0
    }

    /// Archivos leídos por el escaneo: cifra histórica que `remove`, `restore`, `graft` y `forget` no cambian.
    pub fn files(&self) -> u64 {
        self.files
    }

    /// Hijos que siguen en pie, de mayor a menor y con el `Rest` al final.
    pub fn children(&self, id: Id) -> Vec<Id> {
        let n = self.node(id);
        if n.kind != Kind::Dir {
            return Vec::new();
        }
        let mut v: Vec<Id> = (n.first..n.first + n.len).filter(|&c| !self.node(c).gone).collect();
        v.sort_by_key(|&c| (self.node(c).kind == Kind::Rest, Reverse(self.node(c).size)));
        v
    }

    /// `children` partido en (las `max` más grandes con el `Rest` detrás, las que no caben):
    /// una carpeta con 20 000 subcarpetas no puede ser 20 000 filas.
    pub fn split_children(&self, id: Id, max: usize) -> (Vec<Id>, Vec<Id>) {
        let (mut shown, rest): (Vec<Id>, Vec<Id>) = self.children(id).into_iter().partition(|&c| self.node(c).kind != Kind::Rest);
        let hidden = shown.split_off(max.min(shown.len()));
        shown.extend(rest);
        (shown, hidden)
    }

    /// De la raíz a `id`, ambos incluidos.
    pub fn crumbs(&self, id: Id) -> Vec<Id> {
        let mut v = vec![id];
        while *v.last().unwrap_or(&0) != 0 {
            v.push(self.node(*v.last().unwrap_or(&0)).parent);
        }
        v.reverse();
        v
    }

    pub fn path(&self, id: Id) -> PathBuf {
        let mut p = self.base.clone();
        p.extend(self.crumbs(id).iter().skip(1).map(|&c| &*self.node(c).name));
        p
    }

    pub fn find(&self, path: &Path) -> Option<Id> {
        self.lookup(path, false)
    }

    fn lookup(&self, path: &Path, with_gone: bool) -> Option<Id> {
        let mut cur = 0;
        for c in path.strip_prefix(&self.base).ok()?.components() {
            let name = c.as_os_str().to_str()?;
            let n = self.node(cur);
            let mut kids = n.first..n.first + if n.kind == Kind::Dir { n.len } else { 0 };
            cur = kids.find(|&k| {
                let k = self.node(k);
                k.kind != Kind::Rest && !k.odd && (with_gone || !k.gone) && &*k.name == name
            })?;
        }
        Some(cur)
    }

    /// Cambia `size` en `id` y en todos sus antecesores.
    fn bump(&mut self, mut id: Id, f: impl Fn(u64) -> u64) {
        loop {
            let n = &mut self.nodes[id as usize];
            n.size = f(n.size);
            if id == 0 {
                break;
            }
            id = n.parent;
        }
    }

    /// Lo marca `gone` y resta su tamaño a los antecesores.
    pub fn remove(&mut self, id: Id) {
        let n = self.node(id);
        if id == 0 || n.gone {
            return;
        }
        let (p, s) = (n.parent, n.size);
        self.nodes[id as usize].gone = true;
        self.bump(p, |x| x.saturating_sub(s));
    }

    pub fn restore(&mut self, id: Id) {
        let n = self.node(id);
        if !n.gone {
            return;
        }
        let (p, s) = (n.parent, n.size);
        self.nodes[id as usize].gone = false;
        self.bump(p, |x| x + s);
    }

    /// Sustituye el contenido de `id` por el de `sub` (un `scan` de la misma ruta).
    // ponytail: los nodos viejos quedan huérfanos y ocupan memoria hasta el siguiente escaneo; compactar si se hacen miles de graft.
    pub fn graft(&mut self, id: Id, sub: Tree) {
        let base = self.nodes.len() as Id;
        let shift = |x: Id| x - 1 + base;
        let mut it = sub.nodes.into_iter();
        let Some(r) = it.next() else { return };
        for mut n in it {
            n.parent = if n.parent == 0 { id } else { shift(n.parent) };
            if n.kind == Kind::Dir && n.len > 0 {
                n.first = shift(n.first);
            }
            self.nodes.push(n);
        }
        let n = &mut self.nodes[id as usize];
        let old = n.size;
        (n.first, n.len, n.kind) = (if r.len > 0 { shift(r.first) } else { 0 }, r.len, r.kind);
        if r.size >= old {
            self.bump(id, |x| x + (r.size - old));
        } else {
            self.bump(id, |x| x.saturating_sub(old - r.size));
        }
    }

    /// Lo borrado fuera del árbol (Duplicados, Instaladores): si el archivo es un nodo se quita;
    /// si no, se resta `size` y 1 al `Rest` de su carpeta.
    pub fn forget(&mut self, path: &Path, size: u64) {
        if let Some(id) = self.lookup(path, true) {
            return self.remove(id);
        }
        let Some(dir) = path.parent().and_then(|d| self.find(d)) else { return };
        let n = self.node(dir);
        let Some(r) = (n.first..n.first + n.len).find(|&c| self.node(c).kind == Kind::Rest && !self.node(c).gone && self.node(c).len > 0) else { return };
        let len = self.node(r).len.saturating_sub(1);
        self.nodes[r as usize].len = len;
        self.nodes[r as usize].name = rest_name(len);
        self.nodes[r as usize].gone = len == 0;
        self.nodes[r as usize].size = self.node(r).size.saturating_sub(size);
        self.bump(dir, |x| x.saturating_sub(size));
    }
}

/// Subcarpetas, enlaces, los `FILES_PER_DIR` archivos más grandes y el `Rest`.
/// `Err(true)` si no se puede leer por permisos.
fn read_kids(dir: &Path, files: &mut u64, bytes: &mut u64, progress: &mut dyn FnMut(u64, u64, &Path)) -> Result<Read, bool> {
    let rd = std::fs::read_dir(dir).map_err(|e| e.kind() == std::io::ErrorKind::PermissionDenied)?;
    let mut kids: Vec<Kid> = Vec::new();
    let mut odd = Vec::new(); // posiciones de `kids` con nombre no Unicode
    let mut heap: BinaryHeap<Reverse<Small>> = BinaryHeap::new();
    let (mut rest_n, mut rest_size) = (0u32, 0u64);
    for entry in rd.flatten() {
        let Ok(md) = std::fs::symlink_metadata(entry.path()) else { continue };
        let fname = entry.file_name();
        let is_odd = fname.to_str().is_none();
        let name: Box<str> = fname.to_string_lossy().into();
        if is_link(&md) || md.is_dir() {
            let kind = if is_link(&md) { Kind::Link } else if is_odd { Kind::Denied } else { Kind::Dir };
            if is_odd {
                odd.push(kids.len());
            }
            kids.push((name, kind, 0, mtime(&md)));
        } else if md.is_file() {
            heap.push(Reverse((md.len(), name, mtime(&md), is_odd)));
            if heap.len() > FILES_PER_DIR
                && let Some(Reverse((s, ..))) = heap.pop()
            {
                rest_n += 1;
                rest_size += s;
            }
            *files += 1;
            *bytes += md.len();
            if files.is_multiple_of(10_000) {
                progress(*files, *bytes, dir);
            }
        }
    }
    for Reverse((s, n, m, o)) in heap.into_sorted_vec() {
        if o {
            odd.push(kids.len());
        }
        kids.push((n, Kind::File, s, m));
    }
    if rest_n > 0 {
        kids.push((rest_name(rest_n), Kind::Rest, rest_size, rest_n));
    }
    Ok((kids, odd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::fs;

    fn write(path: &Path, size: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; size]).unwrap();
    }

    fn scan(root: &Path) -> Tree {
        Tree::scan(root, &AtomicBool::new(false), &mut |_, _, _| {}).unwrap()
    }

    fn names(t: &Tree, id: Id) -> Vec<String> {
        t.children(id).into_iter().map(|c| t.node(c).name.to_string()).collect()
    }

    #[test]
    fn scan_sizes_propagate() {
        let root = temp_dir("tr-sizes");
        write(&root.join("a/1.bin"), 100);
        write(&root.join("a/b/2.bin"), 50);
        write(&root.join("c/3.bin"), 10);
        let t = scan(&root);
        assert_eq!(t.node(t.root()).size, 160);
        let a = t.find(&root.join("a")).unwrap();
        assert_eq!(t.node(a).size, 150);
        assert_eq!(names(&t, t.root()), ["a", "c"]);
        assert_eq!(t.files(), 3);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_keeps_top_50_and_rest() {
        let root = temp_dir("tr-top50");
        for i in 1..=55 {
            write(&root.join(format!("f{i}.bin")), i);
        }
        let t = scan(&root);
        let kids = t.children(t.root());
        assert_eq!(kids.len(), 51);
        assert_eq!(t.node(kids[0]).size, 55);
        assert_eq!(t.node(kids[49]).size, 6);
        let rest = t.node(kids[50]);
        assert_eq!((rest.kind, rest.len, rest.size), (Kind::Rest, 5, 15));
        assert_eq!(t.files(), 55);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn split_children_caps_and_keeps_rest() {
        let mut t = Tree::new("C:\\");
        let kids = (1..=5u64).map(|i| (format!("d{i}").into(), Kind::Dir, i * 10, 0)).chain([("r".into(), Kind::Rest, 1, 7)]).collect();
        t.add_children(t.root(), kids);
        t.finish();
        let (shown, hidden) = t.split_children(t.root(), 3);
        let n = |v: &[Id]| v.iter().map(|&c| t.node(c).name.to_string()).collect::<Vec<_>>();
        assert_eq!(n(&shown), ["d5", "d4", "d3", "r"]);
        assert_eq!(n(&hidden), ["d2", "d1"]);
        let (shown, hidden) = t.split_children(t.root(), 200);
        assert_eq!((shown.len(), hidden.len()), (6, 0));
    }

    #[cfg(unix)]
    #[test]
    fn scan_does_not_follow_links() {
        let root = temp_dir("tr-link");
        let outside = temp_dir("tr-link-out");
        write(&outside.join("big.bin"), 1000);
        std::os::unix::fs::symlink(&outside, root.join("l")).unwrap();
        let t = scan(&root);
        let l = t.find(&root.join("l")).unwrap();
        assert_eq!((t.node(l).kind, t.node(l).size), (Kind::Link, 0));
        assert_eq!(t.node(t.root()).size, 0);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn scan_marks_denied() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("tr-perm");
        let locked = root.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let t = scan(&root);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // como root el modo 000 no impide leer
        if fs::read_dir(&locked).is_err() {
            assert_eq!(t.node(t.find(&locked).unwrap()).kind, Kind::Denied);
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_cancel_returns_none() {
        let root = temp_dir("tr-cancel");
        write(&root.join("1.bin"), 5);
        assert!(Tree::scan(&root, &AtomicBool::new(true), &mut |_, _, _| {}).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn progress_reports_bytes_and_dir() {
        let root = temp_dir("tr-progress");
        fs::create_dir_all(root.join("a")).unwrap();
        for i in 0..10_001 {
            fs::write(root.join(format!("a/{i}")), b"x").unwrap();
        }
        let mut seen = Vec::new();
        Tree::scan(&root, &AtomicBool::new(false), &mut |f, b, d| seen.push((f, b, d.to_path_buf()))).unwrap();
        assert_eq!(seen, [(10_000, 10_000, root.join("a"))]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn path_roundtrip_unicode() {
        let root = temp_dir("tr-unicode");
        write(&root.join("vídeos/boda 🎉.mov"), 7);
        let t = scan(&root);
        let id = t.children(t.find(&root.join("vídeos")).unwrap())[0];
        assert_eq!(t.path(id), root.join("vídeos").join("boda 🎉.mov"));
        assert_eq!(t.find(&t.path(id)), Some(id));
        assert_eq!(t.path(t.root()), root);
        assert_eq!(t.crumbs(id).len(), 3);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_and_restore_update_ancestors() {
        let root = temp_dir("tr-remove");
        write(&root.join("a/1.bin"), 100);
        write(&root.join("a/b/2.bin"), 50);
        let mut t = scan(&root);
        let (a, b) = (t.find(&root.join("a")).unwrap(), t.find(&root.join("a/b")).unwrap());
        t.remove(b);
        assert_eq!((t.node(a).size, t.node(t.root()).size), (100, 100));
        assert_eq!(names(&t, a), ["1.bin"]);
        assert!(t.find(&root.join("a/b")).is_none());
        t.remove(b);
        assert_eq!(t.node(t.root()).size, 100);
        t.restore(b);
        assert_eq!((t.node(a).size, t.node(t.root()).size), (150, 150));
        assert_eq!(names(&t, a), ["1.bin", "b"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn graft_replaces_content() {
        let root = temp_dir("tr-graft");
        write(&root.join("a/1.bin"), 100);
        write(&root.join("a/b/2.bin"), 50);
        write(&root.join("c/3.bin"), 10);
        let mut t = scan(&root);
        let a = t.find(&root.join("a")).unwrap();
        fs::remove_dir_all(root.join("a/b")).unwrap();
        t.graft(a, scan(&root.join("a")));
        assert_eq!((t.node(a).size, t.node(t.root()).size), (100, 110));
        assert_eq!(names(&t, a), ["1.bin"]);
        let f = t.children(a)[0];
        assert_eq!((t.path(f), t.node(f).parent), (root.join("a/1.bin"), a));
        assert!(t.find(&root.join("a/b")).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn forget_shrinks_rest() {
        let root = temp_dir("tr-forget");
        for i in 1..=55 {
            write(&root.join(format!("f{i}.bin")), i);
        }
        let mut t = scan(&root);
        let total = t.node(t.root()).size;
        t.forget(&root.join("f1.bin"), 1);
        let rest = t.node(*t.children(t.root()).last().unwrap());
        assert_eq!((rest.len, rest.size), (4, 14));
        assert_eq!(t.node(t.root()).size, total - 1);
        // uno de los 50 se quita como nodo
        t.forget(&root.join("f55.bin"), 55);
        assert_eq!(t.node(t.root()).size, total - 56);
        assert_eq!(t.children(t.root()).len(), 50);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn forget_gone_file_is_noop() {
        let root = temp_dir("tr-forget-gone");
        write(&root.join("a.bin"), 10);
        write(&root.join("b.bin"), 20);
        let mut t = scan(&root);
        let id = t.find(&root.join("a.bin")).unwrap();
        t.remove(id);
        let before: Vec<u64> = (0..3).map(|i| t.node(i).size).collect();
        t.forget(&root.join("a.bin"), 10);
        t.forget(&root.join("nunca.bin"), 5);
        assert_eq!((0..3).map(|i| t.node(i).size).collect::<Vec<_>>(), before);
        assert_eq!(t.node(t.root()).size, 20);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn odd_names_marked() {
        use std::os::unix::ffi::OsStrExt;
        let root = temp_dir("tr-odd");
        let bad = std::ffi::OsStr::from_bytes(b"bad\xff");
        fs::write(root.join(bad), b"abc").unwrap();
        fs::create_dir_all(root.join(std::ffi::OsStr::from_bytes(b"dir\xfe"))).unwrap();
        write(&root.join("ok.bin"), 1);
        let t = scan(&root);
        let kids = t.children(t.root());
        assert_eq!(kids.len(), 3);
        for k in kids {
            let n = t.node(k);
            match &*n.name {
                "ok.bin" => assert!(!n.odd),
                x if x.starts_with("bad") => assert!(n.odd && n.kind == Kind::File),
                _ => assert!(n.odd && n.kind == Kind::Denied && n.len == 0),
            }
        }
        assert_eq!(t.node(t.root()).size, 4);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn graft_nested_dirs() {
        let root = temp_dir("tr-graft-nested");
        write(&root.join("z.bin"), 1);
        write(&root.join("a/old.bin"), 5);
        let mut t = scan(&root);
        let a = t.find(&root.join("a")).unwrap();
        fs::remove_file(root.join("a/old.bin")).unwrap();
        write(&root.join("a/x.bin"), 100);
        write(&root.join("a/b/y.bin"), 40);
        write(&root.join("a/b/c/z.bin"), 7);
        t.graft(a, scan(&root.join("a")));
        let (b, c) = (t.find(&root.join("a/b")).unwrap(), t.find(&root.join("a/b/c")).unwrap());
        assert_eq!(names(&t, a), ["x.bin", "b"]);
        assert_eq!(names(&t, b), ["y.bin", "c"]);
        assert_eq!(names(&t, c), ["z.bin"]);
        assert_eq!((t.node(a).size, t.node(b).size, t.node(c).size), (147, 47, 7));
        assert_eq!(t.node(t.root()).size, 148);
        let z = t.children(c)[0];
        assert_eq!(t.path(z), root.join("a/b/c/z.bin"));
        assert_eq!(t.crumbs(z), [0, a, b, c, z]);
        assert_eq!(t.node(c).parent, b);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn node_is_compact() {
        assert!(size_of::<Node>() <= 48);
    }
}
