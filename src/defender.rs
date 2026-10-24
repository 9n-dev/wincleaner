//! Reglas puras de Microsoft Defender: versión de la plataforma, análisis, disponibilidad y WMI.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Versión mayor de las carpetas de `Platform`, comparando por números separados por `.` o `-`.
/// Los nombres que no se leen como versión se ignoran.
pub fn latest_platform(names: &[String]) -> Option<String> {
    let key = |n: &str| -> Option<Vec<u64>> {
        n.split(['.', '-']).map(|p| p.parse().ok()).collect()
    };
    names.iter().filter_map(|n| key(n).map(|k| (k, n))).max_by(|a, b| a.0.cmp(&b.0)).map(|(_, n)| n.clone())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanKind {
    Quick,
    Full,
}

pub fn scan_args(k: ScanKind) -> [&'static str; 3] {
    ["-Scan", "-ScanType", if k == ScanKind::Quick { "1" } else { "2" }]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanExit {
    Clean,
    Threats,
    NeedsAdmin,
    Cancelled,
    Failed(i32),
}

pub fn scan_exit(code: i32, output: &str, cancelled: bool) -> ScanExit {
    if cancelled {
        ScanExit::Cancelled
    } else if code == 0 {
        ScanExit::Clean
    } else if code == 2 {
        ScanExit::Threats
    } else if output.contains("0x80070005") {
        ScanExit::NeedsAdmin
    } else {
        ScanExit::Failed(code)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub signature_age_days: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Defender(Status),
    OtherAv(Vec<String>),
    NoAv,
}

pub fn availability(status: Option<Status>, others: &[String]) -> Availability {
    if let Some(s) = status.filter(|s| s.enabled) {
        return Availability::Defender(s);
    }
    let v: Vec<String> = others.iter().filter(|n| !n.contains("Defender")).cloned().collect();
    if v.is_empty() { Availability::NoAv } else { Availability::OtherAv(v) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    pub name: String,
    pub path: String,
    pub when: SystemTime,
}

/// Quita el prefijo de tipo de WMI (`file:_`, `regkey:_`…): letras, `:` y `_`.
pub fn parse_resource(s: &str) -> String {
    if let Some((pre, rest)) = s.split_once(":_")
        && !pre.is_empty()
        && pre.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return rest.to_string();
    }
    s.to_string()
}

/// `AAAAMMDDhhmmss.ffffff±UUU`; `UUU` son minutos de desfase con la hora UTC. `None` si algo falla.
pub fn parse_wmi_time(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    if b.len() != 25 || b[14] != b'.' || !matches!(b[21], b'+' | b'-') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let d = b.get(r)?;
        if d.is_empty() || !d.iter().all(u8::is_ascii_digit) {
            return None;
        }
        d.iter().try_fold(0i64, |a, &c| Some(a * 10 + i64::from(c - b'0')))
    };
    let (y, mo, d) = (num(0..4)?, num(4..6)?, num(6..8)?);
    let (h, mi, sec) = (num(8..10)?, num(10..12)?, num(12..14)?);
    num(15..21)?;
    let off = num(22..25)? * if b[21] == b'-' { -1 } else { 1 };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Días desde 1970-01-01 (algoritmo de Hinnant).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - off * 60;
    u64::try_from(secs).ok().map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn latest_platform_picks_highest() {
        assert_eq!(
            latest_platform(&v(&["4.18.24080.9-0", "4.18.24090.11-0", "basura"])).as_deref(),
            Some("4.18.24090.11-0")
        );
        assert_eq!(latest_platform(&v(&["basura"])), None);
        assert_eq!(latest_platform(&v(&["4.18.9-0", "4.18.10-0"])).as_deref(), Some("4.18.10-0"));
    }

    #[test]
    fn scan_args_kinds() {
        assert_eq!(scan_args(ScanKind::Quick), ["-Scan", "-ScanType", "1"]);
        assert_eq!(scan_args(ScanKind::Full)[2], "2");
    }

    #[test]
    fn scan_exit_rules() {
        assert_eq!(scan_exit(0, "", false), ScanExit::Clean);
        assert_eq!(scan_exit(2, "", false), ScanExit::Threats);
        assert_eq!(scan_exit(1, "… 0x80070005 …", false), ScanExit::NeedsAdmin);
        assert_eq!(scan_exit(0, "", true), ScanExit::Cancelled);
        assert_eq!(scan_exit(1, "", false), ScanExit::Failed(1));
    }

    #[test]
    fn availability_defender_off_names_other() {
        let off = Status { enabled: false, signature_age_days: None };
        assert_eq!(
            availability(Some(off), &v(&["Windows Defender", "Avast Free Antivirus"])),
            Availability::OtherAv(v(&["Avast Free Antivirus"]))
        );
        let on = Status { enabled: true, signature_age_days: Some(1) };
        assert_eq!(availability(Some(on), &[]), Availability::Defender(on));
        assert_eq!(availability(Some(off), &v(&["Windows Defender"])), Availability::NoAv);
    }

    #[test]
    fn availability_wmi_denied_no_av() {
        assert_eq!(availability(None, &[]), Availability::NoAv);
    }

    #[test]
    fn wmi_time_parses() {
        let want = UNIX_EPOCH + Duration::from_secs(1_792_777_320); // 2026-10-23 17:42:00 UTC
        assert_eq!(parse_wmi_time("20261023184200.000000+060"), Some(want));
        assert_eq!(parse_wmi_time("20261023164200.000000-060"), Some(want));
        assert_eq!(parse_wmi_time("basura"), None);
        assert_eq!(parse_wmi_time("2026102318420é.000000+060"), None);
        assert_eq!(parse_wmi_time("20261323184200.000000+060"), None);
        assert_eq!(parse_wmi_time(""), None);
    }

    #[test]
    fn resource_prefix_stripped() {
        assert_eq!(parse_resource("file:_C:\\Users\\a\\Downloads\\x.exe"), "C:\\Users\\a\\Downloads\\x.exe");
        assert_eq!(parse_resource("C:\\a_b:_c"), "C:\\a_b:_c");
    }
}
