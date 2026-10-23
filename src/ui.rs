//! Vistas de Slint compiladas desde `ui/app.slint`.

slint::include_modules!();

#[cfg(not(windows))]
pub mod headless;
