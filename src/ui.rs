//! Vistas de Slint compiladas desde `ui/app.slint`, y los modales de confirmación.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, VecModel};

use crate::space::notice::{self, Act};

slint::include_modules!();

#[cfg(not(windows))]
pub mod headless;

/// Acciones de la ventana de aviso abierta, [izquierda, principal, grupos…], y a quién dárselas.
type NoticeActs = (Vec<Option<Act>>, Box<dyn Fn(Act)>);

thread_local! {
    // Acción del modal abierto. Se saca al confirmar o al cancelar, así que nunca se
    // ejecuta dos veces ni la hereda otro modal.
    static PENDING: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    // Acción del botón alternativo («Borrar del todo…») del modal abierto.
    static ALT: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    // Se sacan al pulsar o al cerrar, así que cada una se da una sola vez.
    static NOTICE: RefCell<Option<NoticeActs>> = RefCell::new(None);
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

    // Cierra la ventana y, después, da la acción `k`: puede abrir otro modal («Borrar del todo…»).
    let run = |w: slint::Weak<AppWindow>, k: usize| {
        move || {
            if let Some(app) = w.upgrade() {
                app.global::<Notice>().set_open(false);
            }
            if let Some((mut acts, on)) = NOTICE.take()
                && let Some(act) = acts.get_mut(k).and_then(Option::take)
            {
                on(act);
            }
        }
    };
    let n = app.global::<Notice>();
    n.on_left(run(app.as_weak(), 0));
    n.on_main(run(app.as_weak(), 1));
    let w = app.as_weak();
    n.on_group(move |i| run(w.clone(), 2 + i as usize)());
    n.on_close(|| drop(NOTICE.take()));
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

/// Rellena la ventana de aviso con `n` y la abre. Al pulsar un botón se cierra y, después,
/// se llama a `on` con su acción; «Cerrar», Intro y Esc no llaman a nada.
pub fn notice(app: &AppWindow, n: &notice::Notice, on: impl Fn(Act) + 'static) {
    let w = app.global::<Notice>();
    let act = |b: &Option<notice::Button>| b.as_ref().map(|b| b.act.clone());
    let text = |b: &Option<notice::Button>| b.as_ref().map_or("", |b| b.text.as_str()).into();
    let icon = |b: &Option<notice::Button>| b.as_ref().map_or("", |b| b.icon).into();
    w.set_title(n.title.as_str().into());
    w.set_text(n.text.as_str().into());
    w.set_done(n.done.as_str().into());
    w.set_head(n.head.as_str().into());
    w.set_left_text(text(&n.left));
    w.set_left_icon(icon(&n.left));
    w.set_close_text(n.close.into());
    w.set_main_text(text(&n.main));
    w.set_main_icon(icon(&n.main));
    let lines: Vec<ModalLine> = n.lines.iter().map(|(l, v)| ModalLine { label: l.as_str().into(), value: v.as_str().into() }).collect();
    w.set_lines(Rc::new(VecModel::from(lines)).into());
    let groups: Vec<NoticeGroup> = n
        .groups
        .iter()
        .map(|g| NoticeGroup {
            icon: g.icon.into(),
            title: g.title.as_str().into(),
            meta: g.meta.as_str().into(),
            names: g.names.as_str().into(),
            button: text(&g.button),
            button_icon: icon(&g.button),
        })
        .collect();
    w.set_groups(Rc::new(VecModel::from(groups)).into());
    let mut acts = vec![act(&n.left), act(&n.main)];
    acts.extend(n.groups.iter().map(|g| act(&g.button)));
    NOTICE.set(Some((acts, Box::new(on))));
    w.set_open(true);
}
