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

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub names: Vec<String>,
    pub sent: u64,
    pub left: u64,
    pub failed: Vec<Reason>,
    /// Nada se envió: todo había desaparecido antes (otro programa lo borró).
    pub missing: bool,
}

/// `(ok, título, texto)` del aviso que sale al terminar de enviar a la papelera o de borrar
/// del todo (`purge`). Si ha fallado algo, el texto explica el motivo más importante. `alt`:
/// quien lo pidió ofrece «Borrar del todo…» (solo Carpetas).
pub fn outcome_text(o: &Outcome, purge: bool, alt: bool) -> (bool, String, String) {
    let what = what(&o.names);
    if o.missing {
        return (true, format!("Ya no existía{}: {what}.", if o.names.len() == 1 { "" } else { "n" }), String::new());
    }
    if o.failed.is_empty() {
        return match (purge, o.names.len() == 1) {
            (false, one) => (
                true,
                format!("Enviado{} a la papelera: {what} ({}).", if one { "" } else { "s" }, bytes(o.sent)),
                "Vacía la papelera en «Limpiar» cuando quieras liberar el espacio.".into(),
            ),
            (true, _) => (true, format!("Borrado del todo: {what}."), format!("Se han liberado {}.", bytes(o.sent))),
        };
    }
    let title = match (o.names.as_slice(), o.sent > 0) {
        ([one], true) => format!("No se ha podido borrar todo «{one}»."),
        ([one], false) => format!("No se ha podido borrar «{one}»."),
        (_, true) => format!("No se ha podido borrar todo: {what}."),
        (_, false) => format!("No se ha podido borrar: {what}."),
    };
    // El motivo que más importa decide la explicación; el resto se resume en «y N más».
    let main = [Reason::Purged, Reason::NoBin, Reason::InUse, Reason::Denied, Reason::Protected, Reason::Changed]
        .into_iter()
        .find(|r| o.failed.contains(r))
        .unwrap_or(o.failed[0]);
    let n = o.failed.iter().filter(|r| **r == main).count();
    let one = n == 1;
    let pl = |sing: &str, plur: &str| if one { sing.to_string() } else { plur.to_string() };
    let noun = |w: &str| if one { format!("1 {w}") } else { format!("{n} {w}s") };
    let files = noun("archivo");
    // Protegido u otro motivo se cuentan por elemento elegido (carpeta o archivo), no por archivo.
    let items = noun("elemento");
    let rest = o.failed.len() - n;
    let more = if rest == 1 { "1 más no se ha podido borrar".to_string() } else { format!("{rest} más no se han podido borrar") };
    if main == Reason::Purged {
        let title = match o.names.as_slice() {
            [one] => format!("Se ha borrado del todo «{one}»."),
            _ => format!("Se {} borrado del todo {n} de {what}.", pl("ha", "han")),
        };
        let more = if rest > 0 { format!(" Además, {more}.") } else { String::new() };
        return (false, title, format!("Windows {} ha borrado sin pasar por la papelera: no se puede deshacer.{more}", pl("lo", "los")));
    }
    let (cause, advice) = match main {
        Reason::NoBin => (
            format!("{} no {} en la papelera o su unidad no tiene", noun("elemento"), pl("cabe", "caben")),
            if alt {
                "Si quieres liberar el espacio igualmente, usa «Borrar del todo…».".to_string()
            } else {
                format!("Si quieres liberar el espacio igualmente, {} en Carpetas y usa «Borrar del todo…».", pl("búscalo", "búscalos"))
            },
        ),
        Reason::InUse => (
            format!("{files} {} en uso por otro programa", pl("está", "están")),
            pl("Cierra el programa que lo use y vuelve a intentarlo.", "Cierra los programas que los usen y vuelve a intentarlo."),
        ),
        Reason::Denied => (
            format!("{files} no se {} borrar por falta de permisos", pl("puede", "pueden")),
            "Reinicia WinCleaner como administrador y vuelve a intentarlo.".to_string(),
        ),
        Reason::Protected | Reason::Link => (
            format!("{items} {} en una carpeta protegida", pl("está", "están")),
            "WinCleaner no borra nada en las carpetas del sistema.".to_string(),
        ),
        Reason::Changed => (
            format!("{items} {} cambiado desde la búsqueda", pl("ha", "han")),
            "Vuelve a buscar duplicados.".to_string(),
        ),
        Reason::Other(_) | Reason::Purged => (format!("{items} no se {} podido borrar", pl("ha", "han")), "Vuelve a intentarlo.".to_string()),
    };
    let rest = if rest > 0 { format!(" y {more}") } else { String::new() };
    let mut parts = Vec::new();
    if o.sent > 0 {
        parts.push(if purge {
            format!("se han liberado {}", bytes(o.sent))
        } else {
            format!("{} se han enviado a la papelera", bytes(o.sent))
        });
    }
    if o.left > 0 {
        parts.push(format!("quedan {}", bytes(o.left)));
    }
    let paren = if parts.is_empty() { String::new() } else { format!(" ({})", parts.join("; ")) };
    (false, title, format!("{cause}{rest}{paren}. {advice}"))
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

    fn outcome(names: &[&str], sent: u64, left: u64, failed: Vec<Reason>) -> Outcome {
        Outcome { names: names.iter().map(|s| s.to_string()).collect(), sent, left, failed, missing: false }
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
    fn outcome_ok_one() {
        let o = outcome(&["boda_prima_4K.mov"], 13_743_895_347, 0, vec![]);
        assert_eq!(
            outcome_text(&o, false, true),
            (
                true,
                "Enviado a la papelera: boda_prima_4K.mov (12,8 GB).".into(),
                "Vacía la papelera en «Limpiar» cuando quieras liberar el espacio.".into()
            )
        );
    }

    #[test]
    fn outcome_ok_many() {
        let o = outcome(&["a", "b", "c", "d", "e"], 6 * GB, 0, vec![]);
        let (ok, title, _) = outcome_text(&o, false, true);
        assert!(ok);
        assert_eq!(title, "Enviados a la papelera: 5 elementos (6 GB).");
    }

    #[test]
    fn outcome_partial_in_use() {
        let o = outcome(&["OBS"], 38 * GB, 3_435_973_837, vec![Reason::InUse; 3]);
        assert_eq!(
            outcome_text(&o, false, true),
            (
                false,
                "No se ha podido borrar todo «OBS».".into(),
                "3 archivos están en uso por otro programa (38 GB se han enviado a la papelera; quedan 3,2 GB). \
                 Cierra los programas que los usen y vuelve a intentarlo."
                    .into()
            )
        );
    }

    #[test]
    fn outcome_no_bin() {
        let o = outcome(&["juego.iso"], 0, 40 * GB, vec![Reason::NoBin]);
        let (ok, _, text) = outcome_text(&o, false, true);
        assert!(!ok);
        assert!(text.contains("no cabe en la papelera"), "{text}");
        assert!(text.contains("Borrar del todo…"), "{text}");
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
    fn outcome_purge_failed_counts_items() {
        let o = outcome(&["OBS"], 38 * GB, 3 * GB, vec![Reason::Other(0)]);
        let (ok, _, text) = outcome_text(&o, true, true);
        assert!(!ok);
        assert!(text.starts_with("1 elemento no se ha podido borrar (se han liberado 38 GB; quedan 3 GB)."), "{text}");
        let o = outcome(&["a", "b"], 0, 2 * GB, vec![Reason::Protected; 2]);
        assert!(outcome_text(&o, false, true).2.starts_with("2 elementos están en una carpeta protegida"));
    }

    #[test]
    fn outcome_changed() {
        let o = outcome(&["a.mp4", "b.mp4"], GB, GB, vec![Reason::Changed]);
        let (ok, _, text) = outcome_text(&o, false, true);
        assert!(!ok);
        assert!(text.starts_with("1 elemento ha cambiado desde la búsqueda (1 GB se han enviado"), "{text}");
        assert!(text.ends_with("Vuelve a buscar duplicados."), "{text}");
    }

    #[test]
    fn outcome_no_bin_without_purge_option() {
        // Duplicados e Instaladores no tienen «Borrar del todo…»: se manda a Carpetas.
        let o = outcome(&["juego.iso"], 0, 40 * GB, vec![Reason::NoBin]);
        let (ok, _, text) = outcome_text(&o, false, false);
        assert!(!ok);
        assert!(text.contains("no cabe en la papelera"), "{text}");
        assert!(text.contains("Carpetas"), "{text}");
    }

    #[test]
    fn outcome_purged_by_shell() {
        let o = outcome(&["boda.mov"], GB, 0, vec![Reason::Purged]);
        let (ok, title, text) = outcome_text(&o, false, true);
        assert!(!ok);
        assert_eq!(title, "Se ha borrado del todo «boda.mov».");
        assert!(text.contains("sin pasar por la papelera") && text.contains("no se puede deshacer"), "{text}");
        let o = outcome(&["a", "b", "c"], GB, GB, vec![Reason::InUse, Reason::Purged]);
        let (ok, title, text) = outcome_text(&o, false, true);
        assert!(!ok);
        assert_eq!(title, "Se ha borrado del todo 1 de 3 elementos.");
        assert!(text.ends_with("no se puede deshacer. Además, 1 más no se ha podido borrar."), "{text}");
    }

    #[test]
    fn outcome_rest_agrees() {
        let o = outcome(&["a", "b", "c"], 0, GB, vec![Reason::NoBin, Reason::InUse]);
        assert!(outcome_text(&o, false, true).2.contains(" y 1 más no se ha podido borrar"));
        let o = outcome(&["a", "b", "c"], 0, GB, vec![Reason::NoBin, Reason::InUse, Reason::Denied]);
        assert!(outcome_text(&o, false, true).2.contains(" y 2 más no se han podido borrar"));
    }

    #[test]
    fn outcome_all_missing() {
        let o = Outcome { missing: true, ..outcome(&["x.iso"], 0, 0, vec![]) };
        assert_eq!(outcome_text(&o, false, true), (true, "Ya no existía: x.iso.".into(), String::new()));
        let o = Outcome { missing: true, ..outcome(&["a", "b"], 0, 0, vec![]) };
        assert_eq!(outcome_text(&o, true, true).1, "Ya no existían: 2 elementos.");
    }

    #[test]
    fn outcome_purge_ok() {
        let o = outcome(&["boda_prima_4K.mov"], 13_743_895_347, 0, vec![]);
        let (ok, title, text) = outcome_text(&o, true, true);
        assert!(ok);
        assert!(title.starts_with("Borrado del todo:"), "{title}");
        assert!(text.contains("12,8 GB"), "{text}");
    }
}
