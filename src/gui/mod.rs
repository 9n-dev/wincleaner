//! Ventana principal: crea `AppWindow`, conecta la barra lateral y monta las secciones.

mod clean;
mod games;
mod procs;
mod programs;
mod security;
mod space;
mod summary;

use slint::{CloseRequestResponse, ComponentHandle};
use wincleaner::ui::{self, AppWindow, Nav};

use crate::win;

pub fn run() {
    // El icono del .exe (recurso 1 de `wincleaner.rc`) también en la ventana y la barra de tareas.
    let _ = slint::BackendSelector::new()
        .with_winit_window_attributes_hook(|attrs| {
            use slint::winit_030::winit::dpi::PhysicalSize;
            use slint::winit_030::winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
            use slint::winit_030::winit::window::Icon;
            use winapi::um::winuser::{GetSystemMetrics, SM_CXSMICON};
            let small = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
            attrs
                .with_window_icon(Icon::from_resource(1, Some(PhysicalSize::new(small, small))).ok())
                .with_taskbar_icon(Icon::from_resource(1, None).ok())
        })
        .select();
    let Ok(app) = AppWindow::new() else { return };
    app.set_admin(win::is_admin());
    app.set_version(env!("CARGO_PKG_VERSION").into());
    app.global::<Nav>().on_open_slint(|| win::open_url("https://slint.dev"));
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
    let programs = programs::Programs::new(&app);
    let space = space::Space::new(&app);
    games::Games::new(&app, programs, space);
    procs::Procs::new(&app);
    security::Security::new(&app);
    let _ = app.run();
    // Un análisis de Defender en marcha no se queda suelto al cerrar o al reiniciar como admin.
    security::shutdown();
}
