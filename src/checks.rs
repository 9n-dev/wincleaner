//! Reglas puras de las comprobaciones de seguridad. Solo leen: no tocan el disco ni Windows.
//!
//! Contrato de firma: `signed == false` significa «verificado sin firma». Si la firma no se pudo
//! comprobar (`Signature::Unknown`), quien llama no debe llamar a estas reglas con `false`:
//! eso no es un hallazgo.
//!
//! Ante la duda (ruta que no se sabe leer, carpeta vacía) no hay hallazgo, para no dar sustos.

use crate::paths::{is_inside, is_valid, strip_verbatim};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Threat,
    Suspicious,
    Review,
}

/// Carpetas conocidas del usuario y del sistema; las que vengan vacías no cuentan.
#[derive(Clone, Debug, Default)]
pub struct Places {
    pub temp: Vec<String>,
    pub downloads: String,
    /// Roaming.
    pub app_data: String,
    pub local_app_data: String,
    pub public: String,
    pub system_root: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    DisableStartup(usize),
    Kill(String),
    OpenLocation(String),
    /// Solo el nombre de archivo, nunca la ruta.
    Search(String),
    OpenHosts,
    OpenWindowsSecurity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub title: String,
    pub path: String,
    pub why: String,
    pub actions: Vec<Action>,
}

/// Quita `\\?\` si lo lleva; las rutas de red se dejan tal cual (luego `paths` las rechaza).
fn norm(p: &str) -> &str {
    if p.starts_with(r"\\?\") { strip_verbatim(p).unwrap_or(p) } else { p }
}

fn file_name(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

/// `exe` está directamente dentro de `dir` (su carpeta padre es `dir`). Falso si algo no se entiende.
fn directly_in(exe: &str, dir: &str) -> bool {
    let (exe, dir) = (norm(exe), norm(dir));
    if !is_valid(exe) || !is_valid(dir) || !is_inside(exe, dir) {
        return false;
    }
    let parent = exe.rsplit_once(['\\', '/']).map_or("", |(p, _)| p);
    !is_inside(parent, dir)
}

fn inside(exe: &str, dir: &str) -> bool {
    is_inside(norm(exe), norm(dir))
}

/// Una carpeta de riesgo con una sola componente (`C:\`, `C:`) abarcaría media unidad: se descarta.
fn broad(dir: &str) -> bool {
    norm(dir).split(['\\', '/']).filter(|c| !c.is_empty()).count() < 2
}

pub fn risky_place(exe: &str, p: &Places) -> Option<&'static str> {
    let inside = |e: &str, d: &str| !broad(d) && inside(e, d);
    if p.temp.iter().any(|t| inside(exe, t)) {
        Some("una carpeta temporal")
    } else if inside(exe, &p.downloads) {
        Some("Descargas")
    } else if inside(exe, &p.public) {
        Some("la carpeta Public")
    } else if directly_in(exe, &p.app_data) || directly_in(exe, &p.local_app_data) {
        Some("AppData")
    } else {
        None
    }
}

pub fn startup_finding(
    i: usize,
    name: &str,
    exe: Option<&str>,
    exists: bool,
    signed: bool,
    p: &Places,
) -> Option<Finding> {
    let exe = exe.filter(|e| !e.trim().is_empty())?;
    let mk = |level, why: String| Finding {
        level,
        title: name.to_string(),
        path: exe.to_string(),
        why,
        actions: vec![Action::DisableStartup(i), Action::OpenLocation(exe.to_string())],
    };
    if !exists {
        return Some(mk(Level::Review, format!("{name} arranca con Windows pero su archivo ya no existe")));
    }
    match (risky_place(exe, p), signed) {
        (Some(w), false) => Some(mk(
            Level::Suspicious,
            format!("Arranca con Windows desde {w} y no tiene firma digital. Los programas de verdad no suelen instalarse ahí."),
        )),
        (Some(w), true) => Some(mk(Level::Review, format!("Arranca con Windows desde {w}. Tiene firma, pero no es un sitio habitual."))),
        (None, false) => Some(mk(Level::Review, "Arranca con Windows y no tiene firma digital.".into())),
        (None, true) => None,
    }
}

pub fn process_finding(exe: &str, signed: bool, p: &Places) -> Option<Finding> {
    if signed {
        return None; // los instaladores firmados se ejecutan desde Temp
    }
    let w = risky_place(exe, p)?;
    Some(Finding {
        level: Level::Suspicious,
        title: file_name(exe).to_string(),
        path: exe.to_string(),
        why: format!("Se está ejecutando desde {w} y no tiene firma digital."),
        actions: vec![
            Action::Kill(exe.to_string()),
            Action::OpenLocation(exe.to_string()),
            Action::Search(file_name(exe).to_string()),
        ],
    })
}

/// `signer: None` significa solo «verificado sin firma» (R16): si no se pudo comprobar, no se llama.
pub fn service_finding(name: &str, exe: &str, signer: Option<&str>) -> Option<Finding> {
    if signer.is_some() {
        return None; // con firma (de Microsoft o de otro) no hay hallazgo
    }
    let why = "Es un servicio sin firma digital. Revisa que lo reconozcas.".to_string();
    Some(Finding {
        level: Level::Review,
        title: name.to_string(),
        path: exe.to_string(),
        why,
        actions: vec![Action::OpenLocation(exe.to_string()), Action::Search(file_name(exe).to_string())],
    })
}

const SYSTEM32_ONLY: [&str; 14] = [
    "svchost", "lsass", "csrss", "winlogon", "services", "smss", "wininit", "spoolsv", "taskhostw", "dwm",
    "conhost", "sihost", "ctfmon", "rundll32",
];

pub fn fake_system_process(exe: &str, system_root: &str) -> Option<Finding> {
    let (e, root) = (norm(exe), norm(system_root));
    if !is_valid(e) || !is_valid(root) {
        return None; // solo cuenta una ruta bien formada
    }
    let name = file_name(e).to_ascii_lowercase();
    let sys32 = format!("{root}\\System32");
    let wow = format!("{root}\\SysWOW64");
    let ok = if name == "explorer.exe" {
        directly_in(e, root) || directly_in(e, &wow)
    } else if SYSTEM32_ONLY.iter().any(|n| name.strip_suffix(".exe") == Some(n)) {
        directly_in(e, &sys32) || directly_in(e, &wow)
    } else {
        return None;
    };
    if ok {
        return None;
    }
    Some(Finding {
        level: Level::Suspicious,
        title: file_name(e).to_string(),
        path: exe.to_string(),
        why: "Tiene el nombre de un proceso de Windows, pero no está en la carpeta donde debería. Los virus suelen disfrazarse así.".into(),
        actions: vec![
            Action::Kill(exe.to_string()),
            Action::OpenLocation(exe.to_string()),
            Action::OpenWindowsSecurity,
        ],
    })
}

pub fn hosts_entries(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut t = line.split_whitespace();
        let Some(ip) = t.next() else { continue };
        if ip.split('%').next().unwrap_or("").parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        for host in t {
            if host.eq_ignore_ascii_case("localhost") && (ip == "127.0.0.1" || ip == "::1") {
                continue;
            }
            out.push((ip.to_string(), host.to_string()));
        }
    }
    out
}

pub fn hosts_finding(entries: &[(String, String)]) -> Option<Finding> {
    if entries.is_empty() {
        return None;
    }
    let path = entries.iter().take(2).map(|(ip, h)| format!("{ip} {h}")).collect::<Vec<_>>().join(" · ");
    Some(Finding {
        level: Level::Review,
        title: "Archivo hosts modificado".into(),
        path,
        why: match entries.len() {
            1 => "El archivo hosts tiene 1 entrada añadida".to_string(),
            n => format!("El archivo hosts tiene {n} entradas añadidas"),
        },
        actions: vec![Action::OpenHosts],
    })
}

pub fn proxy_finding(enable: Option<u32>, server: Option<&str>, autoconfig: Option<&str>) -> Option<Finding> {
    fn nonempty(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    let server = nonempty(server).filter(|_| enable == Some(1));
    let auto = nonempty(autoconfig);
    let (path, why) = match (server, auto) {
        (Some(s), _) => (s, "Tu navegador envía el tráfico por un proxy. Si no lo configuraste tú, revísalo."),
        (None, Some(a)) => (a, "Tu navegador usa un script de configuración de proxy. Si no lo configuraste tú, revísalo."),
        _ => return None,
    };
    Some(Finding {
        level: Level::Review,
        title: "Proxy configurado".into(),
        path: path.to_string(),
        why: why.into(),
        actions: vec![],
    })
}

/// Solo debe recibir un nombre de archivo (`Path::file_name`), nunca una ruta: la ruta lleva el
/// nombre de usuario.
pub fn search_url(file_name: &str) -> String {
    let mut s = String::from("https://www.bing.com/search?q=");
    for b in self::file_name(file_name).bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Ordena por nivel (amenazas primero) y junta en uno los hallazgos del mismo archivo (el de
/// nivel más alto), con las acciones de todos en el orden de los botones. Como todas hablan del
/// mismo archivo, de cada clase de acción queda la primera. Una amenaza de Defender se queda solo
/// con las suyas («Abrir Seguridad de Windows»): Defender ya la ha puesto en cuarentena.
pub fn merge(mut v: Vec<Finding>) -> Vec<Finding> {
    fn rank(a: &Action) -> u8 {
        match a {
            Action::DisableStartup(_) => 0,
            Action::Kill(_) => 1,
            Action::OpenLocation(_) => 2,
            Action::Search(_) => 3,
            Action::OpenHosts => 4,
            Action::OpenWindowsSecurity => 5,
        }
    }
    v.sort_by_key(|f| f.level as u8);
    let mut out: Vec<Finding> = Vec::new();
    for f in v {
        let same = out.iter_mut().find(|o| !f.path.is_empty() && o.path.eq_ignore_ascii_case(&f.path));
        match same {
            Some(o) if o.level == Level::Threat => {}
            Some(o) => {
                for a in f.actions {
                    if !o.actions.iter().any(|b| rank(b) == rank(&a)) {
                        o.actions.push(a);
                    }
                }
            }
            None => out.push(f),
        }
    }
    for f in &mut out {
        f.actions.sort_by_key(rank);
    }
    out
}

/// El aviso de encima de los hallazgos: título y texto, según cuántos son amenazas de Defender
/// y cuántos avisos de WinCleaner.
pub fn summary(threats: usize, others: usize) -> (String, String) {
    let title = match threats + others {
        1 => "Hay 1 cosa que revisar.".to_string(),
        n => format!("Hay {n} cosas que revisar."),
    };
    let text = match (threats, others) {
        (0, 1) => "Es un aviso de WinCleaner: puede que sea algo que instalaste tú.".to_string(),
        (0, _) => "Son avisos de WinCleaner: puede que sean cosas que instalaste tú.".to_string(),
        (1, 0) => "La ha encontrado Defender.".to_string(),
        (_, 0) => "Las ha encontrado Defender.".to_string(),
        (t, o) => {
            let a = if t == 1 { "Una la ha encontrado Defender.".to_string() } else { format!("{t} las ha encontrado Defender.") };
            let b = if o == 1 {
                "La otra es un aviso de WinCleaner: puede que sea algo que instalaste tú."
            } else {
                "Las otras son avisos de WinCleaner: puede que sean cosas que instalaste tú."
            };
            format!("{a} {b}")
        }
    };
    (title, text)
}

/// Texto de «No se ha encontrado nada raro»: las `total` comprobaciones y las que no se pudieron
/// hacer (`failed`, por su nombre).
pub fn clean_text(defender_ok: bool, total: usize, failed: &[&str]) -> String {
    let def = if defender_ok { "Defender no ha detectado amenazas" } else { "" };
    match (failed, defender_ok) {
        ([], true) => format!("{def} y las {total} comprobaciones están bien."),
        ([], false) => format!("Las {total} comprobaciones de WinCleaner están bien."),
        _ => {
            let ok = total.saturating_sub(failed.len());
            let s = format!("{ok} de {total} comprobaciones bien; no se pudo revisar: {}.", failed.join(", "));
            if defender_ok { format!("{def}. {s}") } else { s }
        }
    }
}

/// Las únicas URL que WinCleaner abre: `https://…` y `windowsdefender:…`, sin comillas, espacios
/// ni caracteres de control (nada que ShellExecute pueda leer como otro argumento).
pub fn openable_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("windowsdefender:"))
        && !url.chars().any(|c| c == '"' || c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places() -> Places {
        Places {
            temp: vec![r"C:\Users\a\AppData\Local\Temp".into(), r"C:\Windows\Temp".into()],
            downloads: r"C:\Users\a\Downloads".into(),
            app_data: r"C:\Users\a\AppData\Roaming".into(),
            local_app_data: r"C:\Users\a\AppData\Local".into(),
            public: r"C:\Users\Public".into(),
            system_root: r"C:\Windows".into(),
        }
    }

    #[test]
    fn temp_unsigned_startup_is_suspicious() {
        let e = r"C:\Users\a\AppData\Local\Temp\x7f2\updater.exe";
        let f = startup_finding(0, "Updater", Some(e), true, false, &places()).unwrap();
        assert_eq!(f.level, Level::Suspicious);
    }

    #[test]
    fn local_programs_is_not_risky() {
        let e = r"C:\Users\a\AppData\Local\Programs\App\app.exe";
        assert_eq!(risky_place(e, &places()), None);
    }

    #[test]
    fn loose_appdata_is_risky() {
        assert_eq!(risky_place(r"C:\Users\a\AppData\Roaming\x.exe", &places()), Some("AppData"));
        assert_eq!(risky_place(r"C:\Users\a\AppData\Local\x.exe", &places()), Some("AppData"));
    }

    #[test]
    fn empty_places_never_match() {
        assert_eq!(risky_place(r"C:\x.exe", &Places::default()), None);
        assert_eq!(risky_place("basura", &places()), None);
        let mut p = places();
        p.temp = vec![r"C:\".into(), "C:".into()];
        assert_eq!(risky_place(r"C:\Program Files\A\a.exe", &p), None);
    }

    #[test]
    fn missing_startup_exe_is_review() {
        let f = startup_finding(1, "Foo", Some(r"C:\x\foo.exe"), false, true, &places()).unwrap();
        assert_eq!(f.level, Level::Review);
        assert_eq!(f.title, "Foo");
        assert!(f.why.contains("arranca con Windows pero su archivo ya no existe"));
    }

    #[test]
    fn startup_other_cases() {
        let p = places();
        let ok = r"C:\Program Files\A\a.exe";
        assert_eq!(startup_finding(0, "A", Some(ok), true, false, &p).unwrap().level, Level::Review);
        assert!(startup_finding(0, "A", Some(ok), true, true, &p).is_none());
        let risky = r"C:\Users\a\Downloads\a.exe";
        assert_eq!(startup_finding(0, "A", Some(risky), true, true, &p).unwrap().level, Level::Review);
        assert!(startup_finding(0, "A", None, true, false, &p).is_none());
    }

    #[test]
    fn signed_process_in_temp_is_ignored() {
        let e = r"C:\Users\a\AppData\Local\Temp\setup.exe";
        assert!(process_finding(e, true, &places()).is_none());
        assert_eq!(process_finding(e, false, &places()).unwrap().level, Level::Suspicious);
    }

    #[test]
    fn microsoft_service_is_ignored() {
        assert!(service_finding("S", r"C:\x.exe", Some("Microsoft Windows")).is_none());
        assert!(service_finding("S", r"C:\x.exe", Some("NVIDIA Corporation")).is_none());
        assert_eq!(service_finding("S", r"C:\x.exe", None).unwrap().level, Level::Review);
    }

    #[test]
    fn fake_svchost() {
        let f = fake_system_process(r"C:\Users\a\svchost.exe", r"C:\Windows").unwrap();
        assert_eq!(f.level, Level::Suspicious);
        assert!(fake_system_process(r"C:\Windows\System32\sub\svchost.exe", r"C:\Windows").is_some());
        assert!(fake_system_process("basura", r"C:\Windows").is_none());
        assert!(fake_system_process(r"C:\Users\a\otro.exe", r"C:\Windows").is_none());
    }

    #[test]
    fn real_svchost_ok() {
        assert!(fake_system_process(r"c:\windows\system32\svchost.exe", r"C:\Windows").is_none());
        assert!(fake_system_process(r"\\?\C:\Windows\SysWOW64\rundll32.exe", r"C:\Windows").is_none());
    }

    #[test]
    fn explorer_in_root_ok() {
        assert!(fake_system_process(r"C:\Windows\explorer.exe", r"C:\Windows").is_none());
        assert!(fake_system_process(r"C:\Temp\explorer.exe", r"C:\Windows").is_some());
    }

    const STOCK_HOSTS: &str = "# Copyright (c) 1993-2009 Microsoft Corp.\r\n#\r\n# This is a sample HOSTS file used by Microsoft TCP/IP for Windows.\r\n#\r\n# This file contains the mappings of IP addresses to host names. Each\r\n# entry should be kept on an individual line. The IP address should\r\n# be placed in the first column followed by the corresponding host name.\r\n# The IP address and the host name should be separated by at least one\r\n# space.\r\n#\r\n# Additionally, comments (such as these) may be inserted on individual\r\n# lines or following the machine name denoted by a '#' symbol.\r\n#\r\n# For example:\r\n#\r\n#      102.54.94.97     rhino.acme.com          # source server\r\n#       38.25.63.10     x.acme.com              # x client host\r\n\r\n# localhost name resolution is handled within DNS itself.\r\n#\t127.0.0.1       localhost\r\n#\t::1             localhost\r\n";

    #[test]
    fn hosts_all_commented_is_empty() {
        assert_eq!(hosts_entries(STOCK_HOSTS), Vec::<(String, String)>::new());
        assert!(hosts_finding(&hosts_entries(STOCK_HOSTS)).is_none());
    }

    #[test]
    fn hosts_variants() {
        let t = "\u{feff}0.0.0.0\tads.ejemplo.com tracker.ejemplo.net # bloqueo\r\n::1 localhost\r\n";
        let e = hosts_entries(t);
        assert_eq!(
            e,
            vec![("0.0.0.0".into(), "ads.ejemplo.com".into()), ("0.0.0.0".into(), "tracker.ejemplo.net".into())]
        );
        assert!(hosts_entries("hola mundo\n").is_empty());
        assert_eq!(hosts_entries("fe80::1%lo0 x.local").len(), 1);
        assert_eq!(hosts_finding(&e[..1]).unwrap().why, "El archivo hosts tiene 1 entrada añadida");
        let f = hosts_finding(&e).unwrap();
        assert_eq!(f.why, "El archivo hosts tiene 2 entradas añadidas");
        assert_eq!(f.path, "0.0.0.0 ads.ejemplo.com · 0.0.0.0 tracker.ejemplo.net");
    }

    #[test]
    fn proxy_rules() {
        assert_eq!(proxy_finding(Some(1), Some("1.2.3.4:8080"), None).unwrap().level, Level::Review);
        assert!(proxy_finding(Some(0), Some("x"), None).is_none());
        assert_eq!(proxy_finding(None, None, Some("http://wpad/x.pac")).unwrap().level, Level::Review);
        assert!(proxy_finding(Some(1), Some(""), None).is_none());
    }

    #[test]
    fn search_url_encodes_only_name() {
        assert_eq!(search_url("juego crack.exe"), "https://www.bing.com/search?q=juego%20crack.exe");
        assert_eq!(search_url("C:\\Users\\ana\\x.exe"), "https://www.bing.com/search?q=x.exe");
        assert_eq!(search_url("ñ"), "https://www.bing.com/search?q=%C3%B1");
    }

    #[test]
    fn merge_joins_same_file_and_sorts() {
        let p = places();
        let e = r"C:\Users\a\AppData\Local\Temp\x7f2\updater.exe";
        let hosts = hosts_finding(&[("0.0.0.0".into(), "x".into())]).unwrap();
        let start = startup_finding(3, "updater", Some(e), true, false, &p).unwrap();
        let proc = process_finding(&e.to_uppercase(), false, &p).unwrap();
        let v = merge(vec![hosts, start, proc]);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].level, v[0].title.as_str()), (Level::Suspicious, "updater"));
        assert_eq!(
            v[0].actions,
            vec![
                Action::DisableStartup(3),
                Action::Kill(e.to_uppercase()),
                Action::OpenLocation(e.into()),
                Action::Search("UPDATER.EXE".into()),
            ]
        );
        assert_eq!(v[1].level, Level::Review);
    }

    #[test]
    fn merge_keeps_threat_actions() {
        let e = r"C:\Users\a\Downloads\x.exe";
        let threat = Finding {
            level: Level::Threat,
            title: "Defender ha encontrado X".into(),
            path: e.into(),
            why: String::new(),
            actions: vec![Action::OpenWindowsSecurity],
        };
        let v = merge(vec![process_finding(e, false, &places()).unwrap(), threat]);
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].level, v[0].actions.clone()), (Level::Threat, vec![Action::OpenWindowsSecurity]));
    }

    #[test]
    fn clean_text_counts_failures() {
        assert_eq!(clean_text(true, 6, &[]), "Defender no ha detectado amenazas y las 6 comprobaciones están bien.");
        assert_eq!(clean_text(false, 6, &[]), "Las 6 comprobaciones de WinCleaner están bien.");
        assert_eq!(clean_text(false, 6, &["Archivo hosts"]), "5 de 6 comprobaciones bien; no se pudo revisar: Archivo hosts.");
        assert_eq!(
            clean_text(true, 6, &["A", "B"]),
            "Defender no ha detectado amenazas. 4 de 6 comprobaciones bien; no se pudo revisar: A, B."
        );
    }

    #[test]
    fn summary_texts() {
        assert_eq!(summary(0, 1).0, "Hay 1 cosa que revisar.");
        assert_eq!(
            summary(1, 2),
            (
                "Hay 3 cosas que revisar.".into(),
                "Una la ha encontrado Defender. Las otras son avisos de WinCleaner: puede que sean cosas que instalaste tú.".into()
            )
        );
        assert_eq!(summary(2, 0).1, "Las ha encontrado Defender.");
    }

    #[test]
    fn openable_urls() {
        assert!(openable_url("https://www.bing.com/search?q=x.exe"));
        assert!(openable_url("windowsdefender://threat"));
        for malo in ["http://x", "file:///C:/x", "javascript:alert(1)", "HTTPS://x", "C:\\x.exe", "", " https://x", "https://x\n", "https://x\" y", "https://a b", "https://a\u{a0}b"] {
            assert!(!openable_url(malo), "{malo}");
        }
    }
}
