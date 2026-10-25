//! Instaladores viejos: `.exe`, `.msi`, `.msix`… e `.iso` sueltos en Descargas y Escritorio, y
//! si su programa ya está instalado (por el nombre del archivo o su `ProductName`).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::categories::is_link;

/// A partir de aquí un instalador sale marcado.
pub const OLD: Duration = Duration::from_secs(30 * 86_400);

pub struct Installer {
    pub path: PathBuf,
    pub size: u64,
    /// Lo más reciente entre creación y modificación.
    pub when: SystemTime,
    pub place: &'static str,
    pub installed: bool,
}

const EXTS: [&str; 6] = ["exe", "msi", "msix", "msixbundle", "appx", "iso"];

/// Fragmentos de nombre que no dicen de qué programa es.
const NOISE: [&str; 21] = [
    "setup", "installer", "install", "windows", "win", "win32", "win64", "x64", "x86", "amd64", "arm64", "full", "offline",
    "online", "web", "latest", "release", "final", "spanish", "es", "en",
];

fn ext(name: &str) -> String {
    Path::new(name).extension().map_or_else(String::new, |e| e.to_string_lossy().to_lowercase())
}

pub fn is_installer(name: &str) -> bool {
    EXTS.contains(&ext(name).as_str())
}

/// Palabras en minúsculas: se parte por lo que no es alfanumérico y entre minúscula y mayúscula.
fn words(s: &str) -> Vec<String> {
    let mut v = vec![String::new()];
    let mut prev = ' ';
    for c in s.chars() {
        if !c.is_alphanumeric() || (prev.is_lowercase() && c.is_uppercase()) {
            v.push(String::new());
        }
        if c.is_alphanumeric()
            && let Some(w) = v.last_mut()
        {
            w.extend(c.to_lowercase());
        }
        prev = c;
    }
    v.retain(|w| !w.is_empty());
    v
}

/// Las palabras que nombran el programa: sin versiones, arquitecturas ni «setup».
fn key_words(s: &str) -> Vec<String> {
    let version = |w: &str| w.strip_prefix('v').is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
    words(s).into_iter().filter(|w| !w.starts_with(|c: char| c.is_ascii_digit()) && !version(w) && !NOISE.contains(&w.as_str())).collect()
}

fn stem(file_name: &str) -> &str {
    Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or(file_name)
}

/// «OBS-Studio-30.2-Windows-Installer.exe» → «obs studio».
pub fn name_key(file_name: &str) -> String {
    key_words(stem(file_name)).join(" ")
}

/// El programa del instalador sale en `programs`: la clave del nombre del archivo (si tiene
/// 3 letras o más) o el `ProductName` aparecen como palabras enteras. Una `.iso` nunca.
pub fn installed(file_name: &str, product: Option<&str>, programs: &[String]) -> bool {
    if ext(file_name) == "iso" {
        return false;
    }
    let letters = |k: &[String]| k.iter().flat_map(|w| w.chars()).filter(|c| c.is_alphabetic()).count() >= 3;
    let keys: Vec<Vec<String>> = [Some(key_words(stem(file_name))), product.map(key_words)].into_iter().flatten().filter(|k| letters(k)).collect();
    programs.iter().map(|p| words(p)).any(|p| keys.iter().any(|k| p.windows(k.len()).any(|w| w == k.as_slice())))
}

pub fn is_old(when: SystemTime, now: SystemTime) -> bool {
    now.duration_since(when).is_ok_and(|d| d > OLD)
}

/// Los instaladores sueltos en cada carpeta de `dirs` (sin entrar en subcarpetas ni seguir
/// enlaces), de mayor a menor. `product_of` lee el `ProductName` de un archivo.
pub fn list(dirs: &[(PathBuf, &'static str)], programs: &[String], product_of: &dyn Fn(&Path) -> Option<String>) -> Vec<Installer> {
    let mut v = Vec::new();
    for (dir, place) in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue }; // DirEntry::metadata no sigue enlaces
            let name = e.file_name().to_string_lossy().into_owned();
            if is_link(&md) || !md.is_file() || !is_installer(&name) {
                continue;
            }
            let path = e.path();
            let when = md.created().into_iter().chain(md.modified()).max().unwrap_or(SystemTime::UNIX_EPOCH);
            let installed = installed(&name, product_of(&path).as_deref(), programs);
            v.push(Installer { path, size: md.len(), when, place, installed });
        }
    }
    v.sort_by_key(|i| std::cmp::Reverse(i.size));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    const FILES: [&str; 5] = ["OBS-Studio-30.2-Windows-Installer.exe", "VisualStudioSetup.exe", "python-3.12.6-amd64.exe", "SteamSetup.exe", "NVIDIA_app_v11.0.exe"];

    #[test]
    fn name_keys() {
        let keys = FILES.map(name_key);
        assert_eq!(keys, ["obs studio", "visual studio", "python", "steam", "nvidia app"]);
    }

    #[test]
    fn installed_hints() {
        let programs: Vec<String> =
            ["OBS Studio", "Microsoft Visual Studio Installer", "Python 3.12.6 (64-bit)", "Steam", "NVIDIA app"].map(String::from).to_vec();
        for f in FILES {
            assert!(installed(f, None, &programs), "{f}");
        }
        for f in ["Win11_24H2_Spanish_x64.iso", "herramienta-rara.exe", "a.exe", "steamy.exe"] {
            assert!(!installed(f, None, &programs), "{f}");
        }
        assert!(installed("setup.exe", Some("OBS Studio"), &programs));
        assert!(!installed("setup.exe", None, &programs));
    }

    #[test]
    fn is_installer_ext() {
        assert!(is_installer("X.MSI"));
        assert!(is_installer("a.msixbundle"));
        assert!(!is_installer("a.zip"));
        assert!(!is_installer("a.exe.txt"));
    }

    #[test]
    fn is_old_threshold() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 86_400);
        assert!(is_old(now - Duration::from_secs(31 * 86_400), now));
        assert!(!is_old(now - Duration::from_secs(29 * 86_400), now));
        // Con fecha en el futuro (reloj cambiado), no es viejo.
        assert!(!is_old(now + Duration::from_secs(86_400), now));
    }

    #[test]
    fn list_only_top_level() {
        let d = temp_dir("installers-list");
        std::fs::write(d.join("a.exe"), [0u8; 10]).unwrap();
        std::fs::write(d.join("b.msi"), [0u8; 20]).unwrap();
        std::fs::write(d.join("c.pdf"), [0u8; 30]).unwrap();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub").join("d.iso"), [0u8; 40]).unwrap();
        let programs = vec!["Steam".to_string()];
        let v = list(&[(d.clone(), "Descargas")], &programs, &|p| p.ends_with("a.exe").then(|| "Steam".into()));
        let got: Vec<_> = v.iter().map(|i| (i.path.file_name().unwrap().to_str().unwrap(), i.size, i.place, i.installed)).collect();
        // Por tamaño, de mayor a menor; `a.exe` dice ser Steam en su `ProductName`.
        assert_eq!(got, [("b.msi", 20, "Descargas", false), ("a.exe", 10, "Descargas", true)]);
    }

    #[cfg(unix)]
    #[test]
    fn list_skips_links() {
        let d = temp_dir("installers-links");
        std::fs::write(d.join("real.exe"), [0u8; 10]).unwrap();
        std::os::unix::fs::symlink(d.join("real.exe"), d.join("link.exe")).unwrap();
        let v = list(&[(d, "Escritorio")], &[], &|_| None);
        assert_eq!(v.len(), 1);
        assert!(v[0].path.ends_with("real.exe"));
    }
}
