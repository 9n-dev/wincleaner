//! Reglas puras de procesos: agrupar, bloquear, % de CPU y verificar antes de terminar.

use crate::paths::{is_inside, is_valid, strip_verbatim};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    /// Ejecutable sin ruta.
    pub name: String,
    /// Ruta completa, si se pudo leer.
    pub exe: Option<String>,
    pub cpu_100ns: u64,
    pub mem: u64,
    pub critical: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub key: String,
    pub name: String,
    pub exe: Option<String>,
    pub pids: Vec<u32>,
    pub cpu_100ns: u64,
    pub mem: u64,
    pub blocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillTarget {
    Kill,
    Gone,
    Mismatch,
}

/// Ante la duda, bloqueado: crítico, sin ruta conocida (o vacía), o con una ruta (o `system_root`)
/// que `paths` no sabe interpretar (`\\?\UNC`, `\Device\…`, `//?/…`), o dentro de `system_root`.
pub fn blocked(exe: Option<&str>, critical: bool, system_root: &str) -> bool {
    let Some(exe) = exe.filter(|e| !e.trim().is_empty()) else { return true };
    // Se quita el prefijo `\\?\` de ambas; `None` solo si es de red (UNC).
    let (Some(exe), Some(root)) = (norm(exe), norm(system_root)) else { return true };
    critical || !is_valid(exe) || !is_valid(root) || is_inside(exe, root)
}

fn norm(p: &str) -> Option<&str> {
    if p.starts_with(r"\\?\") { strip_verbatim(p) } else { Some(p) }
}

/// Agrupa por ruta del exe en minúsculas (o por nombre si no hay) y ordena por memoria, de mayor a menor.
pub fn group(v: Vec<ProcInfo>, system_root: &str) -> Vec<Group> {
    let mut idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Group> = Vec::new();
    // Un grupo se bloquea si se bloquea cualquiera de sus procesos.
    for p in v {
        let key = p.exe.as_deref().filter(|e| !e.is_empty()).unwrap_or(&p.name).to_ascii_lowercase();
        let b = blocked(p.exe.as_deref(), p.critical, system_root);
        let i = *idx.entry(key.clone()).or_insert_with(|| {
            out.push(Group { key, name: p.name.clone(), exe: p.exe.clone(), ..Group::default() });
            out.len() - 1
        });
        let g = &mut out[i];
        g.pids.push(p.pid);
        g.cpu_100ns = g.cpu_100ns.saturating_add(p.cpu_100ns);
        g.mem = g.mem.saturating_add(p.mem);
        g.blocked |= b;
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.mem));
    out
}

/// % de una CPU lógica media entre dos lecturas; 0 si no pasó tiempo y nunca más de 100.
pub fn cpu_percent(prev: u64, now: u64, elapsed_100ns: u64, ncpu: u32) -> f32 {
    if elapsed_100ns == 0 {
        return 0.0;
    }
    let total = elapsed_100ns as f64 * f64::from(ncpu.max(1));
    (now.saturating_sub(prev) as f64 / total * 100.0).min(100.0) as f32
}

/// Antes de terminar un PID se vuelve a mirar su ruta: si ya no existe, `Gone`;
/// si no es la misma (PID reutilizado), `Mismatch`. Solo coincidencia exacta (ASCII sin mayúsculas) da `Kill`.
pub fn kill_target(expected_exe: &str, current_exe: Option<&str>) -> KillTarget {
    match current_exe {
        None => KillTarget::Gone,
        Some(c) if c.eq_ignore_ascii_case(expected_exe) => KillTarget::Kill,
        Some(_) => KillTarget::Mismatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, name: &str, exe: Option<&str>, mem: u64) -> ProcInfo {
        ProcInfo { pid, name: name.into(), exe: exe.map(Into::into), mem, ..Default::default() }
    }

    #[test]
    fn groups_by_exe_case_insensitive() {
        let g = group(
            vec![
                p(1, "Chrome.exe", Some("C:\\P\\Chrome.exe"), 10),
                p(2, "chrome.exe", Some("c:\\p\\chrome.exe"), 5),
            ],
            "C:\\Windows",
        );
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].pids, vec![1, 2]);
        assert_eq!(g[0].mem, 15);
    }

    #[test]
    fn groups_sorted_by_memory_and_fall_back_to_name() {
        let g = group(vec![p(1, "a.exe", None, 1), p(2, "b.exe", Some("C:\\P\\b.exe"), 9)], "C:\\Windows");
        assert_eq!(g[0].name, "b.exe");
        assert!(g[1].blocked);
        assert!(!g[0].blocked);
    }

    #[test]
    fn system_root_and_critical_are_blocked() {
        assert!(blocked(Some("C:\\Windows\\explorer.exe"), false, "C:\\Windows"));
        assert!(!blocked(Some("C:\\Windowsx\\a.exe"), false, "C:\\Windows"));
        assert!(blocked(None, false, "C:\\Windows"));
        assert!(blocked(Some("C:\\P\\a.exe"), true, "C:\\Windows"));
        assert!(blocked(Some("C:\\P\\a.exe"), false, ""));
        assert!(blocked(Some(""), false, "C:\\Windows"));
        let r = "C:\\Windows";
        assert!(blocked(Some(r"\\?\C:\Windows\x.exe"), false, r));
        assert!(blocked(Some(r"\Device\HarddiskVolume3\Windows\x.exe"), false, r));
        assert!(blocked(Some("//?/C:/Windows/x.exe"), false, r));
        assert!(blocked(Some(r"\\?\UNC\srv\x.exe"), false, r));
        assert!(!blocked(Some(r"\\?\C:\P\a.exe"), false, r));
        assert!(!blocked(Some(r"C:\P\a.exe"), false, r"\\?\C:\Windows"));
        assert!(blocked(Some(r"C:\Windows\a.exe"), false, r"\\?\C:\Windows"));
    }

    #[test]
    fn cpu_percent_bounds() {
        assert_eq!(cpu_percent(0, 10_000_000, 10_000_000, 4), 25.0);
        assert_eq!(cpu_percent(5, 5, 0, 4), 0.0);
        assert_eq!(cpu_percent(0, u64::MAX, 1, 4), 100.0);
        assert_eq!(cpu_percent(10, 5, 100, 0), 0.0);
    }

    #[test]
    fn kill_target_gone() {
        assert_eq!(kill_target("C:\\a.exe", None), KillTarget::Gone);
    }

    #[test]
    fn kill_target_mismatch() {
        assert_eq!(kill_target("C:\\a.exe", Some("C:\\b.exe")), KillTarget::Mismatch);
    }

    #[test]
    fn kill_target_same_ignores_case() {
        assert_eq!(kill_target("C:\\A.exe", Some("c:\\a.EXE")), KillTarget::Kill);
    }
}
