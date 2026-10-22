#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("WinCleaner solo funciona en Windows.");
}

#[cfg(windows)]
fn main() {}
