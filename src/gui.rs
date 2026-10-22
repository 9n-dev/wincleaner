//! Ventana principal: unidades, pestaña Limpieza y pestaña Análisis.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use native_windows_gui as nwg;
use nwg::{CheckBoxState, ProgressBarState, RadioButtonState};

use wincleaner::analyze::{Analysis, analyze};
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
    /// Archivos analizados hasta ahora.
    Progress(u64),
    Analyzed(Analysis, Duration),
}

#[derive(Default)]
struct State {
    cats: Vec<Category>,
    scans: Vec<Option<ScanResult>>,
    recycle_items: u64,
    pending: usize,
    /// Hay un borrado en marcha: la ventana no se cierra ni cambia de pestaña.
    cleaning: bool,
    analysis: Option<Analysis>,
    /// Hay un análisis en marcha; se detiene poniéndolo a `true`.
    cancel: Option<Arc<AtomicBool>>,
}

impl State {
    /// Archivos escaneados de la categoría `i` (elementos, en la papelera).
    fn items(&self, i: usize) -> u64 {
        if self.cats[i].id == CategoryId::RecycleBin {
            return self.recycle_items;
        }
        self.scans[i].as_ref().map_or(0, |s| s.files.len() as u64)
    }
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
    drive_label: nwg::Label,
    drive_combo: nwg::ComboBox<String>,
    dirs_radio: nwg::RadioButton,
    files_radio: nwg::RadioButton,
    analyze_btn: nwg::Button,
    list: nwg::ListView,
    summary: nwg::Label,
    open_btn: nwg::Button,
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
    let handler = nwg::full_bind_event_handler(&app.ui.window.handle, move |evt, data, h| a.on_event(evt, &data, h));
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
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::MINIMIZE_BOX | nwg::WindowFlags::VISIBLE)
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
        .text("Reiniciar como administrador")
        .position((12, y))
        .size((260, 32))
        .build(&mut ui.admin_btn)?;
    if let Some(h) = ui.admin_btn.handle.hwnd() {
        win::set_shield(h);
    }
    nwg::Button::builder()
        .parent(&ui.clean_tab)
        .text("Revisar y limpiar (0 B)…")
        .position((342, y))
        .size((260, 32))
        .enabled(false)
        .build(&mut ui.clean_btn)?;
    ui.admin_btn.set_visible(!admin);

    let tab = &ui.analysis_tab;
    nwg::Label::builder().parent(tab).text("Unidad:").position((12, 14)).size((54, 20)).build(&mut ui.drive_label)?;
    nwg::ComboBox::builder()
        .parent(tab)
        .position((70, 10))
        .size((80, 25))
        .collection(win::fixed_drives().into_iter().map(|d| d.root).collect())
        .selected_index(Some(0))
        .build(&mut ui.drive_combo)?;
    nwg::RadioButton::builder()
        .parent(tab)
        .text("Carpetas")
        .position((172, 12))
        .size((90, 24))
        .flags(nwg::RadioButtonFlags::VISIBLE | nwg::RadioButtonFlags::GROUP)
        .check_state(RadioButtonState::Checked)
        .build(&mut ui.dirs_radio)?;
    nwg::RadioButton::builder()
        .parent(tab)
        .text("Archivos")
        .position((266, 12))
        .size((90, 24))
        .build(&mut ui.files_radio)?;
    nwg::Button::builder()
        .parent(tab)
        .text("Analizar")
        .position((482, 8))
        .size((120, 32))
        .build(&mut ui.analyze_btn)?;
    nwg::ListView::builder()
        .parent(tab)
        .position((12, 48))
        .size((590, 332))
        .list_style(nwg::ListViewStyle::Detailed)
        .ex_flags(nwg::ListViewExFlags::FULL_ROW_SELECT)
        .build(&mut ui.list)?;
    // NWG crea la lista con LVS_NOCOLUMNHEADER forzado.
    ui.list.set_headers_enabled(true);
    for (text, width, fmt) in
        [("#", 40, None), ("Ruta", 420, None), ("Tamaño", 100, Some(nwg::ListViewColumnFlags::RIGHT))]
    {
        ui.list.insert_column(nwg::InsertListViewColumn {
            index: None,
            fmt,
            width: Some(width),
            text: Some(text.into()),
        });
    }
    nwg::Label::builder().parent(tab).position((12, 396)).size((440, 24)).build(&mut ui.summary)?;
    nwg::Button::builder()
        .parent(tab)
        .text("Abrir ubicación")
        .position((462, 392))
        .size((140, 32))
        .build(&mut ui.open_btn)?;
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
    fn on_event(&self, evt: nwg::Event, data: &nwg::EventData, h: nwg::ControlHandle) {
        use nwg::Event as E;
        match evt {
            E::OnWindowClose if h == self.ui.window => match data {
                nwg::EventData::OnWindowClose(d) if self.state.borrow().cleaning => d.close(false),
                _ => nwg::stop_thread_dispatch(),
            },
            E::OnNotice if h == self.ui.notice => {
                while let Ok(msg) = self.rx.try_recv() {
                    self.on_msg(msg);
                }
            }
            E::OnButtonClick if h == self.ui.admin_btn && win::relaunch_as_admin() => nwg::stop_thread_dispatch(),
            E::OnButtonClick if h == self.ui.clean_btn => self.confirm_and_clean(),
            E::OnButtonClick if self.ui.rows.iter().any(|r| h == r.0) => self.update_total(),
            E::OnButtonClick if h == self.ui.analyze_btn => self.toggle_analysis(),
            E::OnButtonClick if h == self.ui.dirs_radio || h == self.ui.files_radio => self.fill_list(),
            E::OnButtonClick if h == self.ui.open_btn => self.open_selected(),
            E::OnListViewDoubleClick if h == self.ui.list => self.open_selected(),
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
                if n == 0 && scan.bytes == 0 {
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
                self.state.borrow_mut().cleaning = false;
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
            Msg::Progress(n) => self.set_status(&format!("Analizando… {} archivos", format::count(n))),
            Msg::Analyzed(a, took) => {
                let mut summary =
                    format!("Analizados {} archivos en {} s", format::count(a.files_scanned), took.as_secs());
                if a.cancelled {
                    summary += " (detenido)";
                }
                self.ui.summary.set_text(&summary);
                self.ui.analyze_btn.set_text("Analizar");
                let mut st = self.state.borrow_mut();
                st.analysis = Some(a);
                st.cancel = None;
                drop(st);
                self.fill_list();
                self.set_status("Análisis terminado.");
            }
        }
    }

    /// La barra de estado, salvo durante un borrado, que tiene preferencia.
    fn set_status(&self, text: &str) {
        if !self.state.borrow().cleaning {
            self.ui.status.set_text(0, text);
        }
    }

    /// «Analizar» lanza el análisis de la unidad elegida; si ya hay uno, hace de «Detener».
    fn toggle_analysis(&self) {
        let mut st = self.state.borrow_mut();
        if let Some(cancel) = &st.cancel {
            cancel.store(true, Ordering::Relaxed);
            return;
        }
        let combo = &self.ui.drive_combo;
        let Some(root) = combo.selection().and_then(|i| combo.collection().get(i).cloned()) else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        st.cancel = Some(Arc::clone(&cancel));
        drop(st);
        self.ui.analyze_btn.set_text("Detener");
        self.set_status("Analizando… 0 archivos");
        let (tx, notice) = (self.tx.clone(), self.ui.notice.sender());
        std::thread::spawn(move || {
            let start = Instant::now();
            let a = analyze(Path::new(&root), &cancel, &mut |n| {
                if tx.send(Msg::Progress(n)).is_ok() {
                    notice.notice();
                }
            });
            if tx.send(Msg::Analyzed(a, start.elapsed())).is_ok() {
                notice.notice();
            }
        });
    }

    /// Filas del último análisis según la opción elegida.
    fn shown<'a>(&self, a: &'a Analysis) -> &'a [(PathBuf, u64)] {
        if self.ui.dirs_radio.check_state() == RadioButtonState::Checked { &a.top_dirs } else { &a.top_files }
    }

    fn fill_list(&self) {
        self.ui.list.clear();
        let st = self.state.borrow();
        let Some(a) = &st.analysis else { return };
        for (i, (path, size)) in self.shown(a).iter().enumerate() {
            let row = [(i + 1).to_string(), path.display().to_string(), format::bytes(*size)];
            self.ui.list.insert_items_row(None, &row);
        }
    }

    fn open_selected(&self) {
        let st = self.state.borrow();
        let (Some(a), Some(i)) = (&st.analysis, self.ui.list.selected_item()) else { return };
        if let Some((path, _)) = self.shown(a).get(i) {
            win::open_in_explorer(path);
        }
    }

    /// Reinicia filas y unidades y lanza el escaneo de las categorías `Ready`.
    fn start_scan(&self, cats: Vec<Category>) {
        self.refresh_drives();
        for (cat, (check, size, files)) in cats.iter().zip(&self.ui.rows) {
            let ready = cat.status == Status::Ready;
            check.set_text(cat.name);
            check.set_enabled(ready);
            // La papelera es la única con documentos recuperables: se marca a mano.
            let on = ready && cat.id != CategoryId::RecycleBin;
            check.set_check_state(if on { CheckBoxState::Checked } else { CheckBoxState::Unchecked });
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
        // El análisis sigue: solo se reinicia la parte de la limpieza.
        let old = self.state.take();
        *self.state.borrow_mut() = State {
            scans: vec![None; cats.len()],
            pending: jobs.len(),
            cats,
            analysis: old.analysis,
            cancel: old.cancel,
            ..Default::default()
        };
        self.ui.status.set_text(0, "Calculando tamaños…");
        self.ui.admin_btn.set_enabled(true);
        self.ui.tabs.set_enabled(true);
        self.update_total();

        let (tx, notice) = (self.tx.clone(), self.ui.notice.sender());
        std::thread::spawn(move || {
            win::with_com(|| {
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
            })
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
        let sel = self.checked(&st);
        let bytes = |i: usize| st.scans[i].as_ref().map_or(0, |s| s.bytes);
        let total: u64 = sel.iter().map(|&i| bytes(i)).sum();
        // Una categoría con archivos de 0 B también se puede limpiar.
        let any = sel.iter().any(|&i| st.items(i) > 0 || bytes(i) > 0);
        self.ui.clean_btn.set_text(&format!("Revisar y limpiar ({})…", format::bytes(total)));
        self.ui.clean_btn.set_enabled(st.pending == 0 && any);
    }

    fn confirm_and_clean(&self) {
        let (sel, text) = {
            let st = self.state.borrow();
            let sel = self.checked(&st);
            if st.pending != 0 || st.cleaning || sel.is_empty() {
                return;
            }
            let (mut lines, mut total, mut items) = (Vec::new(), 0, 0);
            for &i in &sel {
                let Some(scan) = &st.scans[i] else { continue };
                lines.push(format!("{} — {}", st.cats[i].name, format::bytes(scan.bytes)));
                total += scan.bytes;
                items += st.items(i);
            }
            let text = format!(
                "Se van a borrar:\n\n{}\n\nTotal: {} ({} elementos)\n\nLos archivos en uso se saltarán. Esto no se puede deshacer.",
                lines.join("\n"),
                format::bytes(total),
                format::count(items)
            );
            (sel, text)
        };
        let hwnd = self.ui.window.handle.hwnd().unwrap_or(std::ptr::null_mut());
        if !win::confirm_warning(hwnd, "Confirmar limpieza", &text) {
            return;
        }

        let jobs: Vec<(Category, ScanResult)> = {
            let mut st = self.state.borrow_mut();
            st.cleaning = true;
            sel.iter().filter_map(|&i| Some((st.cats[i].clone(), st.scans[i].take()?))).collect()
        };
        for (check, ..) in &self.ui.rows {
            check.set_enabled(false);
        }
        self.ui.admin_btn.set_enabled(false);
        self.ui.clean_btn.set_enabled(false);
        self.ui.tabs.set_enabled(false);
        self.ui.status.set_text(0, "Limpiando…");
        let (tx, notice) = (self.tx.clone(), self.ui.notice.sender());
        std::thread::spawn(move || {
            let reports = win::with_com(|| jobs.iter().map(|(cat, scan)| (cat.name, clean_one(cat, scan))).collect());
            if tx.send(Msg::Cleaned(reports)).is_ok() {
                notice.notice();
            }
        });
    }
}
