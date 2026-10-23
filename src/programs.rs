//! Reglas de la lista de programas instalados y de su desinstalación (sin Windows).

use crate::cmdline::{resolve, split_exe};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hive {
    #[default]
    Machine64,
    Machine32,
    User,
}

/// Una clave `Uninstall` tal como se lee del registro; todo puede faltar o venir mal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawEntry {
    pub key: String,
    pub hive: Hive,
    pub display_name: Option<String>,
    pub publisher: Option<String>,
    pub install_date: Option<String>,
    pub estimated_size_kb: Option<u32>,
    pub uninstall_string: Option<String>,
    pub windows_installer: bool,
    pub system_component: bool,
    pub parent_key_name: Option<String>,
    pub display_icon: Option<String>,
}

/// Sin nombre, componente del sistema o parche/actualización de otro programa: no se muestra.
pub fn visible(e: &RawEntry) -> bool {
    e.display_name.as_deref().is_some_and(|n| !n.trim().is_empty())
        && !e.system_component
        && e.parent_key_name.is_none()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UninstallCmd {
    pub exe: String,
    pub args: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UninstallError {
    /// No hay nada que ejecutar.
    Missing,
    /// La ruta no es absoluta ni un nombre suelto de `system32`.
    BadPath,
}

/// `{…}` con solo hex y guiones: lo que se pega en la línea de `msiexec`, así que es estricto.
fn is_guid(s: &str) -> bool {
    s.len() > 2
        && s.starts_with('{')
        && s.ends_with('}')
        && s[1..s.len() - 1].chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn is_msiexec(exe: &str) -> bool {
    let nombre = exe.rsplit(['\\', '/']).next().unwrap_or("");
    nombre.eq_ignore_ascii_case("msiexec") || nombre.eq_ignore_ascii_case("msiexec.exe")
}

/// GUID de un `/I`, `/X`, `-I` o `-X` (pegado `/I{…}` o en el token siguiente).
fn msi_guid(args: &str) -> Option<String> {
    let t: Vec<&str> = args.split_whitespace().collect();
    t.iter().enumerate().find_map(|(i, tok)| {
        let b = tok.as_bytes();
        if b.len() < 2 || !matches!(b[0], b'/' | b'-') || !matches!(b[1].to_ascii_lowercase(), b'i' | b'x') {
            return None;
        }
        let g = if b.len() == 2 { t.get(i + 1).copied()? } else { &tok[2..] };
        is_guid(g).then(|| g.to_string())
    })
}

pub fn uninstall_command(e: &RawEntry, system32: &str) -> Result<UninstallCmd, UninstallError> {
    let msi = |g: &str| {
        let exe = resolve("msiexec.exe", system32).ok_or(UninstallError::BadPath)?;
        Ok(UninstallCmd { exe, args: format!("/x {g}") })
    };
    if e.windows_installer && is_guid(&e.key) {
        return msi(&e.key);
    }
    let cmd = e.uninstall_string.as_deref().unwrap_or("");
    let (exe, args) = split_exe(cmd).ok_or(UninstallError::Missing)?;
    if is_msiexec(&exe) {
        // msiexec solo se lanza para desinstalar un GUID: nada de /i con rutas, /a, /p…
        return msi_guid(&args).map_or(Err(UninstallError::BadPath), |g| msi(&g));
    }
    let exe = resolve(&exe, system32).ok_or(UninstallError::BadPath)?;
    Ok(UninstallCmd { exe, args })
}

/// Con el usuario siendo administrador, no se lanza lo de HKCU: lo habría escrito cualquiera.
pub fn may_run(hive: Hive, is_admin: bool) -> bool {
    !(hive == Hive::User && is_admin)
}

/// `"C:\a.exe",3` o `C:\a.ico` → (ruta, índice); sin índice, 0.
pub fn icon_location(s: &str) -> Option<(String, i32)> {
    let s = s.trim();
    let (ruta, idx) = if let Some(r) = s.strip_prefix('"') {
        let fin = r.find('"')?;
        (&r[..fin], r[fin + 1..].trim().strip_prefix(',').map(str::trim))
    } else {
        match s.rsplit_once(',') {
            Some((p, i)) if i.trim().parse::<i32>().is_ok() => (p, Some(i.trim())),
            _ => (s, None),
        }
    };
    let ruta = ruta.trim();
    if ruta.is_empty() {
        return None;
    }
    Some((ruta.to_string(), idx.and_then(|i| i.parse().ok()).unwrap_or(0)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Size,
    Name,
    Date,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub key: String,
    pub hive: Hive,
    pub name: String,
    pub publisher: String,
    pub date: Option<(u16, u8, u8)>,
    /// Bytes.
    pub size: Option<u64>,
    pub icon: Option<(String, i32)>,
}

/// Tamaño y fecha: de mayor a menor y los desconocidos al final; nombre: A-Z sin mayúsculas.
pub fn sort(v: &mut [Program], k: SortKey) {
    match k {
        SortKey::Size => v.sort_by_key(|p| std::cmp::Reverse(p.size)),
        SortKey::Date => v.sort_by_key(|p| std::cmp::Reverse(p.date)),
        SortKey::Name => v.sort_by_key(|p| p.name.to_lowercase()),
    }
}

/// La consulta aparece (sin distinguir mayúsculas) en el nombre o en el editor.
pub fn matches(p: &Program, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    p.name.to_lowercase().contains(&q) || p.publisher.to_lowercase().contains(&q)
}

/// `AAAAMMDD` válido, o `None`.
fn parse_date(s: &str) -> Option<(u16, u8, u8)> {
    let s = s.trim();
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (y, m, d) = (s[..4].parse().ok()?, s[4..6].parse().ok()?, s[6..].parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

pub fn from_raw(e: &RawEntry) -> Program {
    Program {
        key: e.key.clone(),
        hive: e.hive,
        name: e.display_name.as_deref().unwrap_or("").trim().to_string(),
        publisher: e.publisher.as_deref().unwrap_or("").trim().to_string(),
        date: e.install_date.as_deref().and_then(parse_date),
        size: e.estimated_size_kb.filter(|&k| k > 0).map(|k| u64::from(k) * 1024),
        icon: e.display_icon.as_deref().and_then(icon_location),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitState {
    Waiting,
    Removed,
    StillThere,
}

/// Espera tras lanzar el desinstalador. Con `runas` (sin job) que termine el proceso no
/// cierra la espera: NSIS se copia a Temp y sigue.
pub fn wait_state(key_present: bool, tracked_alive: bool, tracked_is_job: bool, user_done: bool) -> WaitState {
    if !key_present {
        WaitState::Removed
    } else if user_done || (tracked_is_job && !tracked_alive) {
        WaitState::StillThere
    } else {
        WaitState::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "C:\\Windows\\System32";

    fn raw() -> RawEntry {
        RawEntry { key: "App".into(), display_name: Some("App".into()), ..Default::default() }
    }

    fn prog(name: &str) -> Program {
        Program { name: name.into(), ..from_raw(&raw()) }
    }

    #[test]
    fn uninstall_msi_install_flag_becomes_x() {
        let e = RawEntry { uninstall_string: Some("MsiExec.exe /I{1234-5678}".into()), ..raw() };
        let c = uninstall_command(&e, S).unwrap();
        assert!(c.exe.ends_with("\\msiexec.exe"));
        assert_eq!(c.args, "/x {1234-5678}");
    }

    #[test]
    fn uninstall_msi_by_key() {
        let e = RawEntry { windows_installer: true, key: "{ABC}".into(), ..raw() };
        assert_eq!(uninstall_command(&e, S).unwrap().args, "/x {ABC}");
    }

    #[test]
    fn uninstall_msi_rejects_injection() {
        let e = RawEntry { uninstall_string: Some("msiexec /x {AB} & calc".into()), ..raw() };
        assert_eq!(uninstall_command(&e, S).unwrap().args, "/x {AB}");
    }

    #[test]
    fn uninstall_msiexec_without_guid_is_bad() {
        for cmd in [
            "msiexec /i \\\\srv\\x.msi /qn",
            "MsiExec.exe -i C:\\x.msi",
            "C:\\Windows\\System32\\msiexec.exe /package x.msi",
            "msiexec /invalid {AB}",
        ] {
            let e = RawEntry { uninstall_string: Some(cmd.into()), ..raw() };
            assert_eq!(uninstall_command(&e, S), Err(UninstallError::BadPath), "{cmd}");
        }
        let e = RawEntry { uninstall_string: Some("msiexec -X {AB}".into()), ..raw() };
        assert_eq!(uninstall_command(&e, S).unwrap().args, "/x {AB}");
    }

    #[test]
    fn uninstall_missing() {
        assert_eq!(uninstall_command(&raw(), S), Err(UninstallError::Missing));
    }

    #[test]
    fn uninstall_relative_dir_is_bad() {
        let e = RawEntry { uninstall_string: Some("tools\\u.exe".into()), ..raw() };
        assert_eq!(uninstall_command(&e, S), Err(UninstallError::BadPath));
    }

    #[test]
    fn hidden_entries() {
        assert!(visible(&raw()));
        assert!(!visible(&RawEntry { display_name: None, ..raw() }));
        assert!(!visible(&RawEntry { system_component: true, ..raw() }));
        assert!(!visible(&RawEntry { parent_key_name: Some("KB1".into()), ..raw() }));
    }

    #[test]
    fn hkcu_blocked_when_admin() {
        assert!(!may_run(Hive::User, true));
        assert!(may_run(Hive::Machine64, true));
        assert!(may_run(Hive::User, false));
    }

    #[test]
    fn install_date_parses() {
        assert_eq!(parse_date("20250203"), Some((2025, 2, 3)));
        for malo in ["2025-02-03", "", "20251340"] {
            assert_eq!(parse_date(malo), None);
        }
    }

    #[test]
    fn size_zero_or_missing_is_none() {
        let t = |k| from_raw(&RawEntry { estimated_size_kb: k, ..raw() }).size;
        assert_eq!(t(Some(0)), None);
        assert_eq!(t(None), None);
        assert_eq!(t(Some(1024)), Some(1_048_576));
    }

    #[test]
    fn icon_location_parses() {
        assert_eq!(icon_location("\"C:\\a.exe\",3"), Some(("C:\\a.exe".into(), 3)));
        assert_eq!(icon_location("C:\\a.ico"), Some(("C:\\a.ico".into(), 0)));
        assert_eq!(icon_location(""), None);
    }

    #[test]
    fn wait_runas_ignores_process_exit() {
        assert_eq!(wait_state(true, false, false, false), WaitState::Waiting);
    }

    #[test]
    fn wait_job_empty_finishes() {
        assert_eq!(wait_state(true, false, true, false), WaitState::StillThere);
    }

    #[test]
    fn wait_key_gone() {
        assert_eq!(wait_state(false, true, true, false), WaitState::Removed);
        assert_eq!(wait_state(true, true, false, true), WaitState::StillThere);
    }

    #[test]
    fn search_is_case_insensitive() {
        assert!(matches(&prog("Zoom Workplace"), "zoom"));
        assert!(!matches(&prog("Zoom Workplace"), "teams"));
    }

    #[test]
    fn sort_orders() {
        let mut v = vec![
            Program { size: Some(1), ..prog("b") },
            Program { size: None, ..prog("A") },
            Program { size: Some(9), ..prog("c") },
        ];
        sort(&mut v, SortKey::Size);
        assert_eq!(v.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["c", "b", "A"]);
        sort(&mut v, SortKey::Name);
        assert_eq!(v.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["A", "b", "c"]);
    }
}
