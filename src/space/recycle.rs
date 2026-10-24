//! Papelera: ruta del `$I` de un `$R`, motivos de fallo y textos del resultado.

use crate::format::bytes;

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
    Other(i32),
}

pub fn reason(hr: i32) -> Reason {
    match hr as u32 {
        0x8007_0020 | 0x8007_0021 => Reason::InUse,
        0x8007_0005 => Reason::Denied,
        _ => Reason::Other(hr),
    }
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub names: Vec<String>,
    pub sent: u64,
    pub left: u64,
    pub failed: Vec<Reason>,
}

/// `(ok, título, texto)` del aviso que sale al terminar de enviar a la papelera o de borrar
/// del todo (`purge`). Si ha fallado algo, el texto explica el motivo más importante.
pub fn outcome_text(o: &Outcome, purge: bool) -> (bool, String, String) {
    let what = match o.names.as_slice() {
        [one] => one.clone(),
        names => format!("{} elementos", names.len()),
    };
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
    let main = [Reason::NoBin, Reason::InUse, Reason::Denied, Reason::Protected]
        .into_iter()
        .find(|r| o.failed.contains(r))
        .unwrap_or(o.failed[0]);
    let n = o.failed.iter().filter(|r| **r == main).count();
    let one = n == 1;
    let pl = |sing: &str, plur: &str| if one { sing.to_string() } else { plur.to_string() };
    let noun = |w: &str| if one { format!("1 {w}") } else { format!("{n} {w}s") };
    let files = noun("archivo");
    let (cause, advice) = match main {
        Reason::NoBin => (
            format!("{} no {} en la papelera o su unidad no tiene", noun("elemento"), pl("cabe", "caben")),
            "Si quieres liberar el espacio igualmente, usa «Borrar del todo…».".to_string(),
        ),
        Reason::InUse => (
            format!("{files} {} en uso por otro programa", pl("está", "están")),
            pl("Cierra el programa que lo use y vuelve a intentarlo.", "Cierra los programas que los usen y vuelve a intentarlo."),
        ),
        Reason::Denied => (
            format!("{files} no se {} borrar por falta de permisos", pl("puede", "pueden")),
            "Reinicia WinCleaner como administrador y vuelve a intentarlo.".to_string(),
        ),
        Reason::Protected => (
            format!("{files} {} en una carpeta protegida", pl("está", "están")),
            "WinCleaner no borra nada en las carpetas del sistema.".to_string(),
        ),
        Reason::Other(_) => (format!("{files} no se {} podido borrar", pl("ha", "han")), "Vuelve a intentarlo.".to_string()),
    };
    let rest = o.failed.len() - n;
    let rest = if rest > 0 { format!(" y {rest} más no se han podido borrar") } else { String::new() };
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

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn outcome(names: &[&str], sent: u64, left: u64, failed: Vec<Reason>) -> Outcome {
        Outcome { names: names.iter().map(|s| s.to_string()).collect(), sent, left, failed }
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
            outcome_text(&o, false),
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
        let (ok, title, _) = outcome_text(&o, false);
        assert!(ok);
        assert_eq!(title, "Enviados a la papelera: 5 elementos (6 GB).");
    }

    #[test]
    fn outcome_partial_in_use() {
        let o = outcome(&["OBS"], 38 * GB, 3_435_973_837, vec![Reason::InUse; 3]);
        assert_eq!(
            outcome_text(&o, false),
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
        let (ok, _, text) = outcome_text(&o, false);
        assert!(!ok);
        assert!(text.contains("no cabe en la papelera"), "{text}");
        assert!(text.contains("Borrar del todo…"), "{text}");
    }

    #[test]
    fn outcome_purge_ok() {
        let o = outcome(&["boda_prima_4K.mov"], 13_743_895_347, 0, vec![]);
        let (ok, title, text) = outcome_text(&o, true);
        assert!(ok);
        assert!(title.starts_with("Borrado del todo:"), "{title}");
        assert!(text.contains("12,8 GB"), "{text}");
    }
}
