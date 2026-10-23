//! Partir una línea de comandos del registro (`UninstallString`) en ejecutable + argumentos,
//! y decidir si el ejecutable se puede lanzar. Solo léxico, sin tocar el disco.

/// Separa el ejecutable de los argumentos (estos, recortados). Con comillas, hasta la de
/// cierre; sin ellas, hasta el primer `.exe` seguido de espacio o final; si no hay, hasta el
/// primer espacio.
pub fn split_exe(cmd: &str) -> Option<(String, String)> {
    let cmd = cmd.trim();
    let (exe, resto) = if let Some(r) = cmd.strip_prefix('"') {
        let fin = r.find('"')?;
        (&r[..fin], &r[fin + 1..])
    } else {
        // `to_ascii_lowercase` conserva las posiciones de los bytes
        let bajo = cmd.to_ascii_lowercase();
        let fin = bajo
            .match_indices(".exe")
            .map(|(i, _)| i + 4)
            .find(|&e| cmd[e..].chars().next().is_none_or(char::is_whitespace))
            .or_else(|| cmd.find(char::is_whitespace))
            .unwrap_or(cmd.len());
        (&cmd[..fin], &cmd[fin..])
    };
    let exe = exe.trim();
    if exe.is_empty() { None } else { Some((exe.to_string(), resto.trim().to_string())) }
}

/// Ruta absoluta con letra de unidad (`X:\…`) tal cual; nombre suelto, bajo `system32`.
/// Todo lo demás (relativas con carpeta, `C:x`, UNC, `\\?\`, `\x`) da `None`, para que nada
/// se resuelva por `PATH` ni por la carpeta actual. La existencia la comprueba la capa Win32.
pub fn resolve(exe: &str, system32: &str) -> Option<String> {
    let abs = |x: &str| {
        let b = x.as_bytes();
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
    };
    if !abs(system32) || exe.chars().any(char::is_control) {
        return None;
    }
    if abs(exe) {
        return Some(exe.to_string());
    }
    let suelto = !exe.is_empty()
        && exe != "."
        && exe != ".."
        && !exe.contains(['\\', '/', ':']);
    suelto.then(|| format!("{}\\{exe}", system32.trim_end_matches('\\')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_quoted() {
        assert_eq!(
            split_exe("\"C:\\P F\\u.exe\" /S"),
            Some(("C:\\P F\\u.exe".to_string(), "/S".to_string()))
        );
    }

    #[test]
    fn split_unquoted_with_spaces() {
        assert_eq!(
            split_exe("C:\\Program Files\\App\\unins000.exe /S"),
            Some(("C:\\Program Files\\App\\unins000.exe".to_string(), "/S".to_string()))
        );
        assert_eq!(split_exe("tool /x").map(|r| r.0), Some("tool".to_string()));
    }

    #[test]
    fn split_empty() {
        assert_eq!(split_exe("   "), None);
        assert_eq!(split_exe("\"\" /S"), None);
        assert_eq!(split_exe("\"sin cierre"), None);
    }

    #[test]
    fn resolve_rules() {
        let s = "C:\\Windows\\System32";
        assert_eq!(resolve("rundll32.exe", s), Some("C:\\Windows\\System32\\rundll32.exe".into()));
        assert_eq!(resolve("D:\\a\\u.exe", s), Some("D:\\a\\u.exe".into()));
        assert_eq!(resolve("x.exe", "System32"), None);
        assert_eq!(resolve("x.exe", "\\\\srv\\s"), None);
        for malo in ["sub\\x.exe", "\\\\srv\\x.exe", "\\\\?\\C:\\x.exe", "C:x.exe", "\\x.exe", "..", "", "a/b.exe", "C:\\a\u{7}.exe"] {
            assert_eq!(resolve(malo, s), None, "{malo}");
        }
    }
}
