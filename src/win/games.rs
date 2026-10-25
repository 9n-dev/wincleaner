//! Juegos instalados: Steam, Epic y Xbox desde sus archivos, y los demás desde `Uninstall`.
//! Un archivo que no se puede leer se salta.

use std::path::{Path, PathBuf};

use wincleaner::programs::Hive;
use wincleaner::space::games::{self, Game};

use super::reg;

pub fn list() -> Vec<Game> {
    let mut v = steam();
    v.extend(epic());
    v.extend(xbox());
    v.extend(reg::uninstall_entries().iter().filter_map(games::uninstall_game));
    games::dedupe(v)
}

/// Texto del archivo; un byte que no es UTF-8 no lo descarta entero.
fn read(p: &Path) -> Option<String> {
    std::fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Las entradas de `dir` (sin recorrer subcarpetas); vacío si no se puede leer.
fn entries(dir: &Path) -> impl Iterator<Item = (PathBuf, String, std::fs::FileType)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.path(), e.file_name().into_string().ok()?, e.file_type().ok()?)))
}

fn steam() -> Vec<Game> {
    let Some(dir) = reg::read_str(Hive::User, r"Software\Valve\Steam", "SteamPath") else { return Vec::new() };
    let vdf = read(&Path::new(&dir).join(r"steamapps\libraryfolders.vdf")).unwrap_or_default();
    games::steam_libraries(&vdf, &dir)
        .iter()
        .flat_map(|lib| {
            entries(&Path::new(lib).join("steamapps"))
                .filter(|(_, n, t)| t.is_file() && n.starts_with("appmanifest_") && n.ends_with(".acf"))
                .filter_map(|(p, _, _)| games::steam_game(&read(&p)?, lib))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn epic() -> Vec<Game> {
    let Some(data) = std::env::var_os("ProgramData") else { return Vec::new() };
    entries(&Path::new(&data).join(r"Epic\EpicGamesLauncher\Data\Manifests"))
        .filter(|(_, n, t)| t.is_file() && n.ends_with(".item"))
        .filter_map(|(p, _, _)| games::epic_game(&read(&p)?))
        .collect()
}

/// `X:\XboxGames\*` en cada unidad fija; los enlaces no cuentan.
fn xbox() -> Vec<Game> {
    super::fixed_drives()
        .iter()
        .flat_map(|d| entries(&Path::new(&d.root).join("XboxGames")).collect::<Vec<_>>())
        .filter(|(_, _, t)| t.is_dir())
        .map(|(p, _, _)| games::xbox_game(&p.to_string_lossy()))
        .collect()
}
