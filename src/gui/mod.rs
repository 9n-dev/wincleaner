//! Ventana principal: crea `AppWindow`, conecta la barra lateral y monta las secciones.

mod analyze;
mod clean;
mod programs;
mod summary;

use slint::{CloseRequestResponse, ComponentHandle};
use wincleaner::ui::{self, AppWindow, Nav};

use crate::win;

pub fn run() {
    let Ok(app) = AppWindow::new() else { return };
    app.set_admin(win::is_admin());
    ui::install_modal(&app);
    // Un solo manejador para toda la ventana (Slint no admite más): mientras una sección
    // tiene `locked` (p. ej. limpiando), la ventana no se cierra.
    let weak = app.as_weak();
    app.window().on_close_requested(move || {
        if weak.upgrade().is_some_and(|a| a.get_locked()) {
            CloseRequestResponse::KeepWindowShown
        } else {
            CloseRequestResponse::HideWindow
        }
    });
    // Si se cancela el aviso de UAC, la ventana sigue abierta. Mientras se limpia, no
    // se reinicia (la acción de accesibilidad no mira `enabled`).
    let weak = app.as_weak();
    app.global::<Nav>().on_relaunch_admin(move || {
        if weak.upgrade().is_some_and(|a| !a.get_locked()) && win::relaunch_as_admin() {
            let _ = slint::quit_event_loop();
        }
    });
    summary::init(&app);
    let _clean = clean::Clean::new(&app);
    analyze::Analyze::new(&app);
    programs::Programs::new(&app);
    let _ = app.run();
}
