//! Pestaña Duplicados de Espacio: dónde buscar, la búsqueda en un hilo (tres pasadas) y el
//! envío a la papelera de las copias marcadas, comprobando antes que siguen siendo iguales.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel, Weak};
use wincleaner::format;
use wincleaner::space::MAX_ROWS;
use wincleaner::space::zones::Zones;
use wincleaner::space::dups::{self, DupFile, FileKind, Keep, Phase, can_mark, dedupe_roots, keeper, kind_label, still_duplicate};
use wincleaner::ui::{AppWindow, Confirm, DupFileRow, DupGroupRow, DupPlace, DupStep, DupsData, ModalLine, confirm};

use super::space::{Retry, Space, Still};
use crate::win;

const SUBTITLE: &str = "Archivos idénticos repetidos en tus carpetas y unidades.";
const MINS: [u64; 3] = [10 << 10, 1 << 20, 100 << 20];

struct Copy {
    file: DupFile,
    marked: bool,
    /// No pasa `zones.check`: nunca se marca.
    protected: bool,
}

struct Group {
    size: u64,
    kind: FileKind,
    copies: Vec<Copy>,
}

impl Group {
    fn waste(&self) -> u64 {
        self.size.saturating_mul(self.copies.len().saturating_sub(1) as u64)
    }

    /// La copia que se conserva: la primera sin marcar (siempre hay una).
    fn kept(&self) -> Option<&Copy> {
        self.copies.iter().find(|c| !c.marked)
    }
}

#[derive(Default)]
struct State {
    /// Raíces de cada opción de `DupsData.places`, en el mismo orden.
    roots: Vec<Vec<PathBuf>>,
    /// `None` hasta que termina una búsqueda.
    groups: Option<Vec<Group>>,
    /// Índice en `groups` de cada fila de `DupsData.groups`: los del tipo elegido, como mucho
    /// `MAX_ROWS` (los que más sobran). Solo se envía lo que se ve.
    shown: Vec<usize>,
    /// Hay una búsqueda en marcha: `true` en su `cancel` la detiene.
    cancel: Option<Arc<AtomicBool>>,
    /// Pasos en pantalla cada medio segundo, mientras dura la búsqueda.
    timer: Option<Timer>,
    took: Duration,
}

thread_local! {
    // Solo en el hilo de la interfaz: el hilo de trabajo vuelve con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

/// Lo que cuenta la búsqueda: archivos listados, posibles duplicados y cuántos se han comparado.
type Shared = Arc<Mutex<(u64, Option<u64>, Option<u64>)>>;

/// Copia que se va a enviar: (marcada, conservada de su grupo, tamaño).
type Pick = (PathBuf, PathBuf, u64);

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(v)).into()
}

/// «a, b y c»
fn list(v: &[&str]) -> String {
    match v {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} y {last}", rest.join(", ")),
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{} {many}", format::count(n)) }
}

/// Posición de `k` en el filtro de tipo (0 es «Todos»).
fn kind_index(k: FileKind) -> i32 {
    match k {
        FileKind::Image => 1,
        FileKind::Document => 2,
        FileKind::Video => 3,
        FileKind::Music => 4,
        FileKind::Other => 5,
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Los tres pasos con sus cifras y lo que llevan de la última pasada (0..1).
fn steps((listed, sizes, done): (u64, Option<u64>, Option<u64>)) -> (Vec<DupStep>, f32) {
    let step = |name: &str, text: String, state| DupStep { name: name.into(), text: text.into(), state };
    let content = match sizes {
        Some(t) => step("Comparando contenido", format!("{} de {}", format::count(done.unwrap_or(0)), format::count(t)), if done == Some(t) { 2 } else { 1 }),
        None => step("Comparando contenido", String::new(), 0),
    };
    let v = vec![
        step("Listando archivos", plural(listed, "archivo", "archivos"), if sizes.is_some() { 2 } else { 1 }),
        step(
            "Comparando tamaños",
            sizes.map_or(String::new(), |t| plural(t, "posible duplicado", "posibles duplicados")),
            if sizes.is_some() { 2 } else { 0 },
        ),
        content,
    ];
    let progress = match (sizes, done) {
        (Some(t), Some(d)) if t > 0 => (d as f64 / t as f64).min(1.0) as f32,
        _ => 0.0,
    };
    (v, progress)
}

fn row(g: &Group) -> DupGroupRow {
    let n = g.copies.len() as u64;
    let files: Vec<DupFileRow> = g
        .copies
        .iter()
        .map(|c| DupFileRow {
            path: c.file.path.display().to_string().into(),
            date: format::date_dmy(format::ymd(c.file.modified)).into(),
            marked: c.marked,
            protected: c.protected,
        })
        .collect();
    DupGroupRow {
        kind: kind_label(g.kind).into(),
        name: g.kept().map_or_else(String::new, |c| file_name(&c.file.path)).into(),
        text: format!("{} · sobran {}", plural(n, "copia", "copias"), format::bytes(g.waste())).into(),
        files: model(files),
    }
}

/// Las copias marcadas de los grupos que se ven: (marcada, conservada, tamaño) y el nombre del grupo.
fn picked(st: &State) -> Vec<(Vec<Pick>, String)> {
    let Some(groups) = &st.groups else { return Vec::new() };
    st.shown
        .iter()
        .filter_map(|&i| {
            let g = &groups[i];
            let kept = g.kept()?;
            let v: Vec<_> = g
                .copies
                .iter()
                .filter(|c| c.marked && !c.protected)
                .map(|c| (c.file.path.clone(), kept.file.path.clone(), g.size))
                .collect();
            (!v.is_empty()).then(|| (v, file_name(&kept.file.path)))
        })
        .collect()
}

/// Botón de enviar: texto y si se puede pulsar.
fn send_text(st: &State) -> (String, bool) {
    let p = picked(st);
    let n: u64 = p.iter().map(|g| g.0.len() as u64).sum();
    let size: u64 = p.iter().flat_map(|g| &g.0).map(|f| f.2).sum();
    match n {
        0 => ("Enviar a la papelera".into(), false),
        n => (format!("Enviar {} a la papelera · {}", plural(n, "copia", "copias"), format::bytes(size)), true),
    }
}

/// Lo que sobra en todos los grupos, para el Resumen; `None` hasta la primera búsqueda.
pub(super) fn waste() -> Option<u64> {
    STATE.with_borrow(|st| st.groups.as_ref().map(|g| g.iter().map(Group::waste).sum()))
}

/// Sin zonas no se sabe qué es del sistema: no se baja de las raíces.
fn skip(zones: Option<&Zones>, p: &Path) -> bool {
    zones.is_none_or(|z| p.to_str().is_none_or(|s| z.skip_walk(s)))
}

/// Los grupos del tipo elegido (los `MAX_ROWS` que más sobran), el subtítulo, cuántos no caben
/// y el botón.
fn render(app: &AppWindow) {
    let data = app.global::<DupsData>();
    let kind = data.get_kind();
    let (rows, sub, more, (send, can)) = STATE.with_borrow_mut(|st| {
        let groups = st.groups.as_deref().unwrap_or_default();
        // Ya vienen ordenados por lo que sobra.
        let all: Vec<usize> = (0..groups.len()).filter(|&i| kind == 0 || kind_index(groups[i].kind) == kind).collect();
        let waste: u64 = all.iter().map(|&i| groups[i].waste()).sum();
        let sub = match all.len() as u64 {
            0 => format!("Búsqueda terminada en {}", format::duration(st.took)),
            n => format!("{} · sobran {}", plural(n, "grupo", "grupos"), format::bytes(waste)),
        };
        let more = match all.len().saturating_sub(MAX_ROWS) as u64 {
            0 => String::new(),
            m => format!("y {} más", plural(m, "grupo", "grupos")),
        };
        st.shown = all.into_iter().take(MAX_ROWS).collect();
        let rows: Vec<DupGroupRow> = st.shown.iter().map(|&i| row(&groups[i])).collect();
        (rows, sub, more, send_text(st))
    });
    data.set_groups(model(rows));
    data.set_more(more.into());
    data.set_subtitle(sub.into());
    data.set_send_text(send.into());
    data.set_can_send(can);
    data.set_phase(2);
}

fn choose(app: &AppWindow) {
    let data = app.global::<DupsData>();
    data.set_subtitle(SUBTITLE.into());
    data.set_phase(0);
}

#[derive(Clone)]
pub struct Dups {
    app: Weak<AppWindow>,
    space: Space,
}

impl Dups {
    pub fn new(app: &AppWindow, space: Space) -> Dups {
        let d = Dups { app: app.as_weak(), space };
        let data = app.global::<DupsData>();

        // Tus carpetas, las otras unidades enteras y el resto de la de Windows.
        let known = win::known_dirs();
        let mut places = Vec::new();
        let mut roots = Vec::new();
        if !known.is_empty() {
            let labels: Vec<&str> = known.iter().map(|k| k.1).collect();
            places.push(DupPlace { name: "Tus carpetas".into(), text: list(&labels).into(), checked: true });
            roots.push(known.iter().map(|k| k.0.clone()).collect());
        }
        let windows = win::windows_dir().and_then(|w| w.get(..2).map(str::to_uppercase));
        let mut rest = None;
        for drive in win::fixed_drives() {
            let name = drive.root.trim_end_matches('\\').to_string();
            if windows.as_deref() == Some(name.to_uppercase().as_str()) {
                rest = Some((name, drive.root));
                continue;
            }
            let label = format!("{name} {}", drive.label);
            places.push(DupPlace { name: label.trim_end().into(), text: "Toda la unidad".into(), checked: true });
            roots.push(vec![PathBuf::from(&drive.root)]);
        }
        if let Some((name, root)) = rest {
            places.push(DupPlace {
                name: format!("Resto de {name}").into(),
                text: "Sin Windows, Archivos de programa ni AppData".into(),
                checked: false,
            });
            roots.push(vec![PathBuf::from(root)]);
        }
        data.set_places(model(places));
        STATE.with_borrow_mut(|st| st.roots = roots);
        choose(app);

        let k = d.clone();
        data.on_toggle_place(move |i| {
            k.with(|app| {
                let places = app.global::<DupsData>().get_places();
                if let Some(mut p) = usize::try_from(i).ok().and_then(|i| places.row_data(i)) {
                    p.checked = !p.checked;
                    places.set_row_data(i as usize, p);
                }
            })
        });
        let k = d.clone();
        data.on_search(move || k.search());
        let k = d.clone();
        data.on_stop(move || k.with(Self::stop));
        let k = d.clone();
        data.on_kind_changed(move || {
            k.with(|app| {
                if app.global::<DupsData>().get_phase() == 2 {
                    render(app);
                }
            })
        });
        let k = d.clone();
        data.on_toggle(move |g, f| k.with(|app| Self::toggle(app, g, f)));
        let k = d.clone();
        data.on_send(move || k.ask());
        let k = d.clone();
        data.on_again(move || k.again());
        d
    }

    /// «Buscar otra vez»: olvida la búsqueda y vuelve a las opciones.
    pub(super) fn again(&self) {
        self.with(|app| {
            if !app.get_locked() {
                STATE.with_borrow_mut(|st| (st.groups, st.shown) = (None, Vec::new()));
                choose(app);
                super::summary::refresh_space(app);
            }
        })
    }

    fn with(&self, f: impl FnOnce(&AppWindow)) {
        if let Some(app) = self.app.upgrade() {
            f(&app);
        }
    }

    /// «Buscar duplicados»: busca en un hilo en las opciones marcadas.
    fn search(&self) {
        let Some(app) = self.app.upgrade().filter(|a| !a.get_locked()) else { return };
        if STATE.with_borrow(|st| st.cancel.is_some()) {
            return;
        }
        let data = app.global::<DupsData>();
        let places = data.get_places();
        let zones = self.space.zones();
        let roots = STATE.with_borrow(|st| {
            let picked = st.roots.iter().enumerate().filter(|(i, _)| places.row_data(*i).is_some_and(|p| p.checked));
            dedupe_roots(picked.flat_map(|(_, r)| r.clone()).collect(), &|p| skip(zones.as_ref(), p))
        });
        if roots.is_empty() {
            return;
        }
        let min = MINS[usize::try_from(data.get_min()).unwrap_or(0).min(2)];
        let known = win::known_dirs();
        let dirs = |labels: &[&str]| -> Vec<String> {
            known.iter().filter(|k| labels.contains(&k.1)).map(|k| k.0.to_string_lossy().into_owned()).collect()
        };
        let keep = Keep { natural: dirs(&["Documentos", "Imágenes", "Vídeos", "Música"]), loose: dirs(&["Descargas", "Escritorio"]) };

        let cancel = Arc::new(AtomicBool::new(false));
        let shared: Shared = Arc::default();
        let (v, p) = steps((0, None, None));
        data.set_steps(model(v));
        data.set_progress(p);
        data.set_subtitle("Buscando…".into());
        data.set_phase(1);
        let timer = Timer::default();
        let (weak, sh) = (self.app.clone(), Arc::clone(&shared));
        timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            let (Some(app), Ok(g)) = (weak.upgrade(), sh.lock()) else { return };
            let (v, p) = steps(*g);
            let data = app.global::<DupsData>();
            data.set_steps(model(v));
            data.set_progress(p);
        });
        STATE.with_borrow_mut(|st| {
            st.cancel = Some(Arc::clone(&cancel));
            st.timer = Some(timer);
        });

        let weak = self.app.clone();
        let start = Instant::now();
        std::thread::spawn(move || {
            // Sin zonas tampoco se marca nada.
            let found = dups::find(&roots, min, &|p| skip(zones.as_ref(), p), &cancel, &mut |phase| {
                if let Ok(mut g) = shared.lock() {
                    match phase {
                        Phase::Listing(n) => g.0 = n,
                        Phase::Sizes(t) => g.1 = Some(t),
                        Phase::Content(d, _) => g.2 = Some(d),
                    }
                }
            });
            let groups = found.map(|found| {
                found
                    .into_iter()
                    .map(|g| {
                        let k = keeper(&g, &keep);
                        let kind = g.files.first().map_or(FileKind::Other, |f| dups::kind(&file_name(&f.path)));
                        let copies = g
                            .files
                            .into_iter()
                            .enumerate()
                            .map(|(i, file)| {
                                let protected = zones.as_ref().is_none_or(|z| file.path.to_str().is_none_or(|s| z.check(s).is_some()));
                                Copy { marked: i != k && !protected, protected, file }
                            })
                            .collect();
                        Group { size: g.size, kind, copies }
                    })
                    .collect::<Vec<_>>()
            });
            let took = start.elapsed();
            let _ = weak.upgrade_in_event_loop(move |app| Self::found(&app, &cancel, groups, took));
        });
    }

    fn found(app: &AppWindow, cancel: &Arc<AtomicBool>, groups: Option<Vec<Group>>, took: Duration) {
        // Si se detuvo, este resultado no vale.
        let mine = STATE.with_borrow_mut(|st| {
            let mine = st.cancel.as_ref().is_some_and(|c| Arc::ptr_eq(c, cancel));
            if mine {
                (st.cancel, st.timer, st.groups, st.took) = (None, None, groups, took);
            }
            mine
        });
        if mine {
            render(app);
            super::summary::refresh_space(app);
        }
    }

    /// «Detener» descarta la búsqueda y vuelve a «Dónde buscar».
    fn stop(app: &AppWindow) {
        if let Some(c) = STATE.with_borrow_mut(|st| {
            st.timer = None;
            st.cancel.take()
        }) {
            c.store(true, Ordering::Relaxed);
        }
        choose(app);
    }

    /// Casilla de la copia `f` del grupo `g` (fila de `DupsData.groups`). La fila se rehace
    /// siempre: así la casilla vuelve a su sitio si no se podía cambiar.
    fn toggle(app: &AppWindow, g: i32, f: i32) {
        let locked = app.get_locked();
        let Some((r, (send, can))) = STATE.with_borrow_mut(|st| {
            let gi = *st.shown.get(usize::try_from(g).ok()?)?;
            let group = st.groups.as_mut()?.get_mut(gi)?;
            let f = usize::try_from(f).ok().filter(|&f| f < group.copies.len())?;
            let marks: Vec<bool> = group.copies.iter().map(|c| c.marked).collect();
            let c = &mut group.copies[f];
            if !locked && !c.protected && (c.marked || can_mark(&marks, f)) {
                c.marked = !c.marked;
            }
            let r = row(group);
            Some((r, send_text(st)))
        }) else {
            return;
        };
        let data = app.global::<DupsData>();
        data.get_groups().set_row_data(g as usize, r);
        data.set_send_text(send.into());
        data.set_can_send(can);
    }

    /// «Enviar … a la papelera»: pregunta con una línea por grupo.
    fn ask(&self) {
        let Some(app) = self.app.upgrade().filter(|a| !a.get_locked()) else { return };
        let Some(zones) = self.space.zones() else { return };
        // Se vuelven a mirar las zonas al pulsar.
        let mut groups = STATE.with_borrow(picked);
        for g in &mut groups {
            g.0.retain(|(p, _, _)| p.to_str().is_some_and(|s| zones.check(s).is_none()));
        }
        groups.retain(|g| !g.0.is_empty());
        let n: u64 = groups.iter().map(|g| g.0.len() as u64).sum();
        if n == 0 {
            return;
        }
        let mut lines: Vec<ModalLine> = groups
            .iter()
            .take(5)
            .map(|(v, name)| ModalLine { label: name.as_str().into(), value: format::bytes(v.iter().map(|f| f.2).sum()).into() })
            .collect();
        if groups.len() > 5 {
            let m = groups.len() as u64 - 5;
            lines.push(ModalLine { label: format!("y {} más", plural(m, "grupo", "grupos")).into(), value: "".into() });
        }
        let ask = Confirm {
            title: &format!("¿Enviar {} a la papelera?", plural(n, "copia", "copias")),
            text: "",
            lines,
            note: "Podrás recuperarlas desde la papelera mientras no la vacíes. El espacio no se libera hasta entonces.",
            confirm_text: "Enviar a la papelera",
            danger: false,
            alt_text: "",
        };
        let k = self.clone();
        let items: Vec<Pick> = groups.into_iter().flat_map(|g| g.0).collect();
        confirm(&app, ask, move || k.send(items));
    }

    /// Envía las copias `items`. En el hilo, una copia que ya no es igual
    /// a la que se conserva no se envía.
    fn send(&self, items: Vec<Pick>) {
        let paths: Vec<PathBuf> = items.iter().map(|f| f.0.clone()).collect();
        let names: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
        let size = items.iter().map(|f| f.2).sum();
        let pairs: HashMap<PathBuf, (PathBuf, u64)> = items.iter().map(|(m, k, s)| (m.clone(), (k.clone(), *s))).collect();
        let still: Still = Box::new(move |p| pairs.get(p).is_some_and(|(k, s)| still_duplicate(k, p, *s)));
        let k = self.clone();
        let retry: Retry = Rc::new(move |_, _| k.ask());
        let space = self.space.clone();
        let after = move |app: &AppWindow, rep: &win::recycle::Report| {
            let failed: HashSet<&PathBuf> = rep.failed.iter().map(|f| &f.0).collect();
            let gone: Vec<(PathBuf, u64)> = items.into_iter().filter(|f| !failed.contains(&f.0)).map(|f| (f.0, f.2)).collect();
            space.forget(&gone);
            let gone: HashSet<PathBuf> = gone.into_iter().map(|f| f.0).collect();
            STATE.with_borrow_mut(|st| {
                if let Some(groups) = &mut st.groups {
                    for g in groups.iter_mut() {
                        g.copies.retain(|c| !gone.contains(&c.file.path));
                    }
                    groups.retain(|g| g.copies.len() >= 2);
                }
            });
            if app.global::<DupsData>().get_phase() == 2 {
                render(app);
            }
            super::summary::refresh_space(app);
        };
        self.space.send_to_bin(paths, names, size, retry, Some(still), after);
    }
}
