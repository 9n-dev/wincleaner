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
    if !local_path(system32) || exe.chars().any(char::is_control) {
        return None;
    }
    if local_path(exe) {
        return Some(exe.to_string());
    }
    let suelto = !exe.is_empty()
        && exe != "."
        && exe != ".."
        && !exe.contains(['\\', '/', ':']);
    suelto.then(|| format!("{}\\{exe}", system32.trim_end_matches('\\')))
}

/// Ruta absoluta con letra de unidad (`X:\…` o `X:/…`) y sin caracteres de control. Nunca es de
/// red (`\\host\…`, `\\host@SSL\…`), `\\?\` ni relativa: abrirla no conecta con nada.
pub fn local_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/')
        && !p.chars().any(char::is_control)
}

/// Ejecutable de la `ImagePath` de un servicio, ya con las variables expandidas: quita `\??\`,
/// cambia `\SystemRoot\` por la carpeta de Windows y después aplica `resolve`.
pub fn service_exe(image_path: &str, system_root: &str, system32: &str) -> Option<String> {
    let (exe, _) = split_exe(image_path)?;
    let strip = |pre: &str| exe.get(..pre.len()).filter(|p| p.eq_ignore_ascii_case(pre)).map(|_| &exe[pre.len()..]);
    let exe = if let Some(r) = strip("\\??\\") {
        r.to_string()
    } else if let Some(r) = strip("\\SystemRoot\\") {
        format!("{}\\{r}", system_root.trim_end_matches('\\'))
    } else {
        exe.clone()
    };
    resolve(&exe, system32)
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

    #[test]
    fn local_path_rules() {
        assert!(local_path("C:\\a\\x.ico") && local_path("d:/x.exe"));
        for malo in ["\\\\host\\x.ico", "\\\\host@SSL\\s\\x.ico", "\\\\?\\C:\\x.ico", "//host/x", "C:x", "x.ico", "", "C:\\a\u{1}.ico"] {
            assert!(!local_path(malo), "{malo}");
        }
    }

    #[test]
    fn service_exe_rules() {
        let (w, s) = ("C:\\Windows", "C:\\Windows\\System32");
        let svc = |p| service_exe(p, w, s);
        assert_eq!(svc("\"C:\\P F\\a.exe\" -run"), Some("C:\\P F\\a.exe".into()));
        assert_eq!(svc("C:\\P F\\a.exe -run"), Some("C:\\P F\\a.exe".into()));
        assert_eq!(svc("\\SystemRoot\\System32\\x.exe -k"), Some("C:\\Windows\\System32\\x.exe".into()));
        assert_eq!(svc("\\systemroot\\x.exe"), Some("C:\\Windows\\x.exe".into()));
        assert_eq!(svc("\"\\??\\D:\\s\\y.exe\""), Some("D:\\s\\y.exe".into()));
        assert_eq!(svc("lsass.exe"), Some("C:\\Windows\\System32\\lsass.exe".into()));
        for malo in ["", "\\\\srv\\x.exe", "System32\\x.exe", "\\??\\UNC\\srv\\x.exe"] {
            assert_eq!(svc(malo), None, "{malo}");
        }
    }
}
