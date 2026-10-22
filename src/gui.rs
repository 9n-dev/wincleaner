//! Ventana principal: unidades, pestaña Limpieza y pestaña Análisis.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::SystemTime;

use native_windows_gui as nwg;
use nwg::{CheckBoxState, MessageButtons, MessageChoice, MessageIcons, MessageParams, ProgressBarState};

use wincleaner::categories::{Category, CategoryId, Status, build_categories};
use wincleaner::clean::{self, DeleteReport, ScanResult};
use wincleaner::format;

use crate::win;

// ponytail: solo se muestran 4 unidades; con más haría falta scroll.
const MAX_DRIVES: usize = 4;

enum Msg {
    /// Índice de la categoría, escaneo y nº de archivos (de elementos en la papelera).
    Scanned(usize, ScanResult, u64),
    Cleaned(Vec<(&'static str, DeleteReport)>),
}

#[derive(Default)]
struct State {
    cats: Vec<Category>,
    scans: Vec<Option<ScanResult>>,
    recycle_items: u64,
    pending: usize,
}

#[derive(Default)]
struct Ui {
    window: nwg::Window,
    status: nwg::StatusBar,
    notice: nwg::Notice,
    /// Etiqueta `C:`, barra de % usado y texto de espacio libre.
    drives: Vec<(nwg::Label, nwg::ProgressBar, nwg::Label)>,
    tabs: nwg::TabsContainer,
    clean_tab: nwg::Tab,
    analysis_tab: nwg::Tab,
    /// Casilla, tamaño y archivos de cada categoría.
    rows: Vec<(nwg::CheckBox, nwg::Label, nwg::Label)>,
    admin_btn: nwg::Button,
    clean_btn: nwg::Button,
}

pub struct App {
    ui: Ui,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    state: RefCell<State>,
}

pub fn run() {
    nwg::init().expect("no se pudo iniciar la interfaz");
    let mut font = nwg::Font::default();
    nwg::Font::builder().family("Segoe UI").size(16).build(&mut font).expect("fuente Segoe UI");
    nwg::Font::set_global_default(Some(font));

    let cats = categories();
    let ui = build_ui(win::is_admin(), cats.len()).expect("no se pudo crear la ventana");
    let (tx, rx) = channel();
    let app = Rc::new(App { ui, tx, rx, state: Default::default() });
    let a = Rc::clone(&app);
    let handler = nwg::full_bind_event_handler(&app.ui.window.handle, move |evt, _, h| a.on_event(evt, h));
    app.start_scan(cats);
    nwg::dispatch_thread_events();
    nwg::unbind_event_handler(&handler);
}

/// Categorías del entorno actual. Si el entorno no parece fiable (ninguna raíz de
/// unidad protegida o ninguna carpeta personal prohibida), solo queda la papelera.
fn categories() -> Vec<Category> {
    let env = win::gather_env();
    let drive_root = |p: &String| matches!(p.as_bytes(), [l, b':', b'\\'] if l.is_ascii_alphabetic());
    let sane = env.protected.iter().any(drive_root) && !env.forbidden_trees.is_empty();
    let mut cats = build_categories(&env);
    for c in cats.iter_mut().filter(|c| !sane && c.id != CategoryId::RecycleBin) {
        c.status = Status::Unrecognized;
        c.roots.clear();
    }
    cats
}

fn build_ui(admin: bool, rows: usize) -> Result<Ui, nwg::NwgError> {
    let mut ui = Ui::default();
    nwg::Window::builder()
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::VISIBLE)
        .size((640, 600))
        .center(true)
        .title(if admin { "WinCleaner (administrador)" } else { "WinCleaner" })
        .build(&mut ui.window)?;
    nwg::StatusBar::builder().parent(&ui.window).build(&mut ui.status)?;
    nwg::Notice::builder().parent(&ui.window).build(&mut ui.notice)?;

    for i in 0..MAX_DRIVES {
        let y = 8 + 24 * i as i32;
        let (mut name, mut bar, mut text) = Default::default();
        nwg::Label::builder().parent(&ui.window).position((12, y)).size((30, 20)).build(&mut name)?;
        nwg::ProgressBar::builder()
            .parent(&ui.window)
            .position((46, y + 2))
            .size((300, 16))
            .range(0..100)
            .build(&mut bar)?;
        nwg::Label::builder().parent(&ui.window).position((358, y)).size((270, 20)).build(&mut text)?;
        ui.drives.push((name, bar, text));
    }

    nwg::TabsContainer::builder().parent(&ui.window).position((8, 106)).size((624, 464)).build(&mut ui.tabs)?;
    nwg::Tab::builder().parent(&ui.tabs).text("Limpieza").build(&mut ui.clean_tab)?;
    nwg::Tab::builder().parent(&ui.tabs).text("Análisis").build(&mut ui.analysis_tab)?;

    for i in 0..rows {
        let y = 12 + 30 * i as i32;
        let (mut check, mut size, mut files) = Default::default();
        nwg::CheckBox::builder().parent(&ui.clean_tab).position((12, y)).size((330, 24)).build(&mut check)?;
        nwg::Label::builder()
            .parent(&ui.clean_tab)
            .position((346, y))
            .size((110, 24))
            .h_align(nwg::HTextAlign::Right)
            .build(&mut size)?;
        nwg::Label::builder().parent(&ui.clean_tab).position((472, y)).size((130, 24)).build(&mut files)?;
        ui.rows.push((check, size, files));
    }
    let y = 24 + 30 * rows as i32;
    nwg::Button::builder()
        .parent(&ui.clean_tab)
        .text("🛡 Reiniciar como administrador")
        .position((12, y))
        .size((260, 32))
        .build(&mut ui.admin_btn)?;
    nwg::Button::builder()
        .parent(&ui.clean_tab)
        .text("Revisar y limpiar (0 B)…")
        .position((342, y))
        .size((260, 32))
        .enabled(false)
        .build(&mut ui.clean_btn)?;
    ui.admin_btn.set_visible(!admin);
    Ok(ui)
}

/// Borra una categoría ya confirmada. La papelera se mide antes y después de vaciarla,
/// porque `empty_recycle_bin` también da por buena una papelera que no se vació.
fn clean_one(cat: &Category, scan: &ScanResult) -> DeleteReport {
    if cat.id != CategoryId::RecycleBin {
        return clean::delete(cat, scan, SystemTime::now());
    }
    let (bytes, items) = win::recycle_bin_size();
    let ok = win::empty_recycle_bin();
    let (left_bytes, left_items) = win::recycle_bin_size();
    DeleteReport {
        freed: bytes.saturating_sub(left_bytes),
        deleted: items.saturating_sub(left_items),
        skipped: if ok { 0 } else { left_items },
        skipped_bytes: if ok { 0 } else { left_bytes },
    }
}

impl App {
    fn on_event(&self, evt: nwg::Event, h: nwg::ControlHandle) {
        use nwg::Event as E;
        match evt {
            E::OnWindowClose if h == self.ui.window => nwg::stop_thread_dispatch(),
            E::OnNotice if h == self.ui.notice => {
                while let Ok(msg) = self.rx.try_recv() {
                    self.on_msg(msg);
                }
            }
            E::OnButtonClick if h == self.ui.admin_btn && win::relaunch_as_admin() => nwg::stop_thread_dispatch(),
            E::OnButtonClick if h == self.ui.clean_btn => self.confirm_and_clean(),
            E::OnButtonClick if self.ui.rows.iter().any(|r| h == r.0) => self.update_total(),
            _ => {}
        }
    }

    fn on_msg(&self, msg: Msg) {
        match msg {
            Msg::Scanned(i, scan, n) => {
                let (check, size, files) = &self.ui.rows[i];
                let mut st = self.state.borrow_mut();
                let recycle = st.cats[i].id == CategoryId::RecycleBin;
                size.set_text(&format::bytes(scan.bytes));
                files.set_text(&format!("{} {}", format::count(n), if recycle { "elementos" } else { "archivos" }));
                if scan.bytes == 0 {
                    check.set_check_state(CheckBoxState::Unchecked);
                }
                if recycle {
                    st.recycle_items = n;
                }
                st.scans[i] = Some(scan);
                st.pending = st.pending.saturating_sub(1);
                if st.pending == 0 {
                    self.ui.status.set_text(0, "Tamaños calculados.");
                }
                drop(st);
                self.update_total();
            }
            Msg::Cleaned(reports) => {
                let sum = |f: fn(&DeleteReport) -> u64| reports.iter().map(|(_, r)| f(r)).sum::<u64>();
                let (skipped, skipped_bytes) = (sum(|r| r.skipped), sum(|r| r.skipped_bytes));
                let mut text = format!("Liberados {}\n", format::bytes(sum(|r| r.freed)));
                for (name, r) in &reports {
                    text += &format!("\n{name} — {}", format::bytes(r.freed));
                    if r.skipped > 0 {
                        text += &format!(" ({} en uso, saltados)", format::count(r.skipped));
                    }
                }
                if skipped > 0 {
                    text += &format!(
                        "\n\nSaltados: {} archivos en uso ({})",
                        format::count(skipped),
                        format::bytes(skipped_bytes)
                    );
                }
                nwg::modal_info_message(&self.ui.window, "Limpieza terminada", &text);
                self.start_scan(categories());
            }
        }
    }

    /// Reinicia filas y unidades y lanza el escaneo de las categorías `Ready`.
    fn start_scan(&self, cats: Vec<Category>) {
        self.refresh_drives();
        for (cat, (check, size, files)) in cats.iter().zip(&self.ui.rows) {
            let ready = cat.status == Status::Ready;
            check.set_text(cat.name);
            check.set_enabled(ready);
            check.set_check_state(if ready { CheckBoxState::Checked } else { CheckBoxState::Unchecked });
            size.set_text(if ready { "Calculando…" } else { "—" });
            files.set_text(match cat.status {
                Status::Ready => "",
                Status::NeedsAdmin => "requiere admin.",
                Status::NotFound => "no encontrado",
                Status::Unrecognized => "ruta no reconocida",
            });
        }
        let jobs: Vec<(usize, Category)> =
            cats.iter().cloned().enumerate().filter(|(_, c)| c.status == Status::Ready).collect();
        *self.state.borrow_mut() =
            State { scans: vec![None; cats.len()], recycle_items: 0, pending: jobs.len(), cats };
        self.ui.status.set_text(0, "Calculando tamaños…");
        self.ui.admin_btn.set_enabled(true);
        self.update_total();

        let (tx, notice) = (self.tx.clone(), self.ui.notice.sender());
        std::thread::spawn(move || {
            for (i, cat) in jobs {
                let (scan, n) = if cat.id == CategoryId::RecycleBin {
                    let (bytes, items) = win::recycle_bin_size();
                    (ScanResult { files: vec![], dirs: vec![], bytes }, items)
                } else {
                    let scan = clean::scan(&cat, SystemTime::now());
                    let n = scan.files.len() as u64;
                    (scan, n)
                };
                if tx.send(Msg::Scanned(i, scan, n)).is_err() {
                    return;
                }
                notice.notice();
            }
        });
    }

    fn refresh_drives(&self) {
        let drives = win::fixed_drives();
        for (i, (name, bar, text)) in self.ui.drives.iter().enumerate() {
            let d = drives.get(i);
            name.set_visible(d.is_some());
            bar.set_visible(d.is_some());
            text.set_visible(d.is_some());
            let Some(d) = d else { continue };
            let used = d.total.saturating_sub(d.free);
            name.set_text(d.root.trim_end_matches('\\'));
            bar.set_pos(if d.total == 0 { 0 } else { (used as u128 * 100 / d.total as u128) as u32 });
            let low = (d.free as u128) * 10 < d.total as u128;
            bar.set_state(if low { ProgressBarState::Error } else { ProgressBarState::Normal });
            text.set_text(&format!("{} libres de {}", format::bytes(d.free), format::bytes(d.total)));
        }
    }

    /// Índices de las categorías marcadas que ya tienen escaneo.
    fn checked(&self, st: &State) -> Vec<usize> {
        (0..st.cats.len())
            .filter(|&i| st.scans[i].is_some() && self.ui.rows[i].0.check_state() == CheckBoxState::Checked)
            .collect()
    }

    fn update_total(&self) {
        let st = self.state.borrow();
        let total: u64 = self.checked(&st).iter().filter_map(|&i| st.scans[i].as_ref()).map(|s| s.bytes).sum();
        self.ui.clean_btn.set_text(&format!("Revisar y limpiar ({})…", format::bytes(total)));
        self.ui.clean_btn.set_enabled(st.pending == 0 && total > 0);
    }

    fn confirm_and_clean(&self) {
        let (sel, text) = {
            let st = self.state.borrow();
            let sel = self.checked(&st);
            let (mut lines, mut total, mut files) = (Vec::new(), 0, 0);
            for &i in &sel {
                let Some(scan) = &st.scans[i] else { continue };
                lines.push(format!("{} — {}", st.cats[i].name, format::bytes(scan.bytes)));
                total += scan.bytes;
                files += if st.cats[i].id == CategoryId::RecycleBin { st.recycle_items } else { scan.files.len() as u64 };
            }
            let text = format!(
                "Se van a borrar:\n\n{}\n\nTotal: {} ({} archivos)\n\nLos archivos en uso se saltarán. Esto no se puede deshacer.",
                lines.join("\n"),
                format::bytes(total),
                format::count(files)
            );
            (sel, text)
        };
        let params = MessageParams {
            title: "Confirmar limpieza",
            content: &text,
            buttons: MessageButtons::OkCancel,
            icons: MessageIcons::Warning,
        };
        if nwg::modal_message(&self.ui.window, &params) != MessageChoice::Ok {
            return;
        }

        let jobs: Vec<(Category, ScanResult)> = {
            let mut st = self.state.borrow_mut();
            sel.iter().filter_map(|&i| Some((st.cats[i].clone(), st.scans[i].take()?))).collect()
        };
        for (check, ..) in &self.ui.rows {
            check.set_enabled(false);
        }
        self.ui.admin_btn.set_enabled(false);
        self.ui.clean_btn.set_enabled(false);
        self.ui.status.set_text(0, "Limpiando…");
        let (tx, notice) = (self.tx.clone(), self.ui.notice.sender());
        std::thread::spawn(move || {
            let reports = jobs.iter().map(|(cat, scan)| (cat.name, clean_one(cat, scan))).collect();
            if tx.send(Msg::Cleaned(reports)).is_ok() {
                notice.notice();
            }
        });
    }
}
