//! Ventana de aviso: Intro y Esc solo cierran; «Reintentar» e «Ir a…» solo con Espacio o clic.
#![cfg(not(windows))]

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, SharedString, VecModel};
use wincleaner::ui::{AppWindow, Notice, NoticeGroup, headless};

type Count = Rc<Cell<u32>>;

/// Abre el aviso y devuelve (ventana, main, left, group, close).
fn abierto() -> (AppWindow, Count, Count, Count, Count) {
    headless::init();
    let app = AppWindow::new().unwrap();
    let n: [Count; 4] = Default::default();
    let w = app.global::<Notice>();
    let c = n[0].clone();
    w.on_main(move || c.set(c.get() + 1));
    let c = n[1].clone();
    w.on_left(move || c.set(c.get() + 1));
    let c = n[2].clone();
    w.on_group(move |_| c.set(c.get() + 1));
    let c = n[3].clone();
    w.on_close(move || c.set(c.get() + 1));
    w.set_title("No se ha podido borrar: está en uso".into());
    w.set_close_text("Cerrar".into());
    w.set_main_text("Reintentar".into());
    w.set_left_text("Ir a Procesos".into());
    let g = NoticeGroup { title: "En uso por OBS Studio".into(), button: "Ir a Procesos".into(), ..Default::default() };
    w.set_groups(Rc::new(VecModel::from(vec![g])).into());
    w.set_open(true);
    headless::render(app.window(), 1100, 720);
    let [main, left, group, close] = n;
    (app, main, left, group, close)
}

fn key(app: &AppWindow, k: &str) {
    headless::press(app.window(), SharedString::from(k));
    headless::render(app.window(), 1100, 720);
}

#[test]
fn enter_never_acts() {
    let (app, main, left, group, close) = abierto();
    key(&app, "\n");
    key(&app, "\n");
    assert_eq!((main.get(), left.get(), group.get(), close.get()), (0, 0, 0, 1));
    assert!(!app.global::<Notice>().get_open());
}

#[test]
fn escape_closes() {
    let (app, main, left, _, close) = abierto();
    key(&app, "\u{1b}");
    assert_eq!((main.get(), left.get(), close.get()), (0, 0, 1));
    assert!(!app.global::<Notice>().get_open());
}

#[test]
fn tab_to_main_then_enter_still_closes() {
    let (app, main, _, _, close) = abierto();
    key(&app, "\t");
    key(&app, "\n");
    assert_eq!((main.get(), close.get()), (0, 1));
    assert!(!app.global::<Notice>().get_open());
}

#[test]
fn tab_to_main_then_space_acts() {
    let (app, main, left, _, _) = abierto();
    key(&app, "\t");
    key(&app, " ");
    assert_eq!((main.get(), left.get()), (1, 0));
    assert!(!app.global::<Notice>().get_open());
}

#[test]
fn backtab_reaches_left() {
    let (app, main, left, _, _) = abierto();
    key(&app, "\u{19}");
    key(&app, " ");
    assert_eq!((main.get(), left.get()), (0, 1));
    assert!(!app.global::<Notice>().get_open());
}

/// `ui::notice` cierra la ventana antes de dar la acción (que puede abrir otro modal),
/// da la de cada botón una sola vez y «Cerrar» no da ninguna.
#[test]
fn notice_closes_then_acts_once() {
    use std::cell::RefCell;
    use wincleaner::space::notice::{Act, Button, Group, Notice as Aviso};
    use wincleaner::ui::{install_modal, notice};
    headless::init();
    let app = AppWindow::new().unwrap();
    install_modal(&app);
    let btn = |act| Some(Button { text: "x".into(), icon: "", act });
    let n = Aviso {
        title: "No se ha podido borrar".into(),
        text: String::new(),
        lines: vec![],
        done: String::new(),
        head: String::new(),
        groups: vec![Group { icon: "pulse", title: "En uso".into(), meta: String::new(), names: String::new(), button: btn(Act::Procs(Some(7))) }],
        left: btn(Act::Explorer),
        close: "Cerrar",
        main: btn(Act::Retry),
    };
    let got: Rc<RefCell<Vec<(Act, bool)>>> = Rc::default();
    let w = app.global::<wincleaner::ui::Notice>();
    for k in 0..4 {
        let (g, a) = (got.clone(), app.as_weak());
        notice(&app, &n, move |act| g.borrow_mut().push((act, a.unwrap().global::<wincleaner::ui::Notice>().get_open())));
        assert!(w.get_open());
        match k {
            0 => w.invoke_main(),
            1 => w.invoke_left(),
            2 => w.invoke_group(0),
            _ => w.invoke_close(),
        }
        w.invoke_main();
    }
    let want = vec![(Act::Retry, false), (Act::Explorer, false), (Act::Procs(Some(7)), false)];
    assert_eq!(*got.borrow(), want);
}
