//! Vistas de Slint compiladas desde `ui/app.slint`, y los modales de confirmación.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, VecModel};

slint::include_modules!();

#[cfg(not(windows))]
pub mod headless;

thread_local! {
    // Acción del modal abierto. Se saca al confirmar o al cancelar, así que nunca se
    // ejecuta dos veces ni la hereda otro modal.
    static PENDING: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    // Acción del botón alternativo («Borrar del todo…») del modal abierto.
    static ALT: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

/// Contenido del modal.
pub struct Confirm<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub lines: Vec<ModalLine>,
    pub note: &'a str,
    pub confirm_text: &'a str,
    pub danger: bool,
    pub alt_text: &'a str,
}

/// Conecta el modal de `app` con `confirm`. Una vez, al crear la ventana: Slint no deja
/// cambiar un manejador mientras se ejecuta, así que el desarme es sacar la acción.
pub fn install_modal(app: &AppWindow) {
    let ok = || {
        drop(ALT.take());
        if let Some(f) = PENDING.take() {
            f();
        }
    };
    let cancel = || {
        drop(ALT.take());
        drop(PENDING.take());
    };
    let m = app.global::<Modal>();
    m.on_confirm(ok);
    m.on_cancel(cancel);
    m.on_alt(|| {
        drop(PENDING.take());
        if let Some(f) = ALT.take() {
            f();
        }
    });
    let p = app.global::<Purge>();
    p.on_confirm(ok);
    p.on_cancel(cancel);
}

/// Rellena el modal y lo abre. `on_ok` solo se ejecuta si se confirma este modal.
pub fn confirm(app: &AppWindow, c: Confirm, on_ok: impl FnOnce() + 'static) {
    confirm_with_alt(app, c, on_ok, || {});
}

/// Como `confirm`, con el botón `c.alt_text`: al pulsarlo se suelta `on_ok` y se ejecuta `on_alt`.
pub fn confirm_with_alt(app: &AppWindow, c: Confirm, on_ok: impl FnOnce() + 'static, on_alt: impl FnOnce() + 'static) {
    let m = app.global::<Modal>();
    m.set_title(c.title.into());
    m.set_text(c.text.into());
    m.set_lines(Rc::new(VecModel::from(c.lines)).into());
    m.set_note(c.note.into());
    m.set_confirm_text(c.confirm_text.into());
    m.set_danger(c.danger);
    m.set_alt_text(c.alt_text.into());
    PENDING.set(Some(Box::new(on_ok)));
    ALT.set(Some(Box::new(on_alt)));
    m.set_open(true);
}

/// Abre «¿Borrar del todo?» con `text` bajo «No se podrá recuperar.». `on_ok` solo se
/// ejecuta si se escribe BORRAR y se confirma.
pub fn purge(app: &AppWindow, lines: Vec<ModalLine>, text: &str, on_ok: impl FnOnce() + 'static) {
    let p = app.global::<Purge>();
    p.set_lines(Rc::new(VecModel::from(lines)).into());
    p.set_text(text.into());
    ALT.set(None);
    PENDING.set(Some(Box::new(on_ok)));
    p.set_open(true);
}
