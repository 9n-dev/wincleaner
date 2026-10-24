//! Reglas de juegos: Steam, Epic, entradas Uninstall y Xbox (sin Windows).

use crate::programs::{self, Hive, RawEntry};
use crate::space::parse::{self, Json, Vdf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launcher {
    Steam,
    Epic,
    Gog,
    Xbox,
    BattleNet,
    Ea,
    Ubisoft,
}

impl Launcher {
    pub fn name(&self) -> &'static str {
        match self {
            Launcher::Steam => "Steam",
            Launcher::Epic => "Epic Games",
            Launcher::Gog => "GOG",
            Launcher::Xbox => "Xbox",
            Launcher::BattleNet => "Battle.net",
            Launcher::Ea => "EA app",
            Launcher::Ubisoft => "Ubisoft Connect",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Uninstall {
    Url(String),
    Program { hive: Hive, key: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Game {
    pub name: String,
    pub launcher: Launcher,
    pub dir: Option<String>,
    pub size: Option<u64>,
    /// Segundos desde 1970.
    pub last_played: Option<u64>,
    pub uninstall: Uninstall,
}

fn num(v: Option<&Vdf>) -> Option<u64> {
    v?.str()?.trim().parse().ok()
}

/// Bibliotecas de Steam: las `path` del VDF más `steam_dir` si no está (sin mirar mayúsculas).
pub fn steam_libraries(vdf_text: &str, steam_dir: &str) -> Vec<String> {
    let fix = |s: &str| s.replace('/', "\\");
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: String| {
        if !p.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&p)) {
            out.push(p);
        }
    };
    if let Some(Vdf::Map(top)) = parse::vdf(vdf_text).as_ref().and_then(|v| v.get("libraryfolders")) {
        for (_, e) in top {
            if let Some(p) = e.get("path").and_then(Vdf::str) {
                add(fix(p));
            }
        }
    }
    add(fix(steam_dir));
    out
}

pub fn steam_game(acf_text: &str, library: &str) -> Option<Game> {
    let v = parse::vdf(acf_text)?;
    let s = v.get("AppState")?;
    if num(s.get("StateFlags"))? & 4 == 0 {
        return None;
    }
    let appid = s.get("appid")?.str()?.trim();
    if appid.is_empty() || !appid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let installdir = s.get("installdir")?.str()?;
    Some(Game {
        name: s.get("name")?.str()?.to_string(),
        launcher: Launcher::Steam,
        dir: Some(format!("{library}\\steamapps\\common\\{installdir}")),
        size: num(s.get("SizeOnDisk")),
        last_played: num(s.get("LastPlayed")).filter(|&t| t != 0),
        uninstall: Uninstall::Url(format!("steam://uninstall/{appid}")),
    })
}

pub fn epic_game(item_text: &str) -> Option<Game> {
    let j = parse::json(item_text)?;
    if j.get("bIsIncompleteInstall") == Some(&Json::Bool(true)) {
        return None;
    }
    let s = |k| match j.get(k) {
        Some(Json::Str(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    let size = match j.get("InstallSize") {
        Some(&Json::Num(n)) if n.is_finite() && n >= 0.0 => Some(n as u64),
        _ => None,
    };
    Some(Game {
        name: s("DisplayName")?,
        launcher: Launcher::Epic,
        dir: s("InstallLocation"),
        size,
        last_played: None,
        uninstall: Uninstall::Url("com.epicgames.launcher://".into()),
    })
}

const LAUNCHERS: [&str; 6] = ["Battle.net", "EA app", "Origin", "Ubisoft Connect", "Uplay", "GOG GALAXY"];

pub fn uninstall_game(e: &RawEntry) -> Option<Game> {
    if !programs::visible(e) || e.key.starts_with("Steam App ") {
        return None;
    }
    let name = e.display_name.as_deref()?;
    if LAUNCHERS.iter().any(|l| name.eq_ignore_ascii_case(l)) {
        return None;
    }
    let publisher = e.publisher.as_deref().unwrap_or("");
    let launcher = if publisher == "GOG.com" {
        Launcher::Gog
    } else if publisher.contains("Blizzard") && e.uninstall_string.as_deref().is_some_and(|u| u.contains("Battle.net")) {
        Launcher::BattleNet
    } else if publisher.contains("Electronic Arts") {
        Launcher::Ea
    } else if publisher.contains("Ubisoft") {
        Launcher::Ubisoft
    } else {
        return None;
    };
    Some(Game {
        name: name.to_string(),
        launcher,
        dir: e.install_location.clone().filter(|d| !d.trim().is_empty()),
        size: e.estimated_size_kb.map(|k| u64::from(k) * 1024),
        last_played: None,
        uninstall: Uninstall::Program { hive: e.hive, key: e.key.clone() },
    })
}

pub fn xbox_game(dir: &str) -> Game {
    let name = dir.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(dir);
    Game {
        name: name.to_string(),
        launcher: Launcher::Xbox,
        dir: Some(dir.to_string()),
        size: None,
        last_played: None,
        uninstall: Uninstall::Url("ms-settings:appsfeatures".into()),
    }
}

/// Quita las repetidas por `dir` (se queda la primera) y ordena de mayor a menor tamaño.
pub fn dedupe(v: Vec<Game>) -> Vec<Game> {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<Game> = v
        .into_iter()
        .filter(|g| match &g.dir {
            Some(d) => {
                let k = d.to_lowercase();
                !seen.contains(&k) && {
                    seen.push(k);
                    true
                }
            }
            None => true,
        })
        .collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.size));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs::RawEntry;
    use crate::space::parse::{json, vdf};

    const LIBS: &str = include_str!("../../tests/data/libraryfolders.vdf");
    const ACF: &str = include_str!("../../tests/data/appmanifest_1086940.acf");
    const ITEM: &str = include_str!("../../tests/data/fortnite.item");

    #[test]
    fn steam_libraries_from_real_vdf() {
        assert_eq!(
            steam_libraries(LIBS, "c:/program files (x86)/steam"),
            [r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]
        );
    }

    #[test]
    fn steam_game_from_real_acf() {
        let g = steam_game(ACF, r"D:\SteamLibrary").unwrap();
        assert_eq!(g.name, "Baldur's Gate 3");
        assert_eq!(g.launcher, Launcher::Steam);
        assert_eq!(g.dir.as_deref(), Some(r"D:\SteamLibrary\steamapps\common\Baldurs Gate 3"));
        assert_eq!(g.size, Some(126726299419));
        assert_eq!(g.last_played, Some(1729468800));
        assert_eq!(g.uninstall, Uninstall::Url("steam://uninstall/1086940".into()));
    }

    #[test]
    fn acf_missing_fields() {
        let sin = ACF.replace("\"SizeOnDisk\"\t\t\"126726299419\"", "");
        assert_eq!(steam_game(&sin, "D:").unwrap().size, None);
        let cero = ACF.replace("\"LastPlayed\"\t\t\"1729468800\"", "\"LastPlayed\"\t\t\"0\"");
        assert_eq!(steam_game(&cero, "D:").unwrap().last_played, None);
        let flags = ACF.replace("\"StateFlags\"\t\t\"4\"", "\"StateFlags\"\t\t\"2\"");
        assert!(steam_game(&flags, "D:").is_none());
        assert!(steam_game("basura {", "D:").is_none());
    }

    #[test]
    fn vdf_tolerates_garbage() {
        let v = vdf("\u{feff}\"a\" { \"b\" \"c\" // x\n }").unwrap();
        assert_eq!(v.get("A").and_then(|a| a.get("b")).and_then(Vdf::str), Some("c"));
        assert!(vdf("\"a\" { \"b\"").is_none());
        assert!(vdf("").is_none());
        assert!(vdf("}}}").is_none());
        assert!(vdf(&"\"a\" {".repeat(1000)).is_none());
    }

    #[test]
    fn epic_game_from_real_item() {
        let g = epic_game(ITEM).unwrap();
        assert_eq!(g.name, "Fortnite");
        assert_eq!(g.launcher, Launcher::Epic);
        assert_eq!(g.dir.as_deref(), Some(r"C:\Program Files\Epic Games\Fortnite"));
        assert_eq!(g.size, Some(36507222016));
        assert_eq!(g.uninstall, Uninstall::Url("com.epicgames.launcher://".into()));
    }

    #[test]
    fn epic_incomplete_skipped() {
        let t = ITEM.replace("\"bIsIncompleteInstall\": false", "\"bIsIncompleteInstall\": true");
        assert!(epic_game(&t).is_none());
    }

    #[test]
    fn json_escapes() {
        let j = json(r#"{"a":"\u00f1\ud83c\udf89\\"}"#).unwrap();
        assert!(matches!(j.get("a"), Some(Json::Str(s)) if s == "ñ🎉\\"));
        assert!(json(&"[".repeat(100)).is_none());
        let _ = json("1e400");
        assert!(json("{\"a\":}").is_none());
        assert!(json("[1,]").is_none());
    }

    fn entry(publisher: &str, name: &str, unins: &str) -> RawEntry {
        RawEntry {
            key: "k".into(),
            display_name: Some(name.into()),
            publisher: Some(publisher.into()),
            uninstall_string: Some(unins.into()),
            estimated_size_kb: Some(10),
            install_location: Some(r"C:\G".into()),
            ..Default::default()
        }
    }

    #[test]
    fn uninstall_rules() {
        let g = uninstall_game(&entry("GOG.com", "Hollow Knight", "u.exe")).unwrap();
        assert_eq!(g.launcher, Launcher::Gog);
        assert_eq!(g.size, Some(10240));
        assert_eq!(g.dir.as_deref(), Some(r"C:\G"));
        assert!(matches!(g.uninstall, Uninstall::Program { .. }));
        assert!(uninstall_game(&entry("GOG.com", "GOG GALAXY", "u")).is_none());
        let b = entry("Blizzard Entertainment", "Call of Duty®", r#""C:\Battle.net\Battle.net.exe" --uninstall"#);
        assert_eq!(uninstall_game(&b).unwrap().launcher, Launcher::BattleNet);
        assert!(uninstall_game(&entry("Blizzard Entertainment", "Battle.net", "Battle.net")).is_none());
        let s = RawEntry { key: "Steam App 1086940".into(), ..entry("Valve", "BG3", "u") };
        assert!(uninstall_game(&s).is_none());
        assert!(uninstall_game(&entry("Electronic Arts", "EA app", "u")).is_none());
        assert_eq!(uninstall_game(&entry("Electronic Arts", "FC 25", "u")).unwrap().launcher, Launcher::Ea);
        assert_eq!(uninstall_game(&entry("Ubisoft", "Far Cry", "u")).unwrap().launcher, Launcher::Ubisoft);
        assert!(uninstall_game(&entry("Ubisoft", "Ubisoft Connect", "u")).is_none());
        assert!(uninstall_game(&entry("Acme", "Foo", "u")).is_none());
        let oculto = RawEntry { system_component: true, ..entry("GOG.com", "X", "u") };
        assert!(uninstall_game(&oculto).is_none());
    }

    #[test]
    fn xbox_name_from_folder() {
        let g = xbox_game(r"C:\XboxGames\Halo");
        assert_eq!(g.name, "Halo");
        assert_eq!(g.launcher, Launcher::Xbox);
        assert_eq!(g.size, None);
    }

    #[test]
    fn dedupe_by_dir_and_sort() {
        let mk = |d: &str, s: Option<u64>| Game {
            name: d.into(),
            launcher: Launcher::Steam,
            dir: Some(d.into()),
            size: s,
            last_played: None,
            uninstall: Uninstall::Url(String::new()),
        };
        let v = dedupe(vec![mk(r"C:\a", Some(1)), mk(r"c:\A", Some(5)), mk(r"C:\b", Some(9)), mk(r"C:\c", None)]);
        let n: Vec<_> = v.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(n, [r"C:\b", r"C:\a", r"C:\c"]);
    }

    #[test]
    fn launcher_names() {
        assert_eq!(Launcher::Epic.name(), "Epic Games");
        assert_eq!(Launcher::Ubisoft.name(), "Ubisoft Connect");
    }
}
