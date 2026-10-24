//! Orígenes de arranque y valores de `StartupApproved`.

use crate::programs::Hive;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    RunUser,
    RunMachine,
    RunMachine32,
    FolderUser,
    FolderCommon,
}

pub fn needs_admin(s: Source) -> bool {
    matches!(s, Source::RunMachine | Source::RunMachine32 | Source::FolderCommon)
}

/// Clave de `StartupApproved` (bajo `Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved`).
pub fn approved_key(s: Source) -> (Hive, &'static str) {
    match s {
        Source::RunUser => (Hive::User, r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"),
        Source::RunMachine => (Hive::Machine64, r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"),
        Source::RunMachine32 => (Hive::Machine64, r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32"),
        Source::FolderUser => (Hive::User, r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder"),
        Source::FolderCommon => (Hive::Machine64, r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder"),
    }
}

/// Sin valor (o vacío) es activado; si no, el bit 0 del primer byte: `02`/`06` activado, `03`/`07` desactivado.
pub fn is_enabled(v: Option<&[u8]>) -> bool {
    v.and_then(|b| b.first()).is_none_or(|b| b & 1 == 0)
}

/// Valor de 12 bytes: activado `02 00…`; desactivado `03 00 00 00` + FILETIME en little-endian.
pub fn encode(enabled: bool, now_filetime: u64) -> [u8; 12] {
    let mut v = [0u8; 12];
    if enabled {
        v[0] = 2;
    } else {
        v[0] = 3;
        v[4..].copy_from_slice(&now_filetime.to_le_bytes());
    }
    v
}

/// El ejecutable de un valor `Run`, tal como venía, permite decir «su archivo ya no existe» si no
/// está: solo con carpeta y con `.exe`. Un nombre suelto Windows lo busca también por `PATH`, y
/// sin extensión le añade `.exe`.
pub fn missing_is_sure(raw_exe: &str) -> bool {
    raw_exe.contains(['\\', '/']) && raw_exe.to_ascii_lowercase().ends_with(".exe")
}

pub fn where_text(s: Source) -> &'static str {
    match s {
        Source::RunUser => "Registro del usuario",
        Source::RunMachine | Source::RunMachine32 => "Registro del equipo",
        Source::FolderUser => "Carpeta Inicio · tu usuario",
        Source::FolderCommon => "Carpeta Inicio · todos los usuarios",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_only_with_full_path() {
        assert!(missing_is_sure(r"C:\P\a.EXE"));
        for no in ["a.exe", r"C:\P\a", "a", r"C:\P\a.exe.bak"] {
            assert!(!missing_is_sure(no), "{no}");
        }
    }

    #[test]
    fn approved_missing_or_short_is_enabled() {
        assert!(is_enabled(None));
        assert!(is_enabled(Some(&[])));
    }

    #[test]
    fn approved_variants() {
        assert!(is_enabled(Some(&[2, 0, 0, 0])));
        assert!(is_enabled(Some(&[6, 0, 0, 0])));
        assert!(!is_enabled(Some(&[3, 0, 0, 0])));
        assert!(!is_enabled(Some(&[7])));
    }

    #[test]
    fn encode_disabled_has_filetime() {
        let v = encode(false, 0x0102030405060708);
        assert_eq!(v[4..], [8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(v[0], 3);
        assert_eq!(encode(true, 99), [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn encode_roundtrip() {
        for b in [true, false] {
            assert_eq!(is_enabled(Some(&encode(b, 1))), b);
        }
    }

    #[test]
    fn machine_sources_need_admin() {
        assert!(needs_admin(Source::RunMachine));
        assert!(needs_admin(Source::RunMachine32));
        assert!(needs_admin(Source::FolderCommon));
        assert!(!needs_admin(Source::RunUser));
        assert!(!needs_admin(Source::FolderUser));
    }

    #[test]
    fn approved_keys_and_texts() {
        assert_eq!(approved_key(Source::RunUser).0, Hive::User);
        assert!(approved_key(Source::RunMachine32).1.ends_with("\\Run32"));
        assert!(approved_key(Source::FolderCommon).1.ends_with("\\StartupFolder"));
        assert_eq!(where_text(Source::FolderUser), "Carpeta Inicio · tu usuario");
    }
}
