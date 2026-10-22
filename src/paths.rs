//! Seguridad de rutas: comparaciones puramente léxicas, sin tocar el disco.

/// Componentes en minúsculas, o `None` si la forma no es aceptable.
fn parse(p: &str) -> Option<Vec<String>> {
    if p.starts_with(r"\\") || p.starts_with("//") {
        return None;
    }
    let unix = p.starts_with('/');
    if unix && cfg!(windows) {
        return None; // en Windows '/x' se resuelve contra la unidad actual
    }
    let mut comps: Vec<String> = Vec::new();
    for (i, c) in p.split(['\\', '/']).filter(|c| !c.is_empty()).enumerate() {
        if c == "." || c == ".." {
            return None;
        }
        let drive = i == 0 && !unix;
        if drive {
            let b = c.as_bytes();
            if b.len() != 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
                return None;
            }
        } else if c.contains(':') {
            return None;
        }
        comps.push(c.to_lowercase());
    }
    if comps.is_empty() { None } else { Some(comps) }
}

/// `path` es descendiente estricto de `root`.
pub fn is_inside(path: &str, root: &str) -> bool {
    match (parse(path), parse(root)) {
        (Some(p), Some(r)) => p.len() > r.len() && p.starts_with(&r),
        _ => false,
    }
}

/// Ruta de `GetFinalPathNameByHandleW` (`\\?\C:\…`) sin el prefijo; `None` si es de red
/// (`\\?\UNC\…`) o no lleva el prefijo. Lo que quede raro lo rechaza después `is_inside`.
pub fn strip_verbatim(p: &str) -> Option<&str> {
    p.strip_prefix(r"\\?\").filter(|r| !r.get(..4).is_some_and(|u| u.eq_ignore_ascii_case(r"UNC\")))
}

pub fn root_is_safe(root: &str, protected: &[String], forbidden_trees: &[String]) -> bool {
    let Some(r) = parse(root) else { return false };
    let all = |list: &[String]| list.iter().filter_map(|s| parse(s)).collect::<Vec<_>>();
    let (prot, forb) = (all(protected), all(forbidden_trees));
    // igual o antecesor de algo protegido/prohibido (r es prefijo de x)
    if prot.iter().chain(&forb).any(|x| x.starts_with(&r)) {
        return false;
    }
    !forb.iter().any(|f| r.starts_with(f))
}

pub fn temp_name_ok(root: &str) -> bool {
    let Some(c) = parse(root) else { return false };
    let is_temp = |s: &String| s == "temp" || s == "tmp";
    match c.as_slice() {
        [.., last] if is_temp(last) => true,
        [.., prev, last] => last.bytes().all(|b| b.is_ascii_digit()) && is_temp(prev),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const T: &str = r"C:\Users\M\AppData\Local\Temp";

    #[test]
    fn inside_basic() {
        assert!(is_inside(r"C:\Users\M\AppData\Local\Temp\a.txt", T));
        assert!(is_inside(r"C:\Users\M\AppData\Local\Temp\x\y\z.tmp", T));
    }

    #[test]
    fn inside_is_case_insensitive_and_accepts_both_separators() {
        assert!(is_inside(r"c:\users\m\appdata\local\TEMP\a", T));
        assert!(is_inside("C:/Users/M/AppData/Local/Temp/a", T));
        assert!(is_inside(r"C:\Users\M\AppData\Local\Temp\a", r"C:\Users\M\AppData\Local\Temp\"));
    }

    #[test]
    fn root_itself_is_not_inside() {
        assert!(!is_inside(T, T));
        assert!(!is_inside(&format!(r"{T}\"), T));
    }

    #[test]
    fn sibling_prefix_is_not_inside() {
        assert!(!is_inside(r"C:\Users\M\AppData\Local\Temp2\a", T));
    }

    #[test]
    fn dot_components_are_rejected() {
        assert!(!is_inside(r"C:\Users\M\AppData\Local\Temp\..\..\..\Documents\a", T));
        assert!(!is_inside(r"C:\Users\M\AppData\Local\Temp\.\a", T));
        assert!(!is_inside(r"C:\Users\M\AppData\Local\Temp\a", r"C:\Users\M\AppData\Local\Temp\.."));
    }

    #[test]
    fn odd_forms_are_rejected() {
        assert!(!is_inside(r"\\?\C:\Users\M\AppData\Local\Temp\a", T));
        assert!(!is_inside(r"\\server\share\Temp\a", r"\\server\share\Temp"));
        assert!(!is_inside(r"Temp\a", "Temp"));
        assert!(!is_inside(r"C:\Users\M\AppData\Local\Temp\a.txt:zone", T));
        assert!(!is_inside(r"D:\Users\M\AppData\Local\Temp\a", T));
        assert!(!is_inside("", T));
        assert!(!is_inside(r"C:\a", ""));
    }

    #[test]
    fn strip_verbatim_only_accepts_local_drive_paths() {
        assert_eq!(strip_verbatim(r"\\?\C:\Windows\Temp\a"), Some(r"C:\Windows\Temp\a"));
        assert_eq!(strip_verbatim(r"\\?\UNC\server\share\a"), None);
        assert_eq!(strip_verbatim(r"\\?\unc\server\share\a"), None);
        assert_eq!(strip_verbatim(r"C:\Windows\Temp\a"), None);
        assert_eq!(strip_verbatim(r"\\.\C:\a"), None);
    }

    #[test]
    fn unix_absolute_paths_work_for_tests() {
        assert!(is_inside("/tmp/w/Temp/a", "/tmp/w/Temp"));
        assert!(!is_inside("/tmp/w/Temp", "/tmp/w/Temp"));
    }

    #[cfg(windows)]
    #[test]
    fn unix_paths_rejected_on_windows() {
        let prot = vec![r"C:\Windows".to_string()];
        assert!(!is_inside("/tmp/w/Temp/a", "/tmp/w/Temp"));
        assert!(!root_is_safe("/Windows", &prot, &[]));
    }

    #[test]
    fn root_is_safe_rules() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let prot = s(&[r"C:\", r"C:\Users\M", r"C:\Windows", r"C:\Windows\System32", r"C:\Program Files"]);
        let forb = s(&[r"C:\Users\M\Documents", r"D:\Descargas"]);
        assert!(root_is_safe(T, &prot, &forb));
        assert!(!root_is_safe(r"C:\Users\M", &prot, &forb));
        assert!(!root_is_safe(r"c:\users\m\", &prot, &forb));
        assert!(!root_is_safe(r"C:\", &prot, &forb));
        assert!(!root_is_safe(r"C:\Users", &prot, &forb));
        assert!(!root_is_safe(r"C:\Users\M\Documents\Temp", &prot, &forb));
        assert!(!root_is_safe(r"D:\", &prot, &forb));
        assert!(!root_is_safe(r"C:\Windows", &prot, &forb));
        assert!(root_is_safe(r"C:\Windows\Temp", &prot, &forb));
        assert!(!root_is_safe(r"..\Temp", &prot, &forb));
    }

    #[test]
    fn temp_name_rules() {
        assert!(temp_name_ok(T));
        assert!(temp_name_ok(r"C:\x\tmp"));
        assert!(temp_name_ok(r"C:\x\TEMP\"));
        assert!(temp_name_ok(r"C:\x\Temp\2"));
        assert!(!temp_name_ok(r"C:\x\Temp\abc"));
        assert!(!temp_name_ok(r"C:\x\Temp\2a"));
        assert!(!temp_name_ok(r"C:\Users\M"));
        assert!(!temp_name_ok(r"C:\x\Temporal"));
    }
}
