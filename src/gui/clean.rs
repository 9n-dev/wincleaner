//! Sección Limpiar: la lógica de la v1 (escaneo, confirmación y borrado) con la interfaz nueva.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use wincleaner::categories::{Category, CategoryId, Status, build_categories};
use wincleaner::clean::{self, DeleteReport, ScanResult};
use wincleaner::format;
use wincleaner::ui::{AppWindow, CatState, CategoryRow, CleanData, CleanPhase, Confirm, ModalLine, ReportRow, confirm};

use crate::win;

#[derive(Default)]
struct State {
    cats: Vec<Category>,
    scans: Vec<Option<ScanResult>>,
    /// Elementos de la papelera (sus «archivos»).
    recycle_items: u64,
    checked: Vec<bool>,
    pending: usize,
    /// Sube con cada escaneo: los resultados de uno anterior se descartan.
    generation: u64,
    /// Hay un borrado en marcha: la ventana no se cierra ni cambia de sección.
    cleaning: bool,
    /// Se enseña el informe del último borrado.
    report: bool,
}

impl State {
    /// Archivos escaneados de la categoría `i` (elementos, en la papelera).
    fn items(&self, i: usize) -> u64 {
        if self.cats[i].id == CategoryId::RecycleBin {
            return self.recycle_items;
        }
        self.scans[i].as_ref().map_or(0, |s| s.files.len() as u64)
    }

    fn bytes(&self, i: usize) -> u64 {
        self.scans[i].as_ref().map_or(0, |s| s.bytes)
    }

    /// Índices de las categorías marcadas que ya tienen escaneo.
    fn checked(&self) -> Vec<usize> {
        (0..self.cats.len()).filter(|&i| self.scans[i].is_some() && self.checked[i]).collect()
    }

    /// Bytes marcados y nº de categorías marcadas con algo que borrar, para el Resumen.
    fn totals(&self) -> (u64, usize) {
        let sel = self.checked();
        let n = sel.iter().filter(|&&i| self.items(i) > 0 || self.bytes(i) > 0).count();
        (sel.iter().map(|&i| self.bytes(i)).sum(), n)
    }

    fn can_clean(&self) -> bool {
        // Una categoría con archivos de 0 B también se puede limpiar.
        self.pending == 0 && !self.cleaning && self.checked().iter().any(|&i| self.items(i) > 0 || self.bytes(i) > 0)
    }
}

thread_local! {
    // Solo vive en el hilo de la interfaz: los hilos de trabajo vuelven a él con
    // `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

/// Nombre y descripción de cada categoría, como en las maquetas.
fn label(id: CategoryId) -> (&'static str, &'static str) {
    use CategoryId::*;
    match id {
        UserTemp => ("Temporales del usuario", "Archivos de %TEMP% sin tocar en más de 24 h"),
        WindowsTemp => ("Temporales de Windows", "%SystemRoot%\\Temp · más de 24 h"),
        RecycleBin => ("Papelera de reciclaje", "Se vacía entera. Empieza desmarcada"),
        Thumbnails => ("Caché de miniaturas", "thumbcache del Explorador"),
        Chrome => ("Caché de Chrome", "Caché de todos los perfiles"),
        Edge => ("Caché de Edge", "Caché de todos los perfiles"),
        Firefox => ("Caché de Firefox", "cache2 de cada perfil"),
        DumpsLogs => ("Volcados y logs antiguos", "Informes de errores de más de 30 días"),
        WindowsUpdate => ("Caché de Windows Update", "%SystemRoot%\\SoftwareDistribution\\Download"),
    }
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

fn row(st: &State, i: usize) -> CategoryRow {
    let (name, desc) = label(st.cats[i].id);
    let scan = st.scans[i].as_ref();
    let state = match (st.cats[i].status, scan) {
        (Status::Ready, Some(_)) => CatState::Ready,
        (Status::Ready, None) => CatState::Calculating,
        (Status::NeedsAdmin, _) => CatState::Admin,
        (Status::NotFound, _) => CatState::Missing,
        (Status::Unrecognized, _) => CatState::Unrecognized,
    };
    let unit = if st.cats[i].id == CategoryId::RecycleBin { "elementos" } else { "archivos" };
    let (size, files) = match scan {
        Some(s) => (format::bytes(s.bytes), format!("{} {unit}", format::count(st.items(i)))),
        None => Default::default(),
    };
    CategoryRow {
        name: name.into(),
        desc: desc.into(),
        size: size.into(),
        files: files.into(),
        state,
        checked: st.checked[i],
    }
}

/// Texto de los saltados: «N archivos estaban en uso…», o nada si no hubo.
fn skipped_text(n: u64) -> String {
    match n {
        0 => String::new(),
        1 => "1 archivo estaba en uso y se ha saltado. Es normal.".into(),
        n => format!("{} archivos estaban en uso y se han saltado. Es normal.", format::count(n)),
    }
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(v)).into()
}

/// Controlador de la sección. Solo guarda la ventana (débil): se puede mandar a los hilos.
#[derive(Clone)]
pub struct Clean(Weak<AppWindow>);

impl Clean {
    pub fn new(app: &AppWindow) -> Clean {
        let c = Clean(app.as_weak());
        let data = app.global::<CleanData>();
        let k = c.clone();
        data.on_toggled(move |i| k.toggled(i as usize));
        let k = c.clone();
        data.on_review(move || k.review());
        let k = c.clone();
        // Sale del informe: el escaneo que lanzó `cleaned` ya está en curso o terminado.
        data.on_recalc(move || {
            let Some(app) = k.0.upgrade() else { return };
            STATE.with_borrow_mut(|st| {
                st.report = false;
                Self::refresh(&app, st);
            });
        });
        c.start_scan();
        c
    }

    /// Lista, total, botón y fase a partir del estado.
    fn refresh(app: &AppWindow, st: &State) {
        let data = app.global::<CleanData>();
        let rows = data.get_rows();
        for i in 0..st.cats.len() {
            rows.set_row_data(i, row(st, i));
        }
        let (total, cats) = st.totals();
        data.set_total(format::bytes(total).into());
        // Tarjeta del Resumen: se actualiza con cada escaneo y cada borrado.
        super::summary::set_clean(app, total, cats, st.pending > 0);
        data.set_can_clean(st.can_clean());
        data.set_phase(if st.cleaning {
            CleanPhase::Cleaning
        } else if st.report {
            CleanPhase::Report
        } else if st.pending > 0 {
            CleanPhase::Calculating
        } else {
            CleanPhase::Ready
        });
    }

    /// Reinicia la lista y lanza el escaneo de las categorías `Ready`.
    fn start_scan(&self) {
        let Some(app) = self.0.upgrade() else { return };
        if STATE.with_borrow(|st| st.cleaning) {
            return;
        }
        let cats = categories();
        let jobs: Vec<(usize, Category)> =
            cats.iter().cloned().enumerate().filter(|(_, c)| c.status == Status::Ready).collect();
        let generation = STATE.with_borrow_mut(|st| {
            *st = State {
                // La papelera es la única con documentos recuperables: se marca a mano.
                checked: cats.iter().map(|c| c.status == Status::Ready && c.id != CategoryId::RecycleBin).collect(),
                scans: vec![None; cats.len()],
                pending: jobs.len(),
                cats,
                generation: st.generation + 1,
                report: st.report,
                ..Default::default()
            };
            let rows: Vec<CategoryRow> = (0..st.cats.len()).map(|i| row(st, i)).collect();
            app.global::<CleanData>().set_rows(model(rows));
            Self::refresh(&app, st);
            st.generation
        });

        let k = self.clone();
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
                    let k2 = k.clone();
                    if k.0.upgrade_in_event_loop(move |app| k2.scanned(&app, generation, i, scan, n)).is_err() {
                        return;
                    }
                }
            })
        });
    }

    fn scanned(&self, app: &AppWindow, generation: u64, i: usize, scan: ScanResult, n: u64) {
        STATE.with_borrow_mut(|st| {
            if st.generation != generation {
                return;
            }
            if n == 0 && scan.bytes == 0 {
                st.checked[i] = false;
            }
            if st.cats[i].id == CategoryId::RecycleBin {
                st.recycle_items = n;
            }
            st.scans[i] = Some(scan);
            st.pending = st.pending.saturating_sub(1);
            Self::refresh(app, st);
        });
    }

    /// La casilla ya escribió su valor en el modelo; aquí se acepta o se deshace.
    fn toggled(&self, i: usize) {
        let Some(app) = self.0.upgrade() else { return };
        STATE.with_borrow_mut(|st| {
            let rows = app.global::<CleanData>().get_rows();
            let (Some(cat), Some(mut r)) = (st.cats.get(i), rows.row_data(i)) else { return };
            // La acción de accesibilidad no mira `enabled`: la regla se aplica aquí.
            if st.cleaning || cat.status != Status::Ready {
                // Durante el borrado los escaneos ya no están: solo se deshace la casilla.
                r.checked = st.checked[i];
                rows.set_row_data(i, r);
                return;
            }
            st.checked[i] = r.checked;
            Self::refresh(&app, st);
        });
    }

    /// Abre el modal con lo marcado; el borrado solo empieza al confirmar.
    fn review(&self) {
        let Some(app) = self.0.upgrade() else { return };
        let Some((sel, generation, title, lines)) = STATE.with_borrow(|st| {
            if !st.can_clean() {
                return None;
            }
            let sel = st.checked();
            let total: u64 = sel.iter().map(|&i| st.bytes(i)).sum();
            let lines: Vec<ModalLine> = sel
                .iter()
                .map(|&i| ModalLine { label: label(st.cats[i].id).0.into(), value: format::bytes(st.bytes(i)).into() })
                .collect();
            Some((sel, st.generation, format!("¿Borrar {}?", format::bytes(total)), lines))
        }) else {
            return;
        };
        let k = self.clone();
        let ask = Confirm {
            title: &title,
            text: "Se va a borrar el contenido de estas categorías:",
            lines,
            note: "Los archivos en uso se saltarán. Esto no se puede deshacer.",
            confirm_text: "Borrar",
            danger: true,
        };
        confirm(&app, ask, move || k.clean(generation, &sel));
    }

    /// Borra lo confirmado en un hilo, si nada ha cambiado desde que se abrió el modal.
    fn clean(&self, generation: u64, sel: &[usize]) {
        let Some(app) = self.0.upgrade() else { return };
        let Some((first, jobs)) = STATE.with_borrow_mut(|st| {
            if st.generation != generation || !st.can_clean() || st.checked() != sel {
                return None;
            }
            // `can_clean` asegura que cada marcada tiene escaneo; aun así, sin trabajos no
            // se marca `cleaning`, que bloquearía la ventana para siempre.
            let jobs: Vec<(Category, ScanResult)> =
                sel.iter().filter_map(|&i| Some((st.cats[i].clone(), st.scans[i].take()?))).collect();
            let first = jobs.first()?.0.id;
            st.cleaning = true;
            Some((first, jobs))
        }) else {
            return;
        };
        let total: u64 = jobs.iter().map(|(_, s)| s.bytes).sum();
        app.set_locked(true);
        app.global::<CleanData>().set_phase(CleanPhase::Cleaning);
        Self::progress(&app, first, DeleteReport::default(), total, 0.0);

        let k = self.clone();
        std::thread::spawn(move || {
            let reports = win::with_com(|| {
                let (mut done, mut sum, mut out) = (0, DeleteReport::default(), Vec::new());
                for (n, (cat, scan)) in jobs.iter().enumerate() {
                    let r = clean_one(cat, scan);
                    done += scan.bytes;
                    sum.freed += r.freed;
                    sum.deleted += r.deleted;
                    sum.skipped += r.skipped;
                    out.push((cat.id, r));
                    // Se nombra la que viene, que es la que está en curso.
                    let next = jobs.get(n + 1).unwrap_or(&jobs[n]).0.id;
                    let fraction =
                        if total > 0 { done as f32 / total as f32 } else { (n + 1) as f32 / jobs.len() as f32 };
                    let _ = k.0.upgrade_in_event_loop(move |app| Self::progress(&app, next, sum, total, fraction));
                }
                out
            });
            let k2 = k.clone();
            let _ = k.0.upgrade_in_event_loop(move |app| k2.cleaned(&app, reports));
        });
    }

    fn progress(app: &AppWindow, current: CategoryId, sum: DeleteReport, total: u64, fraction: f32) {
        let data = app.global::<CleanData>();
        data.set_progress_name(label(current).0.into());
        data.set_progress_text(format!("{} de {}", format::bytes(sum.freed), format::bytes(total)).into());
        data.set_progress_detail(
            format!(
                "{} archivos borrados · {} en uso, se saltan",
                format::count(sum.deleted),
                format::count(sum.skipped)
            )
            .into(),
        );
        data.set_progress(fraction);
    }

    /// Enseña el informe y vuelve a escanear, como la v1.
    fn cleaned(&self, app: &AppWindow, reports: Vec<(CategoryId, DeleteReport)>) {
        STATE.with_borrow_mut(|st| {
            st.cleaning = false;
            st.report = true;
        });
        app.set_locked(false);
        let data = app.global::<CleanData>();
        let freed: u64 = reports.iter().map(|(_, r)| r.freed).sum();
        let skipped: u64 = reports.iter().map(|(_, r)| r.skipped).sum();
        data.set_report_title(format!("Has liberado {}.", format::bytes(freed)).into());
        data.set_report_text(skipped_text(skipped).into());
        data.set_report(model(
            reports
                .iter()
                .map(|(id, r)| ReportRow {
                    name: label(*id).0.into(),
                    freed: format::bytes(r.freed).into(),
                    skipped: if r.skipped > 0 { format!("{} en uso", format::count(r.skipped)) } else { "—".into() }
                        .into(),
                })
                .collect(),
        ));
        self.start_scan();
    }
}
