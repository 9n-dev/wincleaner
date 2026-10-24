//! Sección Espacio, pestaña Carpetas: análisis de una unidad en un hilo y árbol navegable.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel, Weak};
use wincleaner::format;
use wincleaner::space::tree::{Id, Kind, Tree};
use wincleaner::space::zones::{Block, Zones};
use wincleaner::ui::{AppWindow, DriveItem, SpaceData, SpaceRow, StripPart};

use crate::win;

#[derive(Default)]
struct State {
    drives: Vec<win::Drive>,
    /// Último análisis de cada unidad (por su raíz) y cuándo terminó. Vive hasta que se cierra la app.
    trees: HashMap<String, (Tree, Instant)>,
    /// Unidad y carpeta que se ven.
    root: String,
    cur: Id,
    /// Nodo de cada fila de `SpaceData.rows`.
    kids: Vec<Id>,
    /// `None` si no se pudieron leer: no se borra nada.
    zones: Option<Zones>,
    /// Hay un análisis en marcha: `true` en su `cancel` lo detiene.
    cancel: Option<Arc<AtomicBool>>,
    /// Progreso en pantalla cada segundo; vive mientras dura el análisis.
    timer: Option<Timer>,
    /// Refresca «analizado hace X min» cada minuto.
    clock: Timer,
}

thread_local! {
    // Solo vive en el hilo de la interfaz: el hilo de trabajo vuelve con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

/// Lo que cuenta el hilo de trabajo: archivos, bytes leídos y carpeta en curso.
type Shared = Arc<Mutex<(u64, u64, String)>>;

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(v)).into()
}

fn used(d: &win::Drive) -> u64 {
    d.total.saturating_sub(d.free)
}

/// «412 GB usados de 476 GB»
fn drive_text(d: &win::Drive) -> String {
    format!("{} usados de {}", format::bytes(used(d)), format::bytes(d.total))
}

/// «212 GB de 412 GB · 684 210 archivos · 1 min 5 s»
fn progress_text(files: u64, bytes: u64, used: u64, took: Duration) -> String {
    let files = if files == 1 { "1 archivo".into() } else { format!("{} archivos", format::count(files)) };
    format!("{} de {} · {files} · {}", format::bytes(bytes), format::bytes(used), format::duration(took))
}

/// Nombre que se enseña: el de la carpeta conocida si lo es («Vídeos», «Usuarios»), y la raíz sin `\`.
fn name(tree: &Tree, id: Id, zones: Option<&Zones>) -> String {
    let label = zones.and_then(|z| z.label(&tree.path(id).to_string_lossy()));
    let n = &*tree.node(id).name;
    label.map_or_else(|| if id == tree.root() { n.trim_end_matches('\\') } else { n }.to_string(), Into::into)
}

/// Fila `i` (por tamaño) de una carpeta que ocupa `total`.
fn row(tree: &Tree, id: Id, i: usize, total: u64, zones: Option<&Zones>) -> SpaceRow {
    let n = tree.node(id);
    let path = tree.path(id);
    let block = match n.kind {
        Kind::Rest => None,
        _ => zones.and_then(|z| z.check(&path.to_string_lossy())),
    };
    let icon = match (n.kind, block) {
        (Kind::Link, _) => "link",
        (Kind::Denied, _) | (_, Some(Block::Protected)) => "lock",
        (_, Some(Block::Game)) => "game",
        (Kind::Dir, _) => "folder",
        _ => "file",
    };
    let badge = match (n.kind, block) {
        (Kind::Link, _) => "Enlace",
        (Kind::Denied, _) => "Sin acceso",
        (_, Some(Block::Protected)) => "Protegida",
        (_, Some(Block::System)) => "Sistema",
        (_, Some(Block::Game)) => "Juegos",
        _ => "",
    };
    let sub = match n.kind {
        Kind::File => format::date_dmy(format::ymd(UNIX_EPOCH + Duration::from_secs(n.mtime.into()))),
        Kind::Rest => "Juntos".into(),
        _ => path.display().to_string(),
    };
    let denied = n.kind == Kind::Denied;
    SpaceRow {
        icon: icon.into(),
        name: name(tree, id, zones).into(),
        sub: sub.into(),
        size: if denied { "—".into() } else { format::bytes(n.size).into() },
        pct: if denied { "".into() } else { format::percent(n.size, total).into() },
        share: if total == 0 { 0.0 } else { (n.size as f64 / total as f64) as f32 },
        // Los cinco más grandes llevan su color de la tira; el `Rest` va con el resto.
        color: if i < 5 && n.size > 0 && n.kind != Kind::Rest { i as i32 } else { 5 },
        badge: badge.into(),
        // Un nombre no Unicode no da una ruta exacta: solo se ve.
        can_enter: n.kind == Kind::Dir && !n.odd,
        can_delete: zones.is_some() && block.is_none() && !n.odd && matches!(n.kind, Kind::Dir | Kind::File),
        selectable: n.kind != Kind::Rest,
    }
}

/// Nota sobre la lista al elegir la fila `id` si no se puede borrar: (título, texto).
fn lock_note(tree: &Tree, id: Id, zones: Option<&Zones>) -> Option<(String, String)> {
    let n = tree.node(id);
    let name = name(tree, id, zones);
    let note = |t: String, x: &str| Some((t, x.to_string()));
    if n.odd {
        return note(
            format!("No se puede leer bien el nombre de «{name}»."),
            "Tiene caracteres que Windows no deja leer tal cual. Puedes verlo, pero no borrarlo ni entrar desde aquí, para no equivocarte de archivo.",
        );
    }
    match n.kind {
        Kind::Link => {
            return note(
                format!("{name} es un enlace."),
                "Apunta a otra carpeta: no suma a lo que ocupa esta y no se entra en él desde aquí.",
            );
        }
        Kind::Rest | Kind::Denied => return None,
        Kind::Dir | Kind::File => {}
    }
    let Some(zones) = zones else {
        return note(
            "No se puede borrar nada desde aquí.".into(),
            "WinCleaner no ha podido saber qué carpetas de Windows tiene que proteger, así que por seguridad todo es solo para ver.",
        );
    };
    match zones.check(&tree.path(id).to_string_lossy())? {
        Block::Protected => {
            // La primera carpeta con nombre propio del camino dice de qué zona es.
            let zone = tree.crumbs(id).into_iter().find_map(|c| zones.label(&tree.path(c).to_string_lossy()));
            let why = match zone {
                Some("Windows") => {
                    "Puedes ver qué ocupa, pero no borrarla desde aquí: romperías el sistema. Para liberar espacio de Windows usa «Limpiar»."
                }
                Some(z) if z.starts_with("Archivos de programa") => {
                    "Puedes ver qué ocupa, pero no borrarla desde aquí: los programas dejarían de funcionar. Para quitar un programa usa «Programas»."
                }
                _ => "Puedes ver qué ocupa, pero no borrarla desde aquí: Windows o tus programas la necesitan.",
            };
            note(format!("{name} está protegida."), why)
        }
        Block::Known if n.kind == Kind::File => note(format!("{name} es WinCleaner."), "No se puede borrar mientras lo usas."),
        Block::Known => note(
            format!("{name} no se puede borrar."),
            "Es una carpeta que Windows o WinCleaner necesitan. Lo que hay dentro sí: entra y elige qué borrar.",
        ),
        Block::System => note(
            format!("{name} es un archivo del sistema."),
            "Windows lo usa y lo vuelve a crear. Para que ocupe menos, cambia la memoria virtual o la hibernación en la configuración de Windows.",
        ),
        Block::Game => note(
            format!("{name} es de un juego."),
            "Si la borras, el lanzador seguiría creyendo que está instalado. Desinstálalo desde la pestaña Juegos.",
        ),
    }
}

/// «C: · 412 GB usados de 476 GB · analizado hace 2 min»
fn subtitle(st: &State) -> String {
    let name = st.root.trim_end_matches('\\');
    let when = st.trees.get(&st.root).map(|t| format!(" · analizado {}", format::ago(t.1.elapsed()))).unwrap_or_default();
    match st.drives.iter().find(|d| d.root == st.root) {
        Some(d) => format!("{name} · {}{when}", drive_text(d)),
        None => format!("{name}{when}"),
    }
}

/// Las unidades fijas, con «Ver» en las que ya se analizaron.
fn units(app: &AppWindow) {
    let drives = win::fixed_drives();
    let items = STATE.with_borrow_mut(|st| {
        let items: Vec<DriveItem> = drives
            .iter()
            .map(|d| DriveItem {
                name: d.root.trim_end_matches('\\').into(),
                label: d.label.as_str().into(),
                text: drive_text(d).into(),
                used: if d.total == 0 { 0.0 } else { (used(d) as f64 / d.total as f64) as f32 },
                low: format::low_space(d.free, d.total),
                analyzed: st.trees.contains_key(&d.root),
            })
            .collect();
        st.drives = drives;
        items
    });
    let data = app.global::<SpaceData>();
    data.set_drives(model(items));
    data.set_subtitle("Mira a dónde va cada GB y libera lo que no necesites.".into());
    data.set_phase(0);
}

/// La carpeta `STATE.cur` de la unidad `STATE.root`; sin análisis, las unidades.
fn show(app: &AppWindow) {
    let view = STATE.with_borrow_mut(|st| {
        let State { trees, root, cur, kids, zones, .. } = st;
        let (tree, _) = trees.get(root)?;
        let zones = zones.as_ref();
        *kids = tree.children(*cur);
        let total = tree.node(*cur).size;
        let rows: Vec<SpaceRow> = kids.iter().enumerate().map(|(i, &id)| row(tree, id, i, total, zones)).collect();
        let mut strip: Vec<StripPart> =
            rows.iter().filter(|r| r.color < 5).map(|r| StripPart { label: r.name.clone(), share: r.share, color: r.color }).collect();
        let rest = 1.0 - strip.iter().map(|p| p.share).sum::<f32>();
        if !strip.is_empty() && rest > 0.001 {
            strip.push(StripPart { label: "Resto".into(), share: rest, color: 5 });
        }
        let mut crumbs = vec!["Este equipo".into()];
        crumbs.extend(tree.crumbs(*cur).into_iter().map(|c| name(tree, c, zones).into()));
        let denied = kids.iter().any(|&k| tree.node(k).kind == Kind::Denied && !tree.node(k).odd);
        Some((rows, strip, crumbs, denied, subtitle(st)))
    });
    let Some((rows, strip, crumbs, denied, sub)) = view else { return units(app) };
    let data = app.global::<SpaceData>();
    data.set_rows(model(rows));
    data.set_strip(model(strip));
    data.set_crumbs(model(crumbs));
    data.set_denied(denied);
    data.set_selected(-1);
    data.set_lock_title("".into());
    data.set_lock_text("".into());
    data.set_subtitle(sub.into());
    data.set_phase(2);
}

#[derive(Clone)]
pub struct Space(Weak<AppWindow>);

impl Space {
    pub fn new(app: &AppWindow) -> Space {
        let c = Space(app.as_weak());
        let data = app.global::<SpaceData>();
        let zones = win::space_zones(&[]);
        data.set_can_delete_any(zones.is_some());
        let weak = app.as_weak();
        STATE.with_borrow_mut(|st| {
            st.zones = zones;
            st.clock.start(TimerMode::Repeated, Duration::from_secs(60), move || {
                let Some(app) = weak.upgrade() else { return };
                let data = app.global::<SpaceData>();
                if data.get_phase() == 2 {
                    data.set_subtitle(STATE.with_borrow(subtitle).into());
                }
            });
        });
        units(app);

        let k = c.clone();
        data.on_analyze(move |i| k.analyze(i));
        let k = c.clone();
        data.on_reanalyze(move || {
            let root = STATE.with_borrow(|st| st.root.clone());
            k.with(|app| k.start(app, root));
        });
        let k = c.clone();
        data.on_stop(move || k.with(Self::stop));
        let k = c.clone();
        data.on_select(move |i| k.with(|app| Self::select(app, i)));
        let k = c.clone();
        data.on_enter(move |i| {
            k.go(|st| {
                let (tree, _) = st.trees.get(&st.root)?;
                let id = *st.kids.get(usize::try_from(i).ok()?)?;
                let n = tree.node(id);
                (n.kind == Kind::Dir && !n.odd).then_some(id)
            })
        });
        let k = c.clone();
        data.on_up(move || {
            // En la raíz, vuelve a las unidades.
            k.go(|st| {
                let (tree, _) = st.trees.get(&st.root)?;
                (st.cur != tree.root()).then(|| tree.node(st.cur).parent)
            })
        });
        let k = c.clone();
        data.on_crumb(move |i| {
            // La 0 es «Este equipo»; las demás, la raíz y sus carpetas.
            k.go(|st| {
                let (tree, _) = st.trees.get(&st.root)?;
                tree.crumbs(st.cur).get(usize::try_from(i - 1).ok()?).copied()
            })
        });
        data.on_open(|i| {
            let path = STATE.with_borrow(|st| {
                let (tree, _) = st.trees.get(&st.root)?;
                Some(tree.path(*st.kids.get(usize::try_from(i).ok()?)?))
            });
            if let Some(p) = path {
                win::open_in_explorer(&p);
            }
        });
        // Task 6: enviar a la papelera, deshacer y reintentar.
        data.on_recycle(|_| {});
        data.on_undo(|| {});
        data.on_retry(|| {});
        let k = c.clone();
        data.on_dismiss(move || k.with(|app| app.global::<SpaceData>().set_note(0)));
        c
    }

    fn with(&self, f: impl FnOnce(&AppWindow)) {
        if let Some(app) = self.0.upgrade() {
            f(&app);
        }
    }

    /// Va a la carpeta que diga `to`; si no hay, a las unidades.
    fn go(&self, to: impl FnOnce(&State) -> Option<Id>) {
        self.with(|app| match STATE.with_borrow(to) {
            Some(id) => {
                STATE.with_borrow_mut(|st| st.cur = id);
                show(app);
            }
            None => units(app),
        });
    }

    /// «Analizar» o «Ver» en la unidad `i`.
    fn analyze(&self, i: i32) {
        let Some(root) = STATE.with_borrow(|st| st.drives.get(usize::try_from(i).ok()?).map(|d| d.root.clone())) else {
            return;
        };
        if STATE.with_borrow(|st| st.trees.contains_key(&root)) {
            STATE.with_borrow_mut(|st| (st.root, st.cur) = (root, 0));
            return self.with(show);
        }
        self.with(|app| self.start(app, root));
    }

    /// Analiza `root` en un hilo. El análisis anterior de esa unidad se descarta ya: así no hay
    /// dos árboles en memoria.
    fn start(&self, app: &AppWindow, root: String) {
        if root.is_empty() || STATE.with_borrow(|st| st.cancel.is_some()) {
            return;
        }
        let used = STATE.with_borrow(|st| st.drives.iter().find(|d| d.root == root).map_or(0, used));
        let cancel = Arc::new(AtomicBool::new(false));
        let shared: Shared = Arc::default();
        let start = Instant::now();
        let data = app.global::<SpaceData>();
        data.set_subtitle(format!("Analizando {}…", root.trim_end_matches('\\')).into());
        data.set_progress(progress_text(0, 0, used, Duration::ZERO).into());
        data.set_progress_value(0.0);
        data.set_current("".into());
        data.set_phase(1);
        let timer = Timer::default();
        let (weak, sh) = (self.0.clone(), Arc::clone(&shared));
        timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            let (Some(app), Ok(g)) = (weak.upgrade(), sh.lock()) else { return };
            let data = app.global::<SpaceData>();
            data.set_progress(progress_text(g.0, g.1, used, start.elapsed()).into());
            data.set_progress_value(if used == 0 { 0.0 } else { (g.1 as f64 / used as f64).min(1.0) as f32 });
            data.set_current(g.2.as_str().into());
        });
        STATE.with_borrow_mut(|st| {
            st.trees.remove(&root);
            st.cancel = Some(Arc::clone(&cancel));
            st.timer = Some(timer);
        });

        let k = self.clone();
        std::thread::spawn(move || {
            let tree = Tree::scan(Path::new(&root), &cancel, &mut |files, bytes, dir| {
                if let Ok(mut g) = shared.lock() {
                    *g = (files, bytes, dir.display().to_string());
                }
            });
            let _ = k.0.upgrade_in_event_loop(move |app| Self::finished(&app, &cancel, root, tree));
        });
    }

    fn finished(app: &AppWindow, cancel: &Arc<AtomicBool>, root: String, tree: Option<Tree>) {
        // Si se detuvo (o ya hay otro análisis), este resultado no vale.
        let mine = STATE.with_borrow(|st| st.cancel.as_ref().is_some_and(|c| Arc::ptr_eq(c, cancel)));
        if !mine {
            return;
        }
        STATE.with_borrow_mut(|st| {
            st.cancel = None;
            st.timer = None;
            if let Some(t) = tree {
                st.trees.insert(root.clone(), (t, Instant::now()));
                (st.root, st.cur) = (root, 0);
            }
        });
        // El espacio usado puede haber cambiado: se vuelven a leer las unidades.
        units(app);
        show(app);
    }

    /// «Detener» descarta el análisis: unos tamaños a medias no sirven para decidir qué borrar.
    fn stop(app: &AppWindow) {
        if let Some(c) = STATE.with_borrow_mut(|st| {
            st.timer = None;
            st.cancel.take()
        }) {
            c.store(true, Ordering::Relaxed);
        }
        units(app);
    }

    fn select(app: &AppWindow, i: i32) {
        let note = STATE.with_borrow(|st| {
            let (tree, _) = st.trees.get(&st.root)?;
            lock_note(tree, *st.kids.get(usize::try_from(i).ok()?)?, st.zones.as_ref())
        });
        let (title, text) = note.unwrap_or_default();
        let data = app.global::<SpaceData>();
        data.set_lock_title(title.into());
        data.set_lock_text(text.into());
    }
}
