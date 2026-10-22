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
    fn count_examples() {
        for (n, esperado) in [(0, "0"), (999, "999"), (12304, "12 304"), (1000000, "1 000 000")] {
            assert_eq!(count(n), esperado);
        }
    }
}
