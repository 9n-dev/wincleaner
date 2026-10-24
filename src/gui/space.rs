//! Sección Espacio, pestaña Carpetas: análisis de una unidad en un hilo, árbol navegable y
//! borrado (papelera o del todo) con deshacer y reintentar, compartido con las demás pestañas.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel, Weak};
use wincleaner::categories::is_link;
use wincleaner::space::deletable;
use wincleaner::space::recycle::{Outcome, Reason, outcome_text, outermost, undo_text};
use wincleaner::space::tree::{Id, Kind, Tree};
use wincleaner::space::zones::{Block, Zones};
use wincleaner::ui::{self, AppWindow, Confirm, DriveItem, ModalLine, SpaceData, SpaceRow, StripPart, confirm_with_alt};
use wincleaner::{clean, format};

use crate::win;
use crate::win::recycle::Report;

/// Un análisis concreto: raíz de la unidad y cuándo terminó. Los `Id` solo valen en ese árbol.
type At = (String, Instant);
type After = Box<dyn FnOnce(&AppWindow, &Report)>;
type Retry = Rc<dyn Fn(&Space, &AppWindow)>;

/// Lo que deja el último borrado para «Deshacer» y «Reintentar».
#[derive(Default)]
struct Last {
    names: Vec<String>,
    binned: Vec<(PathBuf, PathBuf)>,
    /// Nodos enviados a la papelera, en el árbol `at`: al deshacer vuelven a verse.
    at: Option<At>,
    ids: Vec<(PathBuf, Id)>,
    /// Lo pone quien pidió el borrado, después de pedirlo.
    retry: Option<Retry>,
}

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
    last: Last,
    /// Lo que hace quien pidió el borrado en curso, al volver.
    after: Option<After>,
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
        can_enter: n.can_enter(),
        can_delete: deletable(n, block, zones.is_some()),
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

/// El análisis que se ve.
fn here(st: &State) -> Option<At> {
    st.trees.get(&st.root).map(|t| (st.root.clone(), t.1))
}

/// El árbol de `at`, si sigue siendo ese análisis (no se ha vuelto a analizar).
fn tree_at<'a>(trees: &'a mut HashMap<String, (Tree, Instant)>, at: &Option<At>) -> Option<&'a mut Tree> {
    let (root, when) = at.as_ref()?;
    trees.get_mut(root).filter(|t| t.1 == *when).map(|t| &mut t.0)
}

/// (ruta, nombre, tamaño) del nodo `id` de `at` si todavía se puede borrar.
fn target(st: &mut State, at: &At, id: Id) -> Option<(PathBuf, String, u64)> {
    let State { trees, zones, .. } = st;
    let tree = tree_at(trees, &Some(at.clone()))?;
    let n = tree.node(id);
    let path = tree.path(id);
    let zones = zones.as_ref();
    let block = zones.and_then(|z| z.check(&path.to_string_lossy()));
    (!n.gone && deletable(n, block, zones.is_some())).then(|| (path, name(tree, id, zones), n.size))
}

/// Sigue ahí, aunque no se pueda leer: solo `NotFound` cuenta como borrado.
fn exists(p: &Path) -> bool {
    !matches!(std::fs::symlink_metadata(p), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// Borra del todo lo que pase las zonas sobre la ruta escrita y la final; lo demás es `Protected`.
/// Lo que sigue en pie después falla con un motivo genérico (`purge` no dice cuál).
fn purge_all(items: &[PathBuf], zones: &Zones) -> Report {
    let mut rep = Report::default();
    for p in items {
        if !win::recycle::vetted(p, zones) {
            rep.failed.push((p.clone(), Reason::Protected));
            continue;
        }
        clean::purge(p);
        if exists(p) {
            rep.failed.push((p.clone(), Reason::Other(0)));
        }
    }
    rep
}

fn set_note(app: &AppWindow, (ok, title, text): (bool, String, String), undo: bool, retry: bool) {
    let data = app.global::<SpaceData>();
    data.set_note(if ok { 1 } else { 2 });
    data.set_note_title(title.into());
    data.set_note_text(text.into());
    data.set_can_undo(undo);
    data.set_can_retry(retry);
}

#[derive(Clone)]
pub struct Space(Weak<AppWindow>);

impl Space {
    pub fn new(app: &AppWindow) -> Space {
        let c = Space(app.as_weak());
        let data = app.global::<SpaceData>();
        let zones = win::space_zones(&[]);
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
                tree.node(id).can_enter().then_some(id)
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
        let k = c.clone();
        data.on_recycle(move |i| k.with(|app| k.recycle(app, i)));
        let k = c.clone();
        data.on_undo(move || k.with(|app| k.undo(app)));
        let k = c.clone();
        data.on_retry(move || {
            k.with(|app| {
                let retry = STATE.with_borrow(|st| st.last.retry.clone());
                if let Some(f) = retry.filter(|_| !app.get_locked()) {
                    f(&k, app);
                }
            })
        });
        let k = c.clone();
        data.on_dismiss(move || {
            k.with(|app| {
                if !app.get_locked() {
                    STATE.with_borrow_mut(|st| st.last = Last::default());
                    app.global::<SpaceData>().set_note(0);
                }
            })
        });
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
        if root.is_empty() || app.get_locked() || STATE.with_borrow(|st| st.cancel.is_some()) {
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

    /// «Papelera» en la fila `i`: pregunta, con «Borrar del todo…» como alternativa.
    fn recycle(&self, app: &AppWindow, i: i32) {
        if app.get_locked() {
            return;
        }
        let Some((at, id, (_, name, size))) = STATE.with_borrow_mut(|st| {
            let id = *st.kids.get(usize::try_from(i).ok()?)?;
            let at = here(st)?;
            let t = target(st, &at, id)?;
            Some((at, id, t))
        }) else {
            return;
        };
        let lines = vec![ModalLine { label: name.into(), value: format::bytes(size).into() }];
        let ask = Confirm {
            title: "¿Enviar a la papelera?",
            text: "",
            lines: lines.clone(),
            note: "Podrás recuperarlo desde la papelera mientras no la vacíes. El espacio no se libera hasta entonces.",
            confirm_text: "Enviar a la papelera",
            danger: false,
            alt_text: "Borrar del todo…",
        };
        let (k, at2) = (self.clone(), at.clone());
        let on_ok = move || k.with(|app| k.trash(app, at, id, false));
        let k = self.clone();
        let on_alt = move || {
            k.with(|app| {
                let text = format!("No pasa por la papelera: se borra ya y libera {} al momento.", format::bytes(size));
                let k2 = k.clone();
                ui::purge(app, lines, &text, move || k2.with(|app| k2.trash(app, at2, id, true)));
            })
        };
        confirm_with_alt(app, ask, on_ok, on_alt);
    }

    /// Envía a la papelera (o borra del todo) el nodo `id` de `at`, si se sigue pudiendo.
    fn trash(&self, app: &AppWindow, at: At, id: Id, purge: bool) {
        if app.get_locked() {
            return;
        }
        let Some((path, name, size)) = STATE.with_borrow_mut(|st| target(st, &at, id)) else { return };
        let gone = Some(at.clone());
        let after = move |_: &AppWindow, rep: &Report| {
            if rep.failed.is_empty() {
                STATE.with_borrow_mut(|st| tree_at(&mut st.trees, &gone).map(|t| t.remove(id)));
            }
        };
        if purge {
            self.delete(true, vec![path], vec![name], size, Box::new(after));
        } else {
            self.send_to_bin(vec![path], vec![name], size, after);
        }
        let retry: Retry = Rc::new(move |k, app| k.trash(app, at.clone(), id, purge));
        STATE.with_borrow_mut(|st| st.last.retry = Some(retry));
    }

    /// Envía `items` a la papelera en un hilo, con la ventana bloqueada. `names` va en paralelo a
    /// `items` y `size` es lo que suman. Al volver, rehace lo que falló en el árbol, llama a
    /// `after` (lo que no está en `failed` ya no existe), rellena el aviso y desbloquea.
    /// Para «Reintentar», quien llama pone `STATE.last.retry` después.
    pub(super) fn send_to_bin(&self, items: Vec<PathBuf>, names: Vec<String>, size: u64, after: impl FnOnce(&AppWindow, &Report) + 'static) {
        self.delete(false, items, names, size, Box::new(after));
    }

    fn delete(&self, purge: bool, items: Vec<PathBuf>, names: Vec<String>, size: u64, after: After) {
        let Some(app) = self.0.upgrade().filter(|a| !a.get_locked()) else { return };
        let Some(zones) = STATE.with_borrow(|st| st.zones.clone()) else { return };
        // Sin repetidos ni lo que ya va dentro de otro.
        let keep = outermost(&items);
        let names = if names.len() == items.len() { keep.iter().map(|&i| names[i].clone()).collect() } else { names };
        let items: Vec<PathBuf> = keep.iter().map(|&i| items[i].clone()).collect();
        // Un borrado nuevo sustituye al aviso anterior y pierde su deshacer.
        let at = STATE.with_borrow_mut(|st| {
            st.last = Last::default();
            st.after = Some(after);
            here(st)
        });
        app.global::<SpaceData>().set_note(0);
        app.set_locked(true);
        let k = self.clone();
        std::thread::spawn(move || {
            let mut rep = if purge { purge_all(&items, &zones) } else { win::recycle::send(&items, &zones) };
            // Lo que ya no existe (otro programa lo borró antes) está hecho.
            rep.failed.retain(|(p, _)| exists(p));
            // Lo que queda de cada fallo: una carpeta se vuelve a leer para el árbol.
            let (mut left, mut scans) = (0, Vec::new());
            for (p, _) in &rep.failed {
                match std::fs::symlink_metadata(p) {
                    Ok(md) if md.is_dir() && !is_link(&md) => {
                        if let Some(t) = Tree::scan(p, &AtomicBool::new(false), &mut |_, _, _| {}) {
                            left += t.node(t.root()).size;
                            scans.push((p.clone(), t));
                        }
                    }
                    Ok(md) => left += md.len(),
                    Err(_) => {}
                }
            }
            let failed = rep.failed.iter().map(|f| f.1).collect();
            let outcome = Outcome { names, sent: size.saturating_sub(left), left, failed };
            let _ = k.0.upgrade_in_event_loop(move |app| Self::deleted(&app, purge, at, rep, scans, outcome));
        });
    }

    fn deleted(app: &AppWindow, purge: bool, at: Option<At>, rep: Report, scans: Vec<(PathBuf, Tree)>, outcome: Outcome) {
        let after = STATE.with_borrow_mut(|st| {
            if let Some(tree) = tree_at(&mut st.trees, &at) {
                for (p, t) in scans {
                    if let Some(id) = tree.find(&p) {
                        tree.graft(id, t);
                    }
                }
                st.last.ids = rep.binned.iter().filter_map(|(p, _)| Some((p.clone(), tree.find(p)?))).collect();
            }
            st.after.take()
        });
        if let Some(f) = after {
            f(app, &rep);
        }
        let undo = !rep.binned.is_empty();
        let retry = STATE.with_borrow_mut(|st| {
            st.last.at = at;
            st.last.names = outcome.names.clone();
            st.last.binned = rep.binned;
            st.last.retry.is_some()
        });
        set_note(app, outcome_text(&outcome, purge), undo, retry && !outcome.failed.is_empty());
        app.set_locked(false);
        units(app);
        show(app);
    }

    /// «Deshacer»: devuelve lo enviado a la papelera, en un hilo.
    fn undo(&self, app: &AppWindow) {
        if app.get_locked() {
            return;
        }
        let Last { names, binned, at, ids, .. } = STATE.with_borrow_mut(|st| std::mem::take(&mut st.last));
        if binned.is_empty() {
            return;
        }
        app.global::<SpaceData>().set_note(0);
        app.set_locked(true);
        let k = self.clone();
        std::thread::spawn(move || {
            let failed = win::recycle::undo(&binned);
            let _ = k.0.upgrade_in_event_loop(move |app| Self::undone(&app, names, at, ids, failed));
        });
    }

    fn undone(app: &AppWindow, names: Vec<String>, at: Option<At>, ids: Vec<(PathBuf, Id)>, failed: Vec<PathBuf>) {
        STATE.with_borrow_mut(|st| {
            if let Some(tree) = tree_at(&mut st.trees, &at) {
                for (p, id) in &ids {
                    if !failed.contains(p) {
                        tree.restore(*id);
                    }
                }
            }
        });
        // Con un solo elemento, su nombre de la lista («Vídeos»); si no, el del archivo.
        let failed: Vec<String> = if failed.len() == 1 && names.len() == 1 {
            names.clone()
        } else {
            failed.iter().map(|p| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())).collect()
        };
        set_note(app, undo_text(&names, &failed), false, false);
        app.set_locked(false);
        units(app);
        show(app);
    }
}
