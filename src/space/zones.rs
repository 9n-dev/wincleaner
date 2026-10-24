//! Zonas que Espacio nunca borra: sistema, otros usuarios, carpetas conocidas y juegos.
//! Todo es léxico (vía `paths`); lo que no se entiende se trata como protegido.

use crate::paths::{is_inside, is_valid, same};

/// Por qué una ruta no se puede borrar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// Zona del sistema o ruta que no se entiende.
    Protected,
    /// Perfil, carpeta conocida, `AppData` o WinCleaner: se conserva la carpeta, no su contenido.
    Known,
    /// Archivos del sistema en la raíz de una unidad.
    System,
    /// Carpeta de juego.
    Game,
}

#[derive(Clone)]
pub struct Zones {
    trees: Vec<String>,
    profile: String,
    known: Vec<(String, &'static str)>,
    exe: String,
    exe_dir: String,
    games: Vec<String>,
}

/// Carpetas protegidas en la raíz de cada unidad.
const ROOT_TREES: [&str; 9] = ["Users", "Windows", "Program Files", "Program Files (x86)", "ProgramData",
    "$Recycle.Bin", "System Volume Information", "Recovery", "PerfLogs"];

fn comps(p: &str) -> Vec<&str> {
    p.split(['\\', '/']).filter(|c| !c.is_empty()).collect()
}

/// Formas que Win32 resuelve a otra ruta (puntos o espacios finales, nombres cortos 8.3) o que no son
/// nombres de archivo: no se pueden comparar en léxico, así que se rechazan.
fn aliased(p: &str) -> bool {
    comps(p).iter().any(|c| {
        c.ends_with(['.', ' ']) || c.contains(['*', '?', '"', '<', '>', '|'])
            || c.as_bytes().windows(2).any(|w| w[0] == b'~' && w[1].is_ascii_digit())
    })
}

impl Zones {
    pub fn new(drives: &[String], system: &[String], profile: &str, known: &[(String, &'static str)], exe: &str,
        games: &[String]) -> Zones {
        let mut trees = system.to_vec();
        for d in drives {
            trees.extend(ROOT_TREES.iter().map(|t| format!(r"{}\{t}", d.trim_end_matches(['\\', '/']))));
        }
        let exe_dir = exe.rfind(['\\', '/']).map_or("", |i| &exe[..i]).to_string();
        // La excepción del perfil solo vale si cuelga de un árbol y no es un árbol ni lo contiene.
        let ok = trees.iter().any(|t| is_inside(profile, t))
            && !trees.iter().any(|t| same(profile, t) || is_inside(t, profile));
        let profile = if ok { profile } else { "" };
        Zones { trees, profile: profile.into(), known: known.to_vec(), exe: exe.into(), exe_dir,
            games: games.to_vec() }
    }

    pub fn with_games(&self, games: &[String]) -> Zones {
        Zones { games: games.to_vec(), ..self.clone() }
    }

    /// Igual o dentro de `root`.
    fn at_or_in(path: &str, root: &str) -> bool {
        same(path, root) || is_inside(path, root)
    }

    /// En un árbol protegido; lo que cuelga del perfil no cuenta como dentro de `Users`.
    fn in_tree(&self, path: &str) -> bool {
        !Self::at_or_in(path, &self.profile) && self.trees.iter().any(|t| Self::at_or_in(path, t))
    }

    /// Igual o dentro de una carpeta de juego (no basta con contener una).
    pub fn in_game(&self, path: &str) -> bool {
        self.games.iter().any(|g| Self::at_or_in(path, g))
    }

    pub fn check(&self, path: &str) -> Option<Block> {
        if !is_valid(path) || aliased(path) {
            return Some(Block::Protected);
        }
        let c = comps(path);
        if c.len() == 1 {
            return Some(Block::Protected);
        }
        if c.len() == 2 {
            let n = c[1].to_lowercase();
            if ["pagefile.sys", "hiberfil.sys", "swapfile.sys"].contains(&n.as_str()) || n.starts_with("dumpstack.log") {
                return Some(Block::System);
            }
        }
        if self.games.iter().any(|g| Self::at_or_in(path, g) || is_inside(g, path)) {
            return Some(Block::Game);
        }
        if self.in_tree(path) {
            return Some(Block::Protected);
        }
        let named = [&self.profile, &self.exe_dir, &self.exe].into_iter().chain(self.known.iter().map(|k| &k.0));
        for k in named {
            if same(path, k) || is_inside(k, path) {
                return Some(Block::Known);
            }
        }
        None
    }

    /// Duplicados no entra aquí: árboles protegidos, juegos y `AppData`. Las raíces de unidad sí se recorren.
    pub fn skip_walk(&self, dir: &str) -> bool {
        if !is_valid(dir) || aliased(dir) {
            return true;
        }
        self.in_tree(dir) || self.in_game(dir)
            || self.known.iter().any(|(k, l)| *l == "AppData" && Self::at_or_in(dir, k))
    }

    pub fn label(&self, path: &str) -> Option<&'static str> {
        if let Some((_, l)) = self.known.iter().find(|(k, _)| same(path, k)) {
            return Some(l);
        }
        if !self.trees.iter().any(|t| same(path, t)) {
            return None;
        }
        match comps(path).last()?.to_lowercase().as_str() {
            "users" => Some("Usuarios"),
            "program files" => Some("Archivos de programa"),
            "program files (x86)" => Some("Archivos de programa (x86)"),
            "windows" => Some("Windows"),
            "programdata" => Some("ProgramData"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z() -> Zones {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let u = r"C:\Users\ana";
        let known: Vec<(String, &str)> = [("Documents", "Documentos"), ("Desktop", "Escritorio"), ("Downloads", "Descargas"),
            ("Pictures", "Imágenes"), ("Music", "Música"), ("Videos", "Vídeos"), ("AppData", "AppData")]
            .iter().map(|(d, l)| (format!(r"{u}\{d}"), *l)).collect();
        Zones::new(&s(&[r"C:\", r"D:\"]), &s(&[r"C:\Windows", r"C:\Program Files", r"C:\Program Files (x86)", r"C:\ProgramData"]),
            u, &known, r"C:\Users\ana\Downloads\tools\wincleaner.exe", &s(&[r"D:\SteamLibrary\steamapps\common\Baldurs Gate 3"]))
    }

    fn all(paths: &[&str], want: Option<Block>) {
        let z = z();
        for p in paths {
            assert_eq!(z.check(p), want, "{p}");
        }
    }

    #[test]
    fn drive_roots_protected() {
        all(&[r"C:\", "c:", r"D:\", "D:/"], Some(Block::Protected));
    }

    #[test]
    fn system_trees_protected() {
        all(&[r"C:\Windows", r"c:\WINDOWS\temp\x.log", "C:/Windows/notepad.exe", r"C:\Program Files\App",
            r"C:\Program Files (x86)\Steam", r"C:\ProgramData\x", r"D:\$Recycle.Bin\x", r"D:\System Volume Information",
            r"C:\Recovery", r"C:\PerfLogs\a"], Some(Block::Protected));
    }

    #[test]
    fn odd_forms_fail_closed() {
        all(&[r"C:\Users\ana\Videos\..\..\..\Windows", r"\\?\C:\Users\ana\Videos\x.mov", r"\\server\share\x", r"Videos\x",
            r"C:\Users\ana\Videos\x.mov:zone", "", r"C:\Windows.", r"C:\Windows ", r"C:\Program Files \x",
            r"C:\PROGRA~1\App", r"C:\WINDOW~1", r"C:\Users\ana\Videos\a*.mov"], Some(Block::Protected));
    }

    #[test]
    fn hostile_profile_ignored() {
        for prof in [r"C:\", r"C:\Users"] {
            let z = Zones::new(&[r"C:\".to_string()], &[], prof, &[], r"C:\x\w.exe", &[]);
            for p in [r"C:\Windows\x", r"C:\Users\pepe\a"] {
                assert_eq!(z.check(p), Some(Block::Protected), "{prof} {p}");
            }
        }
    }

    #[test]
    fn root_system_files() {
        all(&[r"C:\pagefile.sys", r"C:\HIBERFIL.SYS", r"D:\swapfile.sys", r"C:\DumpStack.log.tmp"], Some(Block::System));
        all(&[r"C:\Users\ana\pagefile.sys", r"D:\notas.txt"], None);
    }

    #[test]
    fn other_users_protected() {
        all(&[r"C:\Users", r"C:\Users\Public\x", r"C:\Users\pepe\Documents\a"], Some(Block::Protected));
    }

    #[test]
    fn known_folders_only_contents() {
        all(&[r"C:\Users\ana", r"C:\Users\ana\Videos", r"c:\users\ANA\videos\", r"C:\Users\ana\AppData"], Some(Block::Known));
        all(&[r"C:\Users\ana\Videos\boda.mov", r"C:\Users\ana\AppData\Local\Temp\x", r"C:\Users\ana\source"], None);
    }

    #[test]
    fn own_exe_protected() {
        all(&[r"C:\Users\ana\Downloads\tools", r"C:\Users\ana\Downloads\tools\wincleaner.exe", r"C:\Users\ana\Downloads"],
            Some(Block::Known));
        all(&[r"C:\Users\ana\Downloads\tools\otro.txt"], None);
    }

    #[test]
    fn games_protected_with_ancestors() {
        all(&[r"D:\SteamLibrary\steamapps\common\Baldurs Gate 3", r"D:\SteamLibrary\steamapps\common\Baldurs Gate 3\bin\x.dll",
            r"D:\SteamLibrary"], Some(Block::Game));
        all(&[r"D:\SteamLibrary\otra.txt", r"D:\Juegos viejos"], None);
        assert!(z().in_game(r"D:\SteamLibrary\steamapps\common\Baldurs Gate 3\bin"));
        assert!(!z().in_game(r"D:\SteamLibrary"));
    }

    #[test]
    fn skip_walk_rules() {
        let z = z();
        for p in [r"C:\Windows", r"C:\Windows.", r"C:\PROGRA~1\App", r"C:\Users\ana\AppData", r"C:\Users\pepe", r"D:\SteamLibrary\steamapps\common\Baldurs Gate 3"] {
            assert!(z.skip_walk(p), "{p}");
        }
        for p in [r"C:\", r"D:\", r"C:\Users\ana\Documents", r"C:\Users\ana\source"] {
            assert!(!z.skip_walk(p), "{p}");
        }
    }

    #[test]
    fn labels() {
        let z = z();
        assert_eq!(z.label(r"C:\Users\ana\Videos"), Some("Vídeos"));
        assert_eq!(z.label(r"c:\program files"), Some("Archivos de programa"));
        assert_eq!(z.label(r"C:\Users"), Some("Usuarios"));
    }

    #[test]
    fn with_games_replaces() {
        let z = z().with_games(&[r"E:\Juegos\X".to_string()]);
        assert_eq!(z.check(r"E:\Juegos\X\a"), Some(Block::Game));
        assert_eq!(z.check(r"D:\SteamLibrary\steamapps\common\Baldurs Gate 3"), None);
    }
}
