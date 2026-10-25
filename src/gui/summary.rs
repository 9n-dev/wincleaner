//! Sección Resumen: unidades y tarjeta de limpieza. Las demás tarjetas las rellenan sus secciones.

use std::rc::Rc;

use slint::{ComponentHandle, VecModel};
use wincleaner::format;
use wincleaner::ui::{AppWindow, DriveRow, SummaryData};

use crate::win;

/// «C:» → `C:`; `used` es la parte ocupada, y `low` marca poco espacio libre.
fn drive_row(d: &win::Drive) -> DriveRow {
    let used = if d.total == 0 { 0.0 } else { (1.0 - d.free as f64 / d.total as f64) as f32 };
    DriveRow {
        name: d.root.trim_end_matches('\\').into(),
        label: d.label.as_str().into(),
        text: format!("{} libres de {}", format::bytes(d.free), format::bytes(d.total)).into(),
        used,
        low: format::low_space(d.free, d.total),
    }
}

/// Lee las unidades y las enseña.
fn refresh_drives(app: &AppWindow) {
    let rows: Vec<DriveRow> = win::fixed_drives().iter().map(drive_row).collect();
    app.global::<SummaryData>().set_drives(Rc::new(VecModel::from(rows)).into());
}

/// Tarjeta de Limpieza. La llama el controlador de Limpiar cada vez que cambia su estado:
/// `bytes` y `cats` son los marcados con contenido; `calculating`, si aún se escanea.
pub fn set_clean(app: &AppWindow, bytes: u64, cats: usize, calculating: bool) {
    let d = app.global::<SummaryData>();
    let (total, text) = if calculating && bytes == 0 {
        ("—".into(), "Calculando…".to_string())
    } else if cats == 0 {
        (format::bytes(0), "no hay nada marcado que liberar".to_string())
    } else {
        let n = if cats == 1 { "1 categoría".to_string() } else { format!("{cats} categorías") };
        (format::bytes(bytes), format!("se pueden liberar en {n}"))
    };
    d.set_clean_total(total.into());
    d.set_clean_text(text.into());
}

/// Tarjeta de Programas: `title` es «86 instalados · 38,2 GB»; `biggest`, el que más ocupa.
pub fn set_programs(app: &AppWindow, title: &str, biggest: Option<(&str, u64)>) {
    let d = app.global::<SummaryData>();
    d.set_programs_title(title.into());
    let text = biggest.map(|(n, s)| format!("El más grande: {n} ({})", format::bytes(s)));
    d.set_programs_text(text.unwrap_or_default().into());
}

/// Tarjeta de Procesos: «128 procesos · 5,4 GB».
pub fn set_procs(app: &AppWindow, title: &str) {
    app.global::<SummaryData>().set_procs_title(title.into());
}

/// Segunda línea de la tarjeta de Procesos: «11 programas arrancan con Windows».
pub fn set_procs_text(app: &AppWindow, text: &str) {
    app.global::<SummaryData>().set_procs_text(text.into());
}

/// Tarjeta de Seguridad: «Sin revisar todavía» o el resultado, y el estado del antivirus.
pub fn set_security(app: &AppWindow, title: &str, text: &str) {
    let d = app.global::<SummaryData>();
    d.set_security_title(title.into());
    d.set_security_text(text.into());
}

/// Tarjeta de Unidades, línea de Espacio: lo que ocupan los juegos y lo que sobra en duplicados.
pub fn set_space(app: &AppWindow, games: Option<u64>, dups: Option<u64>) {
    let mut text = match games.filter(|&g| g > 0) {
        Some(g) => format!("{} en juegos", format::bytes(g)),
        None => "Mira a dónde va cada GB".to_string(),
    };
    if let Some(w) = dups.filter(|&w| w > 0) {
        if games.is_some_and(|g| g > 0) {
            text += &format!(" · sobran {} en duplicados", format::bytes(w));
        } else {
            text = format!("Sobran {} en duplicados", format::bytes(w));
        }
    }
    app.global::<SummaryData>().set_space_text(text.into());
}

/// Rehace la línea de Espacio con los datos que haya ahora.
pub fn refresh_space(app: &AppWindow) {
    set_space(app, super::games::total(), super::dups::waste());
}

/// Conecta la sección y lee las unidades por primera vez.
pub fn init(app: &AppWindow) {
    let d = app.global::<SummaryData>();
    let weak = app.as_weak();
    d.on_refresh(move || {
        if let Some(app) = weak.upgrade() {
            refresh_drives(&app);
        }
    });
    let weak = app.as_weak();
    // Mientras se limpia no se cambia de sección (la acción de accesibilidad tampoco).
    d.on_go(move |i| {
        if let Some(app) = weak.upgrade().filter(|a| !a.get_locked()) {
            app.set_section(i);
        }
    });
    refresh_drives(app);
    refresh_space(app);
}
