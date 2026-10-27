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

/// El motivo fino de un bloqueo; `block` lo resume en el `Block` que decide el borrado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lock {
    Unclear,
    Drive,
    Windows,
    SystemFile,
    Programs,
    Bin,
    Users,
    OtherUser,
    Known(&'static str),
    Profile,
    HoldsKnown(&'static str),
    Me,
    Game,
    HoldsGames,
}

impl Lock {
    pub fn block(self) -> Block {
        match self {
            Lock::Unclear | Lock::Drive | Lock::Windows | Lock::Programs | Lock::Bin | Lock::Users
                | Lock::OtherUser => Block::Protected,
            Lock::SystemFile => Block::System,
            Lock::Known(_) | Lock::Profile | Lock::HoldsKnown(_) | Lock::Me => Block::Known,
            Lock::Game | Lock::HoldsGames => Block::Game,
        }
    }
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
const ROOT_TREES: [&str; 13] = ["Users", "Windows", "Program Files", "Program Files (x86)", "ProgramData",
    "$Recycle.Bin", "System Volume Information", "Recovery", "PerfLogs", "Boot", "$WinREAgent", "$SysReset", "Config.Msi"];

/// Archivos del sistema en la raíz de cada unidad (además de `DumpStack.log*`).
const ROOT_FILES: [&str; 6] = ["pagefile.sys", "hiberfil.sys", "swapfile.sys", "bootmgr", "bootnxt", "bootsect.bak"];

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
        self.lock(path).map(Lock::block)
    }

    /// El motivo fino por el que `path` no se borra; `check` es `lock(..).map(Lock::block)`.
    pub fn lock(&self, path: &str) -> Option<Lock> {
        if !is_valid(path) || aliased(path) {
            return Some(Lock::Unclear);
        }
        let c = comps(path);
        if c.len() == 1 {
            return Some(Lock::Drive);
        }
        if c.len() == 2 {
            let n = c[1].to_lowercase();
            if ROOT_FILES.contains(&n.as_str()) || n.starts_with("dumpstack.log") {
                return Some(Lock::SystemFile);
            }
        }
        if self.games.iter().any(|g| Self::at_or_in(path, g)) {
            return Some(Lock::Game);
        }
        if self.games.iter().any(|g| is_inside(g, path)) {
            return Some(Lock::HoldsGames);
        }
        if !Self::at_or_in(path, &self.profile) && let Some(t) = self.trees.iter().find(|t| Self::at_or_in(path, t)) {
            return Some(match comps(t).last().map(|n| n.to_lowercase()).as_deref() {
                Some("program files" | "program files (x86)") => Lock::Programs,
                Some("$recycle.bin") => Lock::Bin,
                Some("users") if same(path, t) => Lock::Users,
                Some("users") => Lock::OtherUser,
                _ => Lock::Windows,
            });
        }
        if let Some((_, l)) = self.known.iter().find(|(k, _)| same(path, k)) {
            return Some(Lock::Known(l));
        }
        if same(path, &self.profile) {
            return Some(Lock::Profile);
        }
        if [&self.exe, &self.exe_dir].into_iter().any(|k| same(path, k) || is_inside(k, path)) {
            return Some(Lock::Me);
        }
        if let Some((_, l)) = self.known.iter().find(|(k, _)| is_inside(k, path)) {
            return Some(Lock::HoldsKnown(l));
        }
        if is_inside(&self.profile, path) {
            return Some(Lock::HoldsKnown("tu perfil"));
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

    /// Es el perfil, una carpeta conocida o algo que las contiene: no puede ser la carpeta de un juego.
    pub fn holds_user(&self, path: &str) -> bool {
        [&self.profile].into_iter().chain(self.known.iter().map(|k| &k.0)).any(|k| !k.is_empty() && Self::at_or_in(k, path))
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
        all(&[r"C:\bootmgr", r"C:\BOOTNXT", r"D:\bootsect.bak"], Some(Block::System));
    }

    #[test]
    fn root_boot_trees_protected() {
        all(&[r"C:\Boot", r"C:\Boot\BCD", r"D:\$WinREAgent\x", r"C:\$SysReset", r"C:\Config.Msi\a.rbf"], Some(Block::Protected));
        all(&[r"C:\Users\ana\Boot"], None);
        let z = z();
        for p in [r"C:\Boot", r"C:\$WinREAgent", r"D:\$SysReset", r"C:\Config.Msi"] {
            assert!(z.skip_walk(p), "{p}");
        }
    }

    #[test]
    fn holds_user_folders() {
        let z = z();
        for p in [r"C:\Users\ana", r"C:\Users", r"C:\", r"c:\users\ana\documents", r"C:\Users\ana\AppData"] {
            assert!(z.holds_user(p), "{p}");
        }
        for p in [r"C:\Users\ana\Documents\Juego", r"D:\Juegos", r"C:\Program Files\Epic Games\Fortnite", r"C:\Users\pepe"] {
            assert!(!z.holds_user(p), "{p}");
        }
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

    #[test]
    fn lock_kinds() {
        let z = z();
        let l = |p| z.lock(p);
        assert_eq!(l(r"C:\Windows\x"), Some(Lock::Windows));
        assert_eq!(l(r"C:\ProgramData"), Some(Lock::Windows));
        assert_eq!(l(r"C:\Boot\BCD"), Some(Lock::Windows));
        assert_eq!(l(r"C:\Program Files\Adobe"), Some(Lock::Programs));
        assert_eq!(l(r"C:\Program Files (x86)"), Some(Lock::Programs));
        assert_eq!(l(r"D:\$Recycle.Bin\x"), Some(Lock::Bin));
        assert_eq!(l(r"C:\Users"), Some(Lock::Users));
        assert_eq!(l(r"C:\Users\pepe\a"), Some(Lock::OtherUser));
        assert_eq!(l(r"C:\Users\Public"), Some(Lock::OtherUser));
        assert_eq!(l(r"C:\hiberfil.sys"), Some(Lock::SystemFile));
        assert_eq!(l(r"C:\"), Some(Lock::Drive));
        assert_eq!(l(r"C:\PROGRA~1\App"), Some(Lock::Unclear));
        assert_eq!(l(r"C:\Users\ana\Videos"), Some(Lock::Known("Vídeos")));
        assert_eq!(l(r"C:\Users\ana"), Some(Lock::Profile));
        assert_eq!(l(r"C:\Users\ana\Downloads"), Some(Lock::Known("Descargas")));
        assert_eq!(l(r"C:\Users\ana\Downloads\tools"), Some(Lock::Me));
        assert_eq!(l(r"C:\Users\ana\Downloads\tools\wincleaner.exe"), Some(Lock::Me));
        assert_eq!(l(r"D:\SteamLibrary\steamapps\common\Baldurs Gate 3\bin"), Some(Lock::Game));
        assert_eq!(l(r"D:\SteamLibrary"), Some(Lock::HoldsGames));
        assert_eq!(l(r"C:\Users\ana\Videos\boda.mov"), None);
    }

    #[test]
    fn holds_known_folder() {
        // Una carpeta que contiene una conocida sin serlo (OneDrive con Documentos dentro).
        let known = vec![(r"C:\Users\ana\OneDrive\Documents".to_string(), "Documentos")];
        let z = Zones::new(&[r"C:\".into()], &[r"C:\Windows".into()], r"C:\Users\ana", &known, r"E:\w.exe", &[]);
        assert_eq!(z.lock(r"C:\Users\ana\OneDrive"), Some(Lock::HoldsKnown("Documentos")));
    }

    #[test]
    fn check_is_lock_block() {
        let z = z();
        for p in [r"C:\", r"C:\Windows", r"C:\pagefile.sys", r"C:\Users\ana", r"C:\Users\ana\Videos\x",
            r"D:\SteamLibrary", r"C:\Users\pepe", r"C:\PROGRA~1", r"C:\Users\ana\Downloads\tools\wincleaner.exe"] {
            assert_eq!(z.check(p), z.lock(p).map(Lock::block), "{p}");
        }
    }
}
