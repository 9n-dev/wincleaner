/// Tamaño legible en español (base 1024), p. ej. "1,5 KB".
pub fn bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unidad = 0;
    // Desde 1023,5 se redondearía a «1024 KB»: mejor «1 MB».
    while v >= 1023.5 && unidad < 4 {
        v /= 1024.0;
        unidad += 1;
    }
    let texto = if v < 100.0 {
        format!("{v:.1}").replace('.', ",").replace(",0", "")
    } else {
        format!("{v:.0}")
    };
    format!("{texto} {}", ["B", "KB", "MB", "GB", "TB"][unidad])
}

/// Número con grupos de 3 cifras separados por espacio.
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Duración legible: "menos de 1 s", "5 s", "2 min 41 s", "1 h 2 min". Las partes a cero
/// se omiten ("2 min", "1 h").
pub fn duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    match (h, m, s) {
        (0, 0, 0) => "menos de 1 s".into(),
        (0, 0, s) => format!("{s} s"),
        (0, m, 0) => format!("{m} min"),
        (0, m, s) => format!("{m} min {s} s"),
        (h, 0, _) => format!("{h} h"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

/// Fecha `(año, mes, día)` como "03/02/2025".
pub fn date_dmy((y, m, d): (u16, u8, u8)) -> String {
    format!("{d:02}/{m:02}/{y:04}")
}

/// Antigüedad aproximada: "hace 3 h", "hace 1 mes" (meses de 30 días, años de 365).
pub fn ago(d: std::time::Duration) -> String {
    let s = d.as_secs();
    let (min, h, days) = (s / 60, s / 3600, s / 86400);
    let plural = |n: u64, uno: &str, varios: &str| format!("hace {n} {}", if n == 1 { uno } else { varios });
    match s {
        0..60 => "hace un momento".into(),
        60..3600 => format!("hace {min} min"),
        3600..86400 => format!("hace {h} h"),
        _ if days < 30 => plural(days, "día", "días"),
        _ if days < 365 => plural(days / 30, "mes", "meses"),
        _ => plural(days / 365, "año", "años"),
    }
}

/// "64 %"; "<1 %" si es menos de uno pero no cero; con `whole` a 0, "0 %".
pub fn percent(part: u64, whole: u64) -> String {
    if whole == 0 || part == 0 {
        return "0 %".into();
    }
    match part as u128 * 100 / whole as u128 {
        0 => "<1 %".into(),
        p => format!("{p} %"),
    }
}

/// Fecha UTC `(año, mes, día)` (algoritmo *civil from days*).
pub fn ymd(t: std::time::SystemTime) -> (u16, u8, u8) {
    let days = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() / 86400;
    let z = days + 719_468;
    let (era, doe) = (z / 146_097, z % 146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((yoe + era * 400 + (m <= 2) as u64) as u16, m as u8, d as u8)
}

/// Queda poco espacio: menos del 15 % libre. Con `total` a 0 (no se pudo leer) no avisa.
pub fn low_space(free: u64, total: u64) -> bool {
    free as u128 * 100 < total as u128 * 15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_space_threshold() {
        assert!(low_space(14, 100));
        assert!(!low_space(15, 100));
        assert!(!low_space(0, 0));
    }

    #[test]
    fn bytes_examples() {
        let casos: [(u64, &str); 11] = [
            (0, "0 B"),
            (512, "512 B"),
            (1536, "1,5 KB"),
            (12 * 1024, "12 KB"),
            (1023 * 1024, "1023 KB"),
            (1024 * 1024 - 1, "1 MB"),
            (640 * 1024 * 1024, "640 MB"),
            (1932735283, "1,8 GB"),
            (22977575731, "21,4 GB"),
            (237 * 1024u64.pow(3), "237 GB"),
            (2 * 1024u64.pow(4), "2 TB"),
        ];
        for (n, esperado) in casos {
            assert_eq!(bytes(n), esperado, "n = {n}");
        }
    }

    #[test]
    fn duration_formats() {
        use std::time::Duration;
        assert_eq!(duration(Duration::from_millis(300)), "menos de 1 s");
        assert_eq!(duration(Duration::from_secs(5)), "5 s");
        assert_eq!(duration(Duration::from_secs(161)), "2 min 41 s");
        assert_eq!(duration(Duration::from_secs(3720)), "1 h 2 min");
    }

    #[test]
    fn ago_formats() {
        use std::time::Duration;
        let d = |s: u64| ago(Duration::from_secs(s));
        let day = 86400;
        assert_eq!(d(30), "hace un momento");
        assert_eq!(d(120), "hace 2 min");
        assert_eq!(d(3 * 3600), "hace 3 h");
        assert_eq!(d(day), "hace 1 día");
        assert_eq!(d(3 * day), "hace 3 días");
        assert_eq!(d(45 * day), "hace 1 mes");
        assert_eq!(d(240 * day), "hace 8 meses");
        assert_eq!(d(365 * day), "hace 1 año");
        assert_eq!(d(800 * day), "hace 2 años");
    }

    #[test]
    fn percent_formats() {
        assert_eq!(percent(64, 100), "64 %");
        assert_eq!(percent(1, 1000), "<1 %");
        assert_eq!(percent(0, 100), "0 %");
        assert_eq!(percent(5, 0), "0 %");
    }

    #[test]
    fn ymd_utc() {
        use std::time::{Duration, UNIX_EPOCH};
        assert_eq!(ymd(UNIX_EPOCH), (1970, 1, 1));
        assert_eq!(ymd(UNIX_EPOCH + Duration::from_secs(1_792_777_320)), (2026, 10, 23));
    }

    #[test]
    fn date_dmy_pads() {
        assert_eq!(date_dmy((2025, 2, 3)), "03/02/2025");
    }

    #[test]
    fn count_examples() {
        for (n, esperado) in [(0, "0"), (999, "999"), (12304, "12 304"), (1000000, "1 000 000")] {
            assert_eq!(count(n), esperado);
        }
    }
}
