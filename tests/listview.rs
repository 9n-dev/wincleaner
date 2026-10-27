//! La lista de Carpetas solo crea las filas que se ven: 100 000 filas se pintan enseguida.
#![cfg(not(windows))]

use slint::ComponentHandle;
use wincleaner::ui::{AppWindow, SpaceData, SpaceRow, headless};

#[test]
fn listview_100k_rows_renders_fast() {
    headless::init();
    let app = AppWindow::new().unwrap();
    app.set_section(2);
    let d = app.global::<SpaceData>();
    let rows: Vec<SpaceRow> = (0..100_000).map(|i| SpaceRow { name: format!("f{i}").into(), selectable: true, ..Default::default() }).collect();
    d.set_rows(std::rc::Rc::new(slint::VecModel::from(rows)).into());
    d.set_phase(2);
    let start = std::time::Instant::now();
    headless::render(app.window(), 1100, 720);
    // Virtualizada tarda 0,1–0,3 s; sin virtualizar, 100 000 filas tardaban unos 30 s. Con 3 s
    // de tope la prueba no falla por una máquina cargada o una compilación de depuración.
    assert!(start.elapsed() < std::time::Duration::from_secs(3), "{:?}", start.elapsed());
}
