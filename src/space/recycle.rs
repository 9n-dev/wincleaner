//! Papelera: ruta del `$I` de un `$R`, motivos de fallo y textos del resultado.

use std::path::PathBuf;

use crate::format::bytes;
use crate::paths::{is_inside, same};

/// `$R…` → `$I…` en la misma carpeta, solo si esa carpeta es la de un usuario dentro de
/// `$Recycle.Bin` (`X:\$Recycle.Bin\<SID>\$R…`).
pub fn info_path(binned: &str) -> Option<String> {
    let (dir, name) = binned.rsplit_once('\\')?;
    let rest = name.strip_prefix("$R")?;
    let (bin, _sid) = dir.rsplit_once('\\')?;
    let bin_name = bin.rsplit('\\').next()?;
    bin_name.eq_ignore_ascii_case("$Recycle.Bin").then(|| format!("{dir}\\$I{rest}"))
}

/// `TSF_DELETE_RECYCLE_IF_POSSIBLE`: el elemento cabe en la papelera.
pub fn may_recycle(flags: u32) -> bool {
    flags & 0x80 != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    InUse,
    Denied,
    /// No cabe en la papelera o la unidad no tiene.
    NoBin,
    /// Zona protegida (o la ruta final no coincide con la escrita).
    Protected,
    /// Duplicados: la copia ya no es igual a la que se conserva (o una de las dos ha desaparecido).
    Changed,
    /// Windows lo ha borrado sin pasar por la papelera (no se puede deshacer): ya no existe.
    Purged,
    /// Enlace o junction: no se sigue ni se borra.
    Link,
    Other(i32),
}

pub fn reason(hr: i32) -> Reason {
    match hr as u32 {
        0x8007_0020 | 0x8007_0021 => Reason::InUse,
        0x8007_0005 => Reason::Denied,
        _ => Reason::Other(hr),
    }
}

/// Motivo a partir del código de error del sistema (`None`: lo ha parado una regla).
pub fn os_reason(code: Option<i32>) -> Reason {
    match code {
        Some(32 | 33) => Reason::InUse,
        Some(5) => Reason::Denied,
        None => Reason::Protected,
        Some(c) => Reason::Other(c),
    }
}

/// Un elemento que no se ha podido borrar: nombre que se enseña, lo que queda de él y el motivo.
#[derive(Clone, Debug, PartialEq)]
pub struct Fail {
    pub name: String,
    pub size: u64,
    pub reason: Reason,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub names: Vec<String>,
    pub sent: u64,
    pub left: u64,
    pub failed: Vec<Fail>,
    /// Programas que tienen abierto lo que falló por estar en uso: `(pid, nombre)`.
    pub users: Vec<(u32, String)>,
    /// Nada se envió: todo había desaparecido antes (otro programa lo borró).
    pub missing: bool,
}

/// `(título, texto)` del aviso verde al terminar de enviar a la papelera o de borrar del todo
/// (`purge`) sin ningún fallo. Los fallos y lo que ya no existía los explica `notice::failed`.
pub fn done_text(o: &Outcome, purge: bool) -> (String, String) {
    let what = what(&o.names);
    if purge {
        (format!("Borrado del todo: {what}."), format!("Se han liberado {}.", bytes(o.sent)))
    } else {
        (
            format!("Enviado{} a la papelera: {what} ({}).", if o.names.len() == 1 { "" } else { "s" }, bytes(o.sent)),
            "Vacía la papelera en «Limpiar» cuando quieras liberar el espacio.".into(),
        )
    }
}

/// Posiciones de `items` que hay que enviar: sin repetidos (se queda el primero) ni lo que está
/// dentro de otro de la lista, que ya va con él. Una ruta no Unicode se deja (la papelera la rechaza).
// ponytail: O(n²) comparaciones; ordenar por ruta si llegan decenas de miles.
pub fn outermost(items: &[PathBuf]) -> Vec<usize> {
    let s: Vec<Option<&str>> = items.iter().map(|p| p.to_str()).collect();
    (0..items.len())
        .filter(|&i| {
            let Some(a) = s[i] else { return true };
            !s.iter().enumerate().any(|(j, b)| b.is_some_and(|b| (j < i && same(a, b)) || is_inside(a, b)))
        })
        .collect()
}

/// «Uno» o «N elementos».
fn what(names: &[String]) -> String {
    match names {
        [one] => one.clone(),
        names => format!("{} elementos", names.len()),
    }
}

/// `(ok, título, texto)` del aviso al deshacer: `failed`, los nombres de lo que no ha vuelto.
pub fn undo_text(names: &[String], failed: &[String]) -> (bool, String, String) {
    match failed {
        [] if names.len() == 1 => (true, format!("Restaurado: {}.", what(names)), String::new()),
        [] => (true, format!("Restaurados: {}.", what(names)), String::new()),
        [one] => (false, format!("No se ha podido restaurar «{one}»."), "Búscalo en la papelera.".into()),
        many => (
            false,
            format!("No se han podido restaurar {} elementos: {}.", many.len(), many.join(", ")),
            "Búscalos en la papelera.".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn outcome(names: &[&str], sent: u64) -> Outcome {
        Outcome { names: names.iter().map(|s| s.to_string()).collect(), sent, ..Default::default() }
    }

    #[test]
    fn os_reasons() {
        assert_eq!(os_reason(Some(32)), Reason::InUse);
        assert_eq!(os_reason(Some(33)), Reason::InUse);
        assert_eq!(os_reason(Some(5)), Reason::Denied);
        assert_eq!(os_reason(None), Reason::Protected);
        assert_eq!(os_reason(Some(1234)), Reason::Other(1234));
    }

    #[test]
    fn info_path_rules() {
        assert_eq!(
            info_path(r"C:\$Recycle.Bin\S-1-5-21-1\$RAB12CD.mov").as_deref(),
            Some(r"C:\$Recycle.Bin\S-1-5-21-1\$IAB12CD.mov")
        );
        assert_eq!(info_path(r"C:\Users\a\$Rx.mov"), None);
        assert_eq!(info_path(r"C:\$Recycle.Bin\S-1\$IAB.mov"), None);
    }

    #[test]
    fn may_recycle_needs_flag() {
        assert!(may_recycle(0x80));
        assert!(may_recycle(0x81));
        assert!(!may_recycle(0));
    }

    #[test]
    fn reasons() {
        assert_eq!(reason(0x80070020_u32 as i32), Reason::InUse);
        assert_eq!(reason(0x80070021_u32 as i32), Reason::InUse);
        assert_eq!(reason(0x80070005_u32 as i32), Reason::Denied);
        assert_eq!(reason(-1), Reason::Other(-1));
    }

    #[test]
    fn done_text_one() {
        let o = outcome(&["boda_prima_4K.mov"], 13_743_895_347);
        assert_eq!(
            done_text(&o, false),
            (
                "Enviado a la papelera: boda_prima_4K.mov (12,8 GB).".into(),
                "Vacía la papelera en «Limpiar» cuando quieras liberar el espacio.".into()
            )
        );
    }

    #[test]
    fn done_text_many() {
        let o = outcome(&["a", "b", "c", "d", "e"], 6 * GB);
        let (title, _) = done_text(&o, false);
        assert_eq!(title, "Enviados a la papelera: 5 elementos (6 GB).");
    }

    #[test]
    fn outermost_drops_duplicates_and_nested() {
        let items: Vec<PathBuf> = [r"C:\a\x.iso", r"C:\b", r"c:/a/x.iso", r"C:\b\c\d.txt", r"C:\bb", r"C:\b\"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(outermost(&items), [0, 1, 4]);
        assert_eq!(outermost(&[]), Vec::<usize>::new());
    }

    #[test]
    fn undo_texts() {
        let names = vec!["boda_prima_4K.mov".to_string()];
        assert_eq!(undo_text(&names, &[]), (true, "Restaurado: boda_prima_4K.mov.".into(), String::new()));
        assert_eq!(
            undo_text(&names, &names),
            (false, "No se ha podido restaurar «boda_prima_4K.mov».".into(), "Búscalo en la papelera.".into())
        );
        let many: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        assert_eq!(undo_text(&many, &[]).1, "Restaurados: 3 elementos.");
        assert_eq!(
            undo_text(&many, &many[1..]),
            (false, "No se han podido restaurar 2 elementos: b, c.".into(), "Búscalos en la papelera.".into())
        );
    }

    #[test]
    fn done_text_purge() {
        let o = outcome(&["boda_prima_4K.mov"], 13_743_895_347);
        let (title, text) = done_text(&o, true);
        assert!(title.starts_with("Borrado del todo:"), "{title}");
        assert!(text.contains("12,8 GB"), "{text}");
    }
}
