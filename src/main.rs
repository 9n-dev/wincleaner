#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod gui;
#[cfg(windows)]
#[expect(dead_code, reason = "las secciones que las usan vuelven en las Tasks 3–4")]
mod win;

#[cfg(not(windows))]
fn main() {
    eprintln!("WinCleaner solo funciona en Windows.");
}

#[cfg(windows)]
fn main() {
    gui::run();
}
