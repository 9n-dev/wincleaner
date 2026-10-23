/// Tamaño legible en español (base 1024), p. ej. "1,5 KB".
pub fn bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unidad = 0;
    while v >= 1024.0 && unidad < 4 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_examples() {
        let casos: [(u64, &str); 9] = [
            (0, "0 B"),
            (512, "512 B"),
            (1536, "1,5 KB"),
            (12 * 1024, "12 KB"),
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
    fn count_examples() {
        for (n, esperado) in [(0, "0"), (999, "999"), (12304, "12 304"), (1000000, "1 000 000")] {
            assert_eq!(count(n), esperado);
        }
    }
}
