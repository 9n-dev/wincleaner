//! Intro y Esc siempre cancelan en el modal; solo se confirma con Espacio sobre «confirmar».
#![cfg(not(windows))]

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, SharedString};
use wincleaner::ui::{AppWindow, Modal, Purge, headless};

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
    assert_eq!((ok.get(), no.get()), (0, 1));
    assert!(!app.global::<Modal>().get_open());
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
    assert_eq!((ok.get(), no.get()), (0, 1));
    assert!(!app.global::<Modal>().get_open());
}

#[test]
fn tab_then_space_confirms() {
    let (app, ok, _) = abierto();
    key(&app, "\t");
    key(&app, " ");
    assert_eq!(ok.get(), 1);
}

mod muestra {
    include!(concat!(env!("OUT_DIR"), "/muestra.rs"));
}

/// Con controles con foco detrás, Tab no saca el foco del modal: Intro sigue cancelando.
#[test]
fn tab_never_leaves_the_modal() {
    headless::init();
    let app = muestra::Muestra::new().unwrap();
    let (ok, no, fondo) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let m = app.global::<muestra::Modal>();
    let c = ok.clone();
    m.on_confirm(move || c.set(c.get() + 1));
    let c = no.clone();
    m.on_cancel(move || c.set(c.get() + 1));
    let c = fondo.clone();
    app.on_fondo(move || c.set(c.get() + 1));
    m.set_open(true);
    headless::render(app.window(), 1100, 720);
    for k in ["\t", "\t", "\t", "\n"] {
        headless::press(app.window(), SharedString::from(k));
        headless::render(app.window(), 1100, 720);
    }
    assert_eq!((ok.get(), no.get(), fondo.get()), (0, 1, 0));
    assert!(!m.get_open());
}

/// Tras confirmar o cancelar, el modal queda desarmado: confirmar otro modal sin acción
/// no repite la anterior, y cada `confirm` solo ejecuta la suya.
#[test]
fn confirm_runs_its_action_once() {
    use wincleaner::ui::{Confirm, confirm, install_modal};
    let ask = || Confirm { title: "¿Borrar?", text: "", lines: vec![], note: "", confirm_text: "Borrar", danger: true, alt_text: "" };
    headless::init();
    let app = AppWindow::new().unwrap();
    install_modal(&app);
    let m = app.global::<Modal>();
    let (a, b) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));

    let c = a.clone();
    confirm(&app, ask(), move || c.set(c.get() + 1));
    assert!(m.get_open());
    m.invoke_confirm();
    m.invoke_confirm();
    assert_eq!(a.get(), 1);

    let c = a.clone();
    confirm(&app, ask(), move || c.set(c.get() + 1));
    m.invoke_cancel();
    m.invoke_confirm();
    assert_eq!(a.get(), 1);

    let c = b.clone();
    confirm(&app, ask(), move || c.set(c.get() + 1));
    // Con el teclado, como el usuario: Tab al botón de confirmar y Espacio.
    headless::render(app.window(), 1100, 720);
    key(&app, "\t");
    key(&app, " ");
    assert_eq!((a.get(), b.get()), (1, 1));
    assert!(!m.get_open());
}

type Count = Rc<Cell<u32>>;

/// Con «Borrar del todo…»: devuelve (ventana, confirmar, cancelar, alt).
fn con_alt() -> (AppWindow, Count, Count, Count) {
    let (app, ok, no) = abierto();
    let alt = Rc::new(Cell::new(0));
    let m = app.global::<Modal>();
    let c = alt.clone();
    m.on_alt(move || c.set(c.get() + 1));
    m.set_open(false);
    m.set_alt_text("Borrar del todo…".into());
    headless::render(app.window(), 1100, 720);
    m.set_open(true);
    headless::render(app.window(), 1100, 720);
    (app, ok, no, alt)
}

#[test]
fn alt_reachable_with_tab_and_space() {
    let (app, ok, no, alt) = con_alt();
    key(&app, "\t");
    key(&app, "\t");
    key(&app, " ");
    assert_eq!((ok.get(), no.get(), alt.get()), (0, 0, 1));
    assert!(!app.global::<Modal>().get_open());
}

#[test]
fn alt_enter_still_cancels() {
    let (app, ok, no, alt) = con_alt();
    key(&app, "\t");
    key(&app, "\n");
    assert_eq!((ok.get(), no.get(), alt.get()), (0, 1, 0));
}

/// Abre «Borrar del todo» y devuelve (ventana, confirmar, cancelar).
fn purga() -> (AppWindow, Rc<Cell<u32>>, Rc<Cell<u32>>) {
    headless::init();
    let app = AppWindow::new().unwrap();
    let (ok, no) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let p = app.global::<Purge>();
    let c = ok.clone();
    p.on_confirm(move || c.set(c.get() + 1));
    let c = no.clone();
    p.on_cancel(move || c.set(c.get() + 1));
    p.set_text("No pasa por la papelera: se borra ya y libera 12,8 GB al momento.".into());
    p.set_open(true);
    headless::render(app.window(), 1100, 720);
    (app, ok, no)
}

fn escribe(app: &AppWindow, s: &str) {
    for ch in s.chars() {
        key(app, &ch.to_string());
    }
}

#[test]
fn purge_enter_in_field_cancels() {
    let (app, ok, no) = purga();
    escribe(&app, "BORRAR");
    key(&app, "\n");
    assert_eq!((ok.get(), no.get()), (0, 1));
    assert!(!app.global::<Purge>().get_open());
}

#[test]
fn purge_disabled_until_borrar() {
    let (app, ok, _) = purga();
    escribe(&app, "BORR");
    key(&app, "\t");
    key(&app, " ");
    assert_eq!(ok.get(), 0);
}

#[test]
fn purge_lowercase_ok() {
    let (app, ok, no) = purga();
    escribe(&app, "borrar");
    key(&app, "\t");
    key(&app, " ");
    assert_eq!((ok.get(), no.get()), (1, 0));
    assert!(!app.global::<Purge>().get_open());
}

#[test]
fn purge_escape_cancels() {
    let (app, ok, no) = purga();
    escribe(&app, "BORRAR");
    key(&app, "\u{1b}");
    assert_eq!((ok.get(), no.get()), (0, 1));
    assert!(!app.global::<Purge>().get_open());
}

/// «Borrar del todo…» suelta la acción de confirmar y ejecuta la suya; la de «¿Borrar del
/// todo?» solo corre una vez.
#[test]
fn alt_drops_confirm_and_purge_runs_once() {
    use wincleaner::ui::{Confirm, confirm_with_alt, install_modal, purge};
    headless::init();
    let app = AppWindow::new().unwrap();
    install_modal(&app);
    let (ok, alt, borrado) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let ask = Confirm { title: "¿Enviar a la papelera?", text: "", lines: vec![], note: "", confirm_text: "Enviar", danger: false, alt_text: "Borrar del todo…" };
    let (o, a, b, w) = (ok.clone(), alt.clone(), borrado.clone(), app.as_weak());
    confirm_with_alt(&app, ask, move || o.set(o.get() + 1), move || {
        a.set(a.get() + 1);
        let b = b.clone();
        purge(&w.unwrap(), vec![], "", move || b.set(b.get() + 1));
    });
    app.global::<Modal>().invoke_alt();
    app.global::<Modal>().invoke_confirm();
    assert_eq!((ok.get(), alt.get()), (0, 1));
    let p = app.global::<Purge>();
    assert!(p.get_open());
    p.invoke_confirm();
    p.invoke_confirm();
    assert_eq!(borrado.get(), 1);
}
