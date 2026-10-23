#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod gui;
#[cfg(windows)]
#[expect(dead_code, reason = "unidades y Explorador vuelven con el Resumen y Analizar")]
mod win;

#[cfg(not(windows))]
fn main() {
    eprintln!("WinCleaner solo funciona en Windows.");
}

#[cfg(windows)]
fn main() {
    gui::run();
}
