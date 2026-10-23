//! Sección Analizar: la lógica de la v1 (análisis en un hilo, con «Detener») con la interfaz nueva.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel, Weak};
use wincleaner::analyze::{self, Analysis};
use wincleaner::format;
use wincleaner::ui::{AnalyzeData, AppWindow, DirRow};

use crate::win;

#[derive(Default)]
struct State {
    drives: Vec<win::Drive>,
    /// Hay un análisis en marcha: `true` en su `cancel` lo detiene.
    cancel: Option<Arc<AtomicBool>>,
    analysis: Option<Analysis>,
    /// Marca el tiempo transcurrido en pantalla; vive mientras dura el análisis.
    timer: Option<Timer>,
}

thread_local! {
    // Solo vive en el hilo de la interfaz: el hilo de trabajo vuelve con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

/// Lo que el hilo de trabajo cuenta: archivos leídos y carpeta en curso. El temporizador de
/// 1 s lo lee, así la interfaz no se actualiza cada vez que `analyze` avisa.
type Shared = Arc<Mutex<(u64, String)>>;

fn files(n: u64) -> String {
    if n == 1 { "1 archivo".into() } else { format!("{} archivos", format::count(n)) }
}

/// «C: · 64 GB libres»
fn drive_text(d: &win::Drive) -> String {
    format!("{} · {} libres", d.root.trim_end_matches('\\'), format::bytes(d.free))
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(v)).into()
}

/// Filas de una lista; la barra es relativa a la primera (la mayor).
fn rows(list: &[(std::path::PathBuf, u64)]) -> ModelRc<DirRow> {
    let max = list.first().map_or(0, |r| r.1).max(1) as f32;
    model(
        list.iter()
            .map(|(p, s)| DirRow { path: p.display().to_string().into(), size: format::bytes(*s).into(), share: *s as f32 / max })
            .collect(),
    )
}

fn shown(a: &Analysis, kind: i32) -> &[(std::path::PathBuf, u64)] {
    if kind == 1 { &a.top_files } else { &a.top_dirs }
}

#[derive(Clone)]
pub struct Analyze(Weak<AppWindow>);

impl Analyze {
    pub fn new(app: &AppWindow) -> Analyze {
        let c = Analyze(app.as_weak());
        let data = app.global::<AnalyzeData>();
        c.refresh_drives(app);
        let k = c.clone();
        data.on_start_stop(move || k.start_stop());
        let k = c.clone();
        data.on_open(move |i| k.open(i));
        let k = c.clone();
        data.on_kind_changed(move || k.kind_changed());
        c
    }

    /// Unidades fijas y su espacio libre; se conserva la elegida si sigue existiendo.
    fn refresh_drives(&self, app: &AppWindow) {
        let drives = win::fixed_drives();
        let data = app.global::<AnalyzeData>();
        data.set_drives(model(drives.iter().map(|d| drive_text(d).into()).collect()));
        if data.get_drive() as usize >= drives.len() {
            data.set_drive(0);
        }
        STATE.with_borrow_mut(|st| st.drives = drives);
    }

    /// «Analizar» y «Volver a analizar» lanzan el análisis; con uno en marcha hacen de «Detener».
    fn start_stop(&self) {
        let Some(app) = self.0.upgrade() else { return };
        let data = app.global::<AnalyzeData>();
        let running = STATE.with_borrow(|st| st.cancel.clone());
        if let Some(cancel) = running {
            cancel.store(true, Ordering::Relaxed);
            return;
        }
        let drive = data.get_drive() as usize;
        let Some(root) = STATE.with_borrow(|st| st.drives.get(drive).map(|d| d.root.clone())) else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        let shared: Shared = Arc::default();
        let start = Instant::now();

        data.set_title(format!("Analizando {}…", root.trim_end_matches('\\')).into());
        data.set_current("".into());
        data.set_progress(format!("{} · 0 s", files(0)).into());
        data.set_selected(-1);
        data.set_phase(1);
        let timer = Timer::default();
        let (weak, sh) = (self.0.clone(), Arc::clone(&shared));
        timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            let (Some(app), Ok(g)) = (weak.upgrade(), sh.lock()) else { return };
            let data = app.global::<AnalyzeData>();
            data.set_progress(format!("{} · {}", files(g.0), format::duration(start.elapsed())).into());
            data.set_current(g.1.as_str().into());
        });
        STATE.with_borrow_mut(|st| {
            st.cancel = Some(Arc::clone(&cancel));
            st.timer = Some(timer);
        });

        let k = self.clone();
        std::thread::spawn(move || {
            let a = analyze::analyze(Path::new(&root), &cancel, &mut |n, dir| {
                if let Ok(mut g) = shared.lock() {
                    *g = (n, dir.display().to_string());
                }
            });
            let took = start.elapsed();
            let _ = k.0.upgrade_in_event_loop(move |app| Self::finished(&app, drive, a, took));
        });
    }

    fn finished(app: &AppWindow, drive: usize, a: Analysis, took: Duration) {
        let data = app.global::<AnalyzeData>();
        let (n, t) = (files(a.files_scanned), format::duration(took));
        data.set_summary(
            if a.cancelled {
                format!("Detenido tras leer {n} en {t}. Los resultados son parciales.")
            } else {
                format!("Analizados {n} en {t}.")
            }
            .into(),
        );
        data.set_rows(rows(shown(&a, data.get_kind())));
        data.set_selected(-1);
        // La unidad no se pudo cambiar durante el análisis; aquí se deja la que se analizó.
        data.set_drive(drive as i32);
        data.set_phase(2);
        STATE.with_borrow_mut(|st| {
            st.cancel = None;
            st.timer = None;
            st.analysis = Some(a);
        });
        // El análisis cambia el espacio libre.
        Analyze(app.as_weak()).refresh_drives(app);
    }

    fn kind_changed(&self) {
        let Some(app) = self.0.upgrade() else { return };
        let data = app.global::<AnalyzeData>();
        STATE.with_borrow(|st| {
            if let Some(a) = &st.analysis {
                data.set_rows(rows(shown(a, data.get_kind())));
            }
        });
        data.set_selected(-1);
    }

    /// Abre en el Explorador la fila `i` de la lista que se ve.
    fn open(&self, i: i32) {
        let Some(app) = self.0.upgrade() else { return };
        let kind = app.global::<AnalyzeData>().get_kind();
        STATE.with_borrow(|st| {
            let path = st.analysis.as_ref().and_then(|a| shown(a, kind).get(usize::try_from(i).ok()?));
            if let Some((path, _)) = path {
                win::open_in_explorer(path);
            }
        });
    }
}
