//! Ventana principal: crea `AppWindow`, conecta la barra lateral y monta las secciones.

mod clean;

use slint::ComponentHandle;
use wincleaner::ui::{AppWindow, Nav};

use crate::win;

pub fn run() {
    let Ok(app) = AppWindow::new() else { return };
    app.set_admin(win::is_admin());
    // Si se cancela el aviso de UAC, la ventana sigue abierta. Mientras se limpia, no
    // se reinicia (la acción de accesibilidad no mira `enabled`).
    let weak = app.as_weak();
    app.global::<Nav>().on_relaunch_admin(move || {
        if weak.upgrade().is_some_and(|a| !a.get_locked()) && win::relaunch_as_admin() {
            let _ = slint::quit_event_loop();
        }
    });
    let _clean = clean::Clean::new(&app);
    let _ = app.run();
}
