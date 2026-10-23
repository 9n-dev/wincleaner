//! Intro y Esc siempre cancelan en el modal; solo se confirma con Espacio sobre «confirmar».
#![cfg(not(windows))]

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, SharedString};
use wincleaner::ui::{AppWindow, Modal, headless};

/// Abre el modal y devuelve (ventana, nº de confirmar, nº de cancelar).
fn abierto() -> (AppWindow, Rc<Cell<u32>>, Rc<Cell<u32>>) {
    headless::init();
    let app = AppWindow::new().unwrap();
    let (ok, no) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let m = app.global::<Modal>();
    let c = ok.clone();
    m.on_confirm(move || c.set(c.get() + 1));
    let c = no.clone();
    m.on_cancel(move || c.set(c.get() + 1));
    m.set_title("¿Borrar 3,0 GB?".into());
    m.set_danger(true);
    m.set_open(true);
    headless::render(app.window(), 1100, 720);
    (app, ok, no)
}

fn key(app: &AppWindow, k: &str) {
    headless::press(app.window(), SharedString::from(k));
    headless::render(app.window(), 1100, 720);
}

#[test]
fn enter_twice_never_confirms() {
    let (app, ok, no) = abierto();
    key(&app, "\n");
    key(&app, "\n");
    assert_eq!(ok.get(), 0);
    assert!(no.get() >= 1);
}

#[test]
fn escape_cancels() {
    let (app, ok, no) = abierto();
    key(&app, "\u{1b}");
    assert_eq!((ok.get(), no.get()), (0, 1));
    assert!(!app.global::<Modal>().get_open());
}

#[test]
fn tab_then_enter_still_cancels() {
    let (app, ok, no) = abierto();
    key(&app, "\t");
    key(&app, "\n");
    assert_eq!(ok.get(), 0);
    assert!(no.get() >= 1);
}

#[test]
fn tab_then_space_confirms() {
    let (app, ok, _) = abierto();
    key(&app, "\t");
    key(&app, " ");
    assert_eq!(ok.get(), 1);
}
