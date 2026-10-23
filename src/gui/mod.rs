//! Ventana principal: crea `AppWindow` y conecta la barra lateral.

use slint::ComponentHandle;
use wincleaner::ui::{AppWindow, Nav};

use crate::win;

pub fn run() {
    let Ok(app) = AppWindow::new() else { return };
    app.set_admin(win::is_admin());
    // Si se cancela el aviso de UAC, la ventana sigue abierta.
    app.global::<Nav>().on_relaunch_admin(|| {
        if win::relaunch_as_admin() {
            let _ = slint::quit_event_loop();
        }
    });
    let _ = app.run();
}
