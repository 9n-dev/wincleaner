#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(not(windows))]
fn main() {
    eprintln!("WinCleaner solo funciona en Windows.");
}

#[cfg(windows)]
fn main() {
    let _ = win::gather_env();
}
