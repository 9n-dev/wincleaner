//! Sección Espacio, pestaña Carpetas: análisis de una unidad en un hilo, árbol navegable y
//! borrado (papelera o del todo) con deshacer y reintentar, compartido con las demás pestañas.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, Timer, TimerMode, VecModel, Weak};
use wincleaner::categories::is_link;
use wincleaner::paths::strip_verbatim;
use wincleaner::space::dups::{kind, kind_label};
use wincleaner::space::notice::{self, Act, Ctx};
use wincleaner::space::recycle::{Fail, Outcome, Reason, done_text, outermost, undo_text};
use wincleaner::space::tree::{self, Id, Kind, Node, Tree, files_under, read_files};
use wincleaner::space::zones::{Block, Lock, Zones};
use wincleaner::space::{count_text, deletable, why};
use wincleaner::ui::{self, AppWindow, Confirm, DriveItem, ModalLine, SpaceData, SpaceRow, StripPart, confirm_with_alt};
use wincleaner::{clean, format};

use super::games;
use crate::win;
use crate::win::recycle::Report;

/// Un análisis concreto: raíz de la unidad y cuándo terminó. Los `Id` solo valen en ese árbol.
type At = (String, Instant);
type After = Box<dyn FnOnce(&AppWindow, &Report)>;
/// Lo que hacen los botones de los avisos que llevan a otra sección (`Space::on_act`).
type Route = Rc<dyn Fn(&AppWindow, Act)>;
/// «Reintentar»: lo pone quien pide el borrado.
pub(super) type Retry = Rc<dyn Fn(&Space, &AppWindow)>;
/// Se mira en el hilo, justo antes de enviar cada elemento: `false` lo deja fuera como `Changed`.
pub(super) type Still = Box<dyn Fn(&Path) -> bool + Send>;

/// Lo que deja el último borrado para «Deshacer», «Reintentar» y «Borrar del todo…».
#[derive(Default)]
struct Last {
    names: Vec<String>,
    binned: Vec<(PathBuf, PathBuf)>,
    /// Nodos enviados a la papelera, con el árbol de cada uno: al deshacer vuelven a verse.
    ids: Vec<(At, PathBuf, Id)>,
    retry: Option<Retry>,
    /// Solo desde Carpetas: vuelve a preguntar «¿Borrar del todo?».
    purge: Option<Retry>,
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
    /// Hay una carpeta leyéndose al entrar: `true` en su `cancel` la detiene; el `Timer` pinta la
    /// cuenta; el número es `rev` al empezar.
    reading: Option<(Arc<AtomicBool>, Timer, u64)>,
    /// Sube cada vez que un borrado, un «Deshacer» o `forget` cambian los árboles: una lectura
    /// empezada antes ya no cuadra con el árbol y se repite.
    rev: u64,
    last: Last,
    /// Lo que hace quien pidió el borrado en curso, al volver.
    after: Option<After>,
    route: Option<Route>,
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

/// Zona en la que cae la fila; un `Rest` no tiene ruta propia.
fn lock_of(n: &Node, path: &Path, zones: Option<&Zones>) -> Option<Lock> {
    if matches!(n.kind, Kind::Rest | Kind::Opened) {
        return None;
    }
    zones.and_then(|z| z.lock(&path.to_string_lossy()))
}

/// La pastilla de la fila y su icono (decisión 12).
fn badge(lock: Option<Lock>, kind: Kind) -> (&'static str, &'static str) {
    match (kind, lock) {
        (Kind::Link, _) => ("Enlace", "link"),
        (Kind::Denied, _) => ("Sin acceso", "lock"),
        (_, Some(Lock::Game)) => ("Juego", "game"),
        (_, Some(Lock::HoldsGames)) => ("Juegos", "game"),
        (_, Some(Lock::Programs)) => ("Programa", "apps"),
        (_, Some(Lock::Known(_) | Lock::Profile | Lock::HoldsKnown(_))) => ("Carpeta de Windows", "lock"),
        (_, Some(Lock::SystemFile)) => ("Sistema", ""),
        (_, Some(Lock::Me)) => ("WinCleaner", "lock"),
        (_, Some(Lock::Unclear | Lock::Drive | Lock::Windows | Lock::Bin | Lock::Users | Lock::OtherUser)) => ("Protegida", "lock"),
        (_, None) => ("", ""),
    }
}

/// Fila `i` (por tamaño) de una carpeta que ocupa `total`.
fn row(tree: &Tree, id: Id, i: usize, total: u64, zones: Option<&Zones>) -> SpaceRow {
    let n = tree.node(id);
    let path = tree.path(id);
    let lock = lock_of(n, &path, zones);
    let (pill, pill_icon) = badge(lock, n.kind);
    let icon = match (n.kind, lock) {
        (Kind::Link, _) => "link",
        (Kind::Denied, _) => "lock",
        (_, Some(Lock::Programs)) => "apps",
        (_, Some(l)) if l.block() == Block::Protected => "lock",
        (_, Some(Lock::Game | Lock::HoldsGames)) => "game",
        (Kind::Dir, _) => "folder",
        _ => "file",
    };
    // Los archivos de sistema de la raíz dicen qué son.
    let system = match n.name.to_lowercase().as_str() {
        _ if lock != Some(Lock::SystemFile) => None,
        "pagefile.sys" => Some("Memoria virtual de Windows"),
        "hiberfil.sys" => Some("Archivo de hibernación de Windows"),
        "swapfile.sys" => Some("Archivo de intercambio de Windows"),
        _ => None,
    };
    let sub = match (n.kind, system) {
        (_, Some(s)) => s.into(),
        (Kind::File, _) => format!(
            "{} · {}",
            kind_label(kind(&n.name)),
            format::date_dmy(format::ymd(UNIX_EPOCH + Duration::from_secs(n.mtime.into())))
        ),
        (Kind::Rest | Kind::Opened, _) => "Juntos".into(),
        // Sin seguirlo: solo se lee a dónde apunta.
        (Kind::Link, _) => match std::fs::read_link(&path) {
            Ok(to) => {
                let to = to.to_string_lossy();
                format!("{} → {}", path.display(), strip_verbatim(&to).unwrap_or(&to))
            }
            Err(_) => path.display().to_string(),
        },
        (Kind::Dir | Kind::Denied, _) => path.display().to_string(),
    };
    let denied = n.kind == Kind::Denied;
    let real = !matches!(n.kind, Kind::Rest | Kind::Opened);
    SpaceRow {
        icon: icon.into(),
        name: name(tree, id, zones).into(),
        sub: sub.into(),
        size: if denied { "—".into() } else { format::bytes(n.size).into() },
        pct: if denied { "".into() } else { format::percent(n.size, total).into() },
        share: if total == 0 { 0.0 } else { (n.size as f64 / total as f64) as f32 },
        // Los cinco más grandes llevan su color de la tira; el `Rest` va con el resto.
        color: if i < 5 && n.size > 0 && real { i as i32 } else { 5 },
        badge: pill.into(),
        badge_icon: pill_icon.into(),
        // Un nombre no Unicode no da una ruta exacta: solo se ve.
        can_enter: n.can_enter(),
        // El Explorador decide qué abrir (`space::explorer_target`); abrir nunca borra.
        can_open: real,
        selectable: real,
    }
}

/// Filas de la carpeta que se ve, construidas al vuelo: Slint solo pide las visibles.
struct Rows {
    ids: Vec<Id>,
    total: u64,
    notify: ModelNotify,
}

impl Model for Rows {
    type Data = SpaceRow;

    fn row_count(&self) -> usize {
        self.ids.len()
    }

    /// Solo lee `STATE`: nunca se pide desde dentro de un `with_borrow_mut`. Si el árbol ya no
    /// es el de estas filas (otra carpeta, otro análisis), da una fila vacía.
    fn row_data(&self, i: usize) -> Option<SpaceRow> {
        let &id = self.ids.get(i)?;
        let r = STATE.with_borrow(|st| {
            let (tree, _) = st.trees.get(&st.root)?;
            (st.kids.get(i) == Some(&id)).then(|| row(tree, id, i, self.total, st.zones.as_ref()))
        });
        Some(r.unwrap_or_default())
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
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

/// Migas de la carpeta `cur`: «Este equipo», la raíz y sus carpetas.
fn crumbs(tree: &Tree, cur: Id, zones: Option<&Zones>) -> Vec<SharedString> {
    let mut v = vec!["Este equipo".into()];
    v.extend(tree.crumbs(cur).into_iter().map(|c| name(tree, c, zones).into()));
    v
}

/// La carpeta `STATE.cur` de la unidad `STATE.root`; sin análisis, las unidades. Si le faltan
/// archivos (tiene un `Rest`), antes los lee en un hilo.
fn show(app: &AppWindow) {
    let dir = STATE.with_borrow(|st| {
        let (tree, _) = st.trees.get(&st.root)?;
        tree.needs_read(st.cur).then(|| tree.path(st.cur))
    });
    match dir {
        Some(dir) => read(app, dir),
        None => paint(app),
    }
}

/// Pinta la carpeta `STATE.cur` tal como está en el árbol, sin leer nada.
fn paint(app: &AppWindow) {
    let view = STATE.with_borrow_mut(|st| {
        let State { trees, root, cur, kids, zones, .. } = st;
        let (tree, _) = trees.get(root)?;
        let zones = zones.as_ref();
        *kids = tree.children(*cur);
        let total = tree.node(*cur).size;
        // La tira sale de las cinco primeras filas; las demás se piden al pintarse.
        let mut strip: Vec<StripPart> = kids
            .iter()
            .take(5)
            .enumerate()
            .map(|(i, &id)| row(tree, id, i, total, zones))
            .filter(|r| r.color < 5)
            .map(|r| StripPart { label: r.name, share: r.share, color: r.color })
            .collect();
        let rest = 1.0 - strip.iter().map(|p| p.share).sum::<f32>();
        if !strip.is_empty() && rest > 0.001 {
            strip.push(StripPart { label: "Resto".into(), share: rest, color: 5 });
        }
        let (mut dirs, mut files) = (0, 0);
        for &k in kids.iter() {
            match tree.node(k).kind {
                Kind::Dir | Kind::Denied | Kind::Link => dirs += 1,
                Kind::File => files += 1,
                // Un `Rest` sin leer cuenta sus archivos.
                Kind::Rest => files += u64::from(tree.node(k).len),
                // No sale: `children()` enseña sus archivos.
                Kind::Opened => {}
            }
        }
        let count = count_text(dirs, files, (*cur != tree.root()).then_some(total));
        let denied = kids.iter().any(|&k| tree.node(k).kind == Kind::Denied && !tree.node(k).odd);
        let rows = Rows { ids: kids.clone(), total, notify: ModelNotify::default() };
        Some((rows, strip, crumbs(tree, *cur, zones), denied, count, subtitle(st)))
    });
    let Some((rows, strip, crumbs, denied, count, sub)) = view else { return units(app) };
    let data = app.global::<SpaceData>();
    data.set_rows(ModelRc::new(rows));
    data.set_strip(model(strip));
    data.set_crumbs(model(crumbs));
    data.set_denied(denied);
    data.set_count(count.into());
    data.set_selected(-1);
    data.set_subtitle(sub.into());
    data.set_phase(2);
}

/// Detiene la lectura en marcha, si hay: su resultado se descarta.
fn stop_reading(st: &mut State) {
    if let Some((c, ..)) = st.reading.take() {
        c.store(true, Ordering::Relaxed);
    }
}

/// «Leyendo la carpeta…»: lee en un hilo los archivos de `dir` (la carpeta `STATE.cur`) y, al
/// volver, los cuelga del árbol. Mientras, no hay filas: no se puede borrar nada.
fn read(app: &AppWindow, dir: PathBuf) {
    let cancel = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(AtomicU64::new(0));
    let timer = Timer::default();
    let (weak, n) = (app.as_weak(), Arc::clone(&seen));
    timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
        if let Some(app) = weak.upgrade() {
            let n = n.load(Ordering::Relaxed);
            app.global::<SpaceData>().set_reading(format!("{} archivos", format::count(n)).into());
        }
    });
    let view = STATE.with_borrow_mut(|st| {
        let at = here(st)?;
        let (tree, _) = st.trees.get(&st.root)?;
        let view = (at, st.cur, crumbs(tree, st.cur, st.zones.as_ref()), subtitle(st));
        stop_reading(st);
        st.reading = Some((Arc::clone(&cancel), timer, st.rev));
        st.kids.clear();
        Some(view)
    });
    let Some((at, cur, crumbs, sub)) = view else { return units(app) };
    let data = app.global::<SpaceData>();
    data.set_rows(model(Vec::new()));
    data.set_strip(model(Vec::new()));
    data.set_crumbs(model(crumbs));
    data.set_denied(false);
    data.set_selected(-1);
    data.set_reading("".into());
    data.set_subtitle(sub.into());
    data.set_phase(3);
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let files = read_files(&dir, &cancel, &seen);
        let _ = weak.upgrade_in_event_loop(move |app| read_done(&app, &cancel, at, cur, files));
    });
}

/// Vuelve la lectura: si sigue siendo la de la carpeta que se ve, cuelga los archivos y la pinta.
/// Si no se pudo leer, se ve tal cual, con su `Rest`. Si el árbol cambió mientras se leía (un
/// borrado, «Deshacer»), lo leído puede no cuadrar: se descarta y se vuelve a leer.
fn read_done(app: &AppWindow, cancel: &Arc<AtomicBool>, at: At, cur: Id, files: Option<Vec<tree::Entry>>) {
    enum Then {
        Nothing,
        Paint,
        Again,
    }
    let then = STATE.with_borrow_mut(|st| {
        let Some((.., rev)) = st.reading.take_if(|r| Arc::ptr_eq(&r.0, cancel)) else { return Then::Nothing };
        if here(st) != Some(at) || st.cur != cur {
            return Then::Nothing;
        }
        if rev != st.rev {
            return Then::Again;
        }
        if let (Some(files), Some((tree, _))) = (files, st.trees.get_mut(&st.root)) {
            tree.expand(cur, files);
        }
        Then::Paint
    });
    match then {
        Then::Paint => paint(app),
        Then::Again => show(app),
        Then::Nothing => {}
    }
}

/// El análisis que se ve.
fn here(st: &State) -> Option<At> {
    st.trees.get(&st.root).map(|t| (st.root.clone(), t.1))
}

/// Repinta Carpetas, solo si se ve, según su fase: las unidades se vuelven a leer y la carpeta
/// se rehace. Mientras se analiza no se toca nada (el análisis sigue en marcha).
fn repaint(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    if data.get_tab() != 0 {
        return;
    }
    match data.get_phase() {
        0 => units(app),
        2 => {
            units(app);
            paint(app);
        }
        _ => {}
    }
}

/// El análisis (y su árbol) en el que está `p`, si alguna unidad analizada lo tiene.
fn tree_of<'a>(trees: &'a mut HashMap<String, (Tree, Instant)>, p: &Path) -> Option<(At, &'a mut Tree)> {
    trees.iter_mut().find(|(_, t)| t.0.find(p).is_some()).map(|(r, t)| ((r.clone(), t.1), &mut t.0))
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
/// Cada archivo que no se borra sale con su ruta, su tamaño y su motivo.
fn purge_all(items: &[PathBuf], zones: &Zones) -> Vec<(PathBuf, u64, Reason)> {
    let mut failed = Vec::new();
    for p in items {
        if !win::recycle::vetted(p, zones) {
            failed.push((p.clone(), 0, Reason::Protected));
            continue;
        }
        failed.extend(clean::purge(p).failed);
    }
    failed
}

/// Lo que queda de `p` (una carpeta se vuelve a leer y va a `scans`, para el árbol).
fn measure(p: &Path, scans: &mut Vec<(PathBuf, Tree)>) -> u64 {
    match std::fs::symlink_metadata(p) {
        Ok(md) if md.is_dir() && !is_link(&md) => {
            let Some(t) = Tree::scan(p, &AtomicBool::new(false), &mut |_, _, _| {}) else { return 0 };
            let size = t.node(t.root()).size;
            scans.push((p.to_path_buf(), t));
            size
        }
        Ok(md) => md.len(),
        Err(_) => 0,
    }
}

fn is_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|md| md.is_dir() && !is_link(&md))
}

/// El aviso verde: lo enviado a la papelera (con «Deshacer») o lo borrado del todo.
fn set_note(app: &AppWindow, (title, text): (String, String), undo: bool) {
    let data = app.global::<SpaceData>();
    data.set_note(1);
    data.set_note_title(title.into());
    data.set_note_text(text.into());
    data.set_can_undo(undo);
}

/// Lo que vuelve del hilo de borrado.
struct Done {
    rep: Report,
    /// Lo que queda de cada carpeta que no se borró entera, para el árbol.
    scans: Vec<(PathBuf, Tree)>,
    outcome: Outcome,
    /// Nombres de lo que sí llegó a la papelera.
    binned: Vec<String>,
    /// Lo elegido era una sola carpeta.
    folder: bool,
    /// Ruta de lo único que ha fallado (o de lo único que ya no estaba), para la línea del aviso.
    only: Option<PathBuf>,
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().into_owned()
}

/// El borrado en sí, en el hilo de trabajo. `names` va en paralelo a `items` (si no, se usa el
/// nombre del archivo) y `size` es lo que suman. Lo que no pase `still` no se envía y falla
/// como `Changed`.
fn run(purge: bool, items: &[PathBuf], names: &[String], size: u64, still: Option<Still>, zones: &Zones) -> Done {
    let both = names.len() == items.len();
    let label = |i: usize| if both { names[i].clone() } else { file_name(&items[i]) };
    let folder = items.len() == 1 && is_dir(&items[0]);
    // Lo que ya no existía antes de enviar no se envía ni sale como enviado; si no queda
    // nada, el aviso lo dice con los nombres de todo.
    let here: Vec<usize> = (0..items.len()).filter(|&i| exists(&items[i])).collect();
    let missing = !items.is_empty() && here.is_empty();
    // Lo que ocupa cada uno antes de enviarlo: si Windows lo borra sin pasar por la papelera,
    // después ya no se puede medir.
    let was: Vec<u64> = if items.len() == 1 { vec![size] } else { items.iter().map(|p| std::fs::symlink_metadata(p).map_or(0, |m| m.len())).collect() };
    let (send, changed): (Vec<PathBuf>, Vec<PathBuf>) = here.iter().map(|&i| items[i].clone()).partition(|p| still.as_ref().is_none_or(|f| f(p)));
    let mut rep = Report::default();
    let mut fails = if purge {
        purge_all(&send, zones)
    } else {
        rep = win::recycle::send(&send, zones);
        std::mem::take(&mut rep.failed).into_iter().map(|(p, r)| (p, 0, r)).collect()
    };
    fails.extend(changed.into_iter().map(|p| (p, 0, Reason::Changed)));
    // Lo que ya no existe (otro programa lo borró mientras) está hecho. Lo que Windows borró sin
    // pasar por la papelera, también, para quien pidió el borrado, pero el aviso lo cuenta aparte.
    fails.retain(|(p, _, r)| *r == Reason::Purged || exists(p));
    rep.failed = fails.iter().filter(|f| f.2 != Reason::Purged).map(|(p, _, r)| (p.clone(), *r)).collect();

    let mut scans = Vec::new();
    // Una carpeta borrada del todo a medias se vuelve a leer entera, para el árbol. (Si falla ella
    // misma, nada de dentro ha fallado y se mide abajo.)
    for p in send.iter().filter(|p| fails.iter().any(|f| f.0 != **p && f.0.starts_with(p))) {
        measure(p, &mut scans);
    }
    let (mut left, mut busy, mut failed) = (0, Vec::new(), Vec::new());
    for (p, known, r) in &fails {
        // El elemento elegido del que sale: él mismo o la carpeta que lo contiene.
        let i = items.iter().position(|e| p.starts_with(e));
        let inside = i.is_some_and(|i| *p != items[i]);
        let name = match i {
            Some(i) if inside => p.strip_prefix(&items[i]).map_or_else(|_| file_name(p), |rel| rel.to_string_lossy().into_owned()),
            Some(i) => label(i),
            None => file_name(p),
        };
        let size = match r {
            Reason::Purged => i.map_or(0, |i| was[i]),
            // Lo que no se pudo borrar dentro de la carpeta, con lo que medía al recorrerla.
            _ if inside => *known,
            _ => measure(p, &mut scans),
        };
        if *r == Reason::InUse {
            // ponytail: 1 000 archivos por carpeta; más, si alguna vez no basta.
            if is_dir(p) { busy.extend(files_under(p, 1000)) } else { busy.push(p.clone()) }
        }
        left += size;
        failed.push(Fail { name, size, reason: *r });
    }
    let only = match (fails.as_slice(), items) {
        ([f], _) => Some(f.0.clone()),
        ([], [one]) if missing => Some(one.clone()),
        _ => None,
    };
    if missing {
        left = size;
    }
    let binned = rep.binned.iter().filter_map(|(p, _)| items.iter().position(|e| e == p).map(label)).collect();
    let names = if missing || !both { names.to_vec() } else { here.iter().map(|&i| names[i].clone()).collect() };
    let users = win::rm::who(&busy);
    let outcome = Outcome { names, sent: size.saturating_sub(left), left, failed, users, missing };
    Done { rep, scans, outcome, binned, folder, only }
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
            let target = STATE.with_borrow(|st| {
                let (tree, _) = st.trees.get(&st.root)?;
                let id = *st.kids.get(usize::try_from(i).ok()?)?;
                let n = tree.node(id);
                // Una carpeta sin acceso se abre igual: si Windows no deja entrar, lo dice el
                // Explorador. Un enlace va como archivo: se selecciona sin seguirlo.
                (n.kind != Kind::Rest).then(|| (tree.path(id), matches!(n.kind, Kind::Dir | Kind::Denied), n.odd))
            });
            if let Some((p, is_dir, odd)) = target {
                win::open_in_explorer(&p, is_dir, odd);
            }
        });
        let k = c.clone();
        data.on_recycle(move |i| k.with(|app| k.recycle(app, i)));
        let k = c.clone();
        data.on_undo(move || k.with(|app| k.undo(app)));
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

    /// Las carpetas de los juegos pasan a estar protegidas (pastilla «Juegos») y se repintan las filas.
    pub(super) fn set_games(&self, dirs: &[String]) {
        STATE.with_borrow_mut(|st| st.zones = st.zones.as_ref().map(|z| z.with_games(dirs)));
        // Solo con una carpeta a la vista: mientras se analiza no hay filas que repintar.
        self.with(|app| {
            if app.global::<SpaceData>().get_phase() == 2 {
                paint(app);
            }
        });
    }

    fn with(&self, f: impl FnOnce(&AppWindow)) {
        if let Some(app) = self.0.upgrade() {
            f(&app);
        }
    }

    /// Va a la carpeta que diga `to`; si no hay, a las unidades.
    fn go(&self, to: impl FnOnce(&State) -> Option<Id>) {
        STATE.with_borrow_mut(stop_reading);
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
        STATE.with_borrow_mut(stop_reading);
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
            stop_reading(st);
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

    /// «Borrar…» en la fila `i`: si se puede, pregunta, con «Borrar del todo…» como alternativa;
    /// si no, explica por qué.
    fn recycle(&self, app: &AppWindow, i: i32) {
        if app.get_locked() {
            return;
        }
        let Some((at, id)) = STATE.with_borrow(|st| Some((here(st)?, *st.kids.get(usize::try_from(i).ok()?)?))) else { return };
        let Some((_, name, size)) = STATE.with_borrow_mut(|st| target(st, &at, id)) else {
            return self.why_notice(app, id);
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
        let on_alt = move || k.with(|app| k.ask_purge(app, at2, id));
        confirm_with_alt(app, ask, on_ok, on_alt);
    }

    /// Por qué no se puede borrar el nodo `id` de la carpeta que se ve, en la ventana de aviso.
    /// Sus botones entran en la carpeta, la abren en el Explorador, analizan otra unidad o
    /// llevan a otra sección (`on_act`).
    fn why_notice(&self, app: &AppWindow, id: Id) {
        let admin = app.get_admin();
        let Some((at, nt, path, is_dir, odd)) = STATE.with_borrow(|st| {
            let (tree, _) = st.trees.get(&st.root)?;
            let (n, zones) = (tree.node(id), st.zones.as_ref());
            let path = tree.path(id);
            let why = why(n, lock_of(n, &path, zones), zones.is_some())?;
            // A dónde apunta el enlace, sin seguirlo; su unidad solo si es otra unidad fija.
            let link_to = (n.kind == Kind::Link).then(|| std::fs::read_link(&path).ok()).flatten().map(|to| {
                let to = to.to_string_lossy().into_owned();
                strip_verbatim(&to).map_or(to.clone(), Into::into)
            });
            let link_drive = link_to.as_deref().and_then(|to| {
                let d = st.drives.iter().find(|d| d.root != st.root && to.get(..3).is_some_and(|r| r.eq_ignore_ascii_case(&d.root)))?;
                Some(d.root.trim_end_matches('\\').to_string())
            });
            let (name, p) = (name(tree, id, zones), path.to_string_lossy().into_owned());
            // Lee el estado de Juegos, no este.
            let game = games::by_dir(&p);
            let ctx = Ctx {
                name: &name,
                path: &p,
                size: (!matches!(n.kind, Kind::Denied | Kind::Link)).then_some(n.size),
                admin,
                can_enter: n.can_enter(),
                game: game.as_ref().map(|g| (g.0.as_str(), g.1)),
                xbox: game.as_ref().is_some_and(|g| g.2),
                link_to: link_to.as_deref(),
                link_drive: link_drive.as_deref(),
            };
            Some((here(st)?, notice::locked(why, &ctx), path, matches!(n.kind, Kind::Dir | Kind::Denied), n.odd))
        }) else {
            return;
        };
        let k = self.clone();
        ui::notice(app, &nt, move |act| {
            k.with(|app| match act {
                // Solo si sigue siendo el mismo análisis.
                Act::Enter => k.go(|st| (here(st).as_ref() == Some(&at)).then_some(id)),
                Act::Explorer => win::open_in_explorer(&path, is_dir, odd),
                Act::Analyze(root) => {
                    let i = STATE.with_borrow(|st| st.drives.iter().position(|d| d.root.eq_ignore_ascii_case(&root)));
                    if let Some(i) = i {
                        k.analyze(i as i32);
                    }
                }
                act => k.act(app, act),
            })
        });
    }

    /// Un botón de un aviso: «Reintentar» y «Borrar del todo…» repiten lo que dejó el último
    /// borrado (si la ventana no está bloqueada); lo demás va al enrutador (`on_act`).
    fn act(&self, app: &AppWindow, act: Act) {
        let (retry, purge, route) = STATE.with_borrow(|st| (st.last.retry.clone(), st.last.purge.clone(), st.route.clone()));
        let again = match act {
            Act::Retry => retry,
            Act::Purge => purge,
            act => {
                if let Some(f) = route {
                    f(app, act);
                }
                return;
            }
        };
        if let Some(f) = again.filter(|_| !app.get_locked()) {
            f(self, app);
        }
    }

    /// Lo que hacen los botones de los avisos que llevan a otra sección (Juegos, Programas,
    /// Procesos, reiniciar como administrador, buscar duplicados otra vez).
    pub(super) fn on_act(&self, f: impl Fn(&AppWindow, Act) + 'static) {
        STATE.with_borrow_mut(|st| st.route = Some(Rc::new(f)));
    }

    /// «¿Borrar del todo?» para el nodo `id` de `at`, con lo que ocupa ahora. Solo borra si se
    /// escribe BORRAR y se confirma; también desde «Reintentar».
    fn ask_purge(&self, app: &AppWindow, at: At, id: Id) {
        if app.get_locked() {
            return;
        }
        let Some((_, name, size)) = STATE.with_borrow_mut(|st| target(st, &at, id)) else { return };
        let lines = vec![ModalLine { label: name.into(), value: format::bytes(size).into() }];
        let text = format!("No pasa por la papelera: se borra ya y libera {} al momento.", format::bytes(size));
        let k = self.clone();
        ui::purge(app, lines, &text, move || k.with(|app| k.trash(app, at, id, true)));
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
        // Reintentar la papelera se repite tal cual (se puede recuperar); borrar del todo vuelve a pedir BORRAR.
        let at2 = at.clone();
        let retry: Retry = Rc::new(move |k, app| if purge { k.ask_purge(app, at2.clone(), id) } else { k.trash(app, at2.clone(), id, false) });
        let alt: Retry = Rc::new(move |k, app| k.ask_purge(app, at.clone(), id));
        self.delete(purge, Some(alt), vec![path], vec![name], size, Some(retry), None, Box::new(after));
    }

    /// Envía `items` a la papelera en un hilo, con la ventana bloqueada, desde cualquier pestaña.
    /// `names` va en paralelo a `items` y `size` es lo que suman. En el hilo, lo que no pase
    /// `still` no se envía y falla como `Changed`. Al volver, rehace lo que falló en los árboles,
    /// llama a `after` (lo que no está en `failed` ya no existe), rellena el aviso (con
    /// «Reintentar», que llama a `retry`, si algo falló) y desbloquea.
    pub(super) fn send_to_bin(
        &self,
        items: Vec<PathBuf>,
        names: Vec<String>,
        size: u64,
        retry: Retry,
        still: Option<Still>,
        after: impl FnOnce(&AppWindow, &Report) + 'static,
    ) {
        self.delete(false, None, items, names, size, Some(retry), still, Box::new(after));
    }

    /// `alt`: lo que hace «Borrar del todo…» en el aviso (solo desde Carpetas).
    #[allow(clippy::too_many_arguments)]
    fn delete(&self, purge: bool, alt: Option<Retry>, items: Vec<PathBuf>, names: Vec<String>, size: u64, retry: Option<Retry>, still: Option<Still>, after: After) {
        let Some(app) = self.0.upgrade().filter(|a| !a.get_locked()) else { return };
        let Some(zones) = STATE.with_borrow(|st| st.zones.clone()) else { return };
        // Sin repetidos ni lo que ya va dentro de otro.
        let keep = outermost(&items);
        let names = if names.len() == items.len() { keep.iter().map(|&i| names[i].clone()).collect() } else { names };
        let items: Vec<PathBuf> = keep.iter().map(|&i| items[i].clone()).collect();
        // Un borrado nuevo sustituye al aviso anterior y pierde su deshacer.
        STATE.with_borrow_mut(|st| {
            st.last = Last { retry, purge: alt, ..Last::default() };
            st.after = Some(after);
        });
        app.global::<SpaceData>().set_note(0);
        app.set_locked(true);
        let k = self.clone();
        std::thread::spawn(move || {
            let done = run(purge, &items, &names, size, still, &zones);
            let _ = k.0.upgrade_in_event_loop(move |app| Self::deleted(&app, purge, done));
        });
    }

    fn deleted(app: &AppWindow, purge: bool, done: Done) {
        let Done { rep, scans, outcome, binned, folder, only } = done;
        // En el árbol de la unidad de cada elemento, si está analizada (no tiene por qué ser la que se ve).
        let after = STATE.with_borrow_mut(|st| {
            st.rev += 1;
            for (p, t) in scans {
                if let Some((_, tree)) = tree_of(&mut st.trees, &p)
                    && let Some(id) = tree.find(&p)
                {
                    tree.graft(id, t);
                }
            }
            st.last.ids = rep
                .binned
                .iter()
                .filter_map(|(p, _)| {
                    let (at, tree) = tree_of(&mut st.trees, p)?;
                    Some((at, p.clone(), tree.find(p)?))
                })
                .collect();
            st.after.take()
        });
        if let Some(f) = after {
            f(app, &rep);
        }
        let undo = !rep.binned.is_empty();
        let ok = outcome.failed.is_empty() && !outcome.missing;
        // El aviso verde, solo con lo que sí se envió (o se borró del todo).
        let names = if purge { outcome.names.clone() } else { binned };
        let alt = STATE.with_borrow_mut(|st| {
            st.last.names = names.clone();
            st.last.binned = rep.binned;
            st.last.purge.is_some()
        });
        if ok || undo {
            set_note(app, done_text(&Outcome { names, sent: outcome.sent, ..Default::default() }, purge), undo);
        }
        app.set_locked(false);
        repaint(app);
        if !ok {
            let mut nt = notice::failed(&outcome, purge, alt, app.get_admin(), folder);
            // La línea enseña la ruta entera, no solo el nombre (decisión 10).
            if let (Some(p), [line]) = (only, nt.lines.as_mut_slice()) {
                let p = p.to_string_lossy();
                line.0 = strip_verbatim(&p).unwrap_or(&p).to_string();
            }
            let k = Space(app.as_weak());
            ui::notice(app, &nt, move |act| k.with(|app| k.act(app, act)));
        }
    }

    /// «Deshacer»: devuelve lo enviado a la papelera, en un hilo.
    fn undo(&self, app: &AppWindow) {
        if app.get_locked() {
            return;
        }
        let Last { names, binned, ids, .. } = STATE.with_borrow_mut(|st| std::mem::take(&mut st.last));
        if binned.is_empty() {
            return;
        }
        app.global::<SpaceData>().set_note(0);
        app.set_locked(true);
        let k = self.clone();
        std::thread::spawn(move || {
            let failed = win::recycle::undo(&binned);
            let _ = k.0.upgrade_in_event_loop(move |app| Self::undone(&app, names, ids, failed));
        });
    }

    fn undone(app: &AppWindow, names: Vec<String>, ids: Vec<(At, PathBuf, Id)>, failed: Vec<PathBuf>) {
        STATE.with_borrow_mut(|st| {
            st.rev += 1;
            for (at, p, id) in ids {
                if !failed.contains(&p)
                    && let Some(tree) = tree_at(&mut st.trees, &Some(at))
                {
                    tree.restore(id);
                }
            }
        });
        // Con un solo elemento, su nombre de la lista («Vídeos»); si no, el del archivo.
        let failed: Vec<String> = if failed.len() == 1 && names.len() == 1 {
            names.clone()
        } else {
            failed.iter().map(|p| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())).collect()
        };
        app.set_locked(false);
        repaint(app);
        if failed.is_empty() {
            let (_, title, text) = undo_text(&names, &[]);
            set_note(app, (title, text), false);
        } else {
            ui::notice(app, &notice::undo_failed(&failed), |_| {});
        }
    }

    /// Se vuelve a la pestaña Carpetas: se repinta lo que se veía (un borrado desde otra pestaña
    /// puede haber cambiado tamaños y espacio libre). Mientras se analiza no hay nada que repintar.
    pub(super) fn shown(&self) {
        self.with(repaint);
    }

    /// Las zonas protegidas; `None` si no se pudieron leer (entonces no se borra nada).
    pub(super) fn zones(&self) -> Option<Zones> {
        STATE.with_borrow(|st| st.zones.clone())
    }

    /// Lo borrado desde otra pestaña sale de los árboles de todas las unidades analizadas.
    pub(super) fn forget(&self, files: &[(PathBuf, u64)]) {
        STATE.with_borrow_mut(|st| {
            st.rev += 1;
            for (tree, _) in st.trees.values_mut() {
                for (p, size) in files {
                    tree.forget(p, *size);
                }
            }
        });
    }
}
