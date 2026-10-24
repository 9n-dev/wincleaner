//! Vistas de Slint compiladas desde `ui/app.slint`, y el modal de confirmación.

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
}

/// Contenido del modal.
pub struct Confirm<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub lines: Vec<ModalLine>,
    pub note: &'a str,
    pub confirm_text: &'a str,
    pub danger: bool,
}

/// Conecta el modal de `app` con `confirm`. Una vez, al crear la ventana: Slint no deja
/// cambiar un manejador mientras se ejecuta, así que el desarme es sacar la acción.
pub fn install_modal(app: &AppWindow) {
    let m = app.global::<Modal>();
    m.on_confirm(|| {
        if let Some(f) = PENDING.take() {
            f();
        }
    });
    m.on_cancel(|| drop(PENDING.take()));
}

/// Rellena el modal y lo abre. `on_ok` solo se ejecuta si se confirma este modal.
pub fn confirm(app: &AppWindow, c: Confirm, on_ok: impl FnOnce() + 'static) {
    let m = app.global::<Modal>();
    m.set_title(c.title.into());
    m.set_text(c.text.into());
    m.set_lines(Rc::new(VecModel::from(c.lines)).into());
    m.set_note(c.note.into());
    m.set_confirm_text(c.confirm_text.into());
    m.set_danger(c.danger);
    PENDING.set(Some(Box::new(on_ok)));
    m.set_open(true);
}
