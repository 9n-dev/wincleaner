#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod gui;
#[cfg(windows)]
mod win;

#[cfg(not(windows))]
fn main() {
    eprintln!("WinCleaner solo funciona en Windows.");
}

#[cfg(windows)]
fn main() {
    gui::run();
}
