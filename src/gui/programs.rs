//! Sección Programas: la lista de las claves `Uninstall`, con búsqueda, orden e iconos, y la
//! desinstalación con el desinstalador del propio programa. El registro y los iconos se leen
//! en hilos; la interfaz no espera a ninguno.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use slint::{Color, ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel, Weak};
use wincleaner::format;
use wincleaner::programs::{self, Hive, Program, RawEntry, SortKey, WaitState};
use wincleaner::ui::{AppWindow, Confirm, ProgramRow, ProgramsData, SummaryData, confirm};

use super::summary;
use crate::win;
use crate::win::uninstall::{self, Launched, LaunchError};

/// Ruta e índice del icono, en minúsculas.
type IconKey = (String, i32);

#[derive(Default)]
struct State {
    /// Las entradas visibles, tal como se leyeron (la desinstalación las necesita).
    raws: Vec<RawEntry>,
    programs: Vec<Program>,
    /// Lo que se ve, filtrado y ordenado; `selected` y `uninstall(i)` son índices de aquí.
    view: Vec<Program>,
    /// `None`: se intentó y no tiene icono.
    icons: HashMap<IconKey, Option<SharedPixelBuffer<Rgba8Pixel>>>,
    /// El desinstalador en marcha; solo uno a la vez.
    waiting: Option<Waiting>,
    /// Cada segundo, mientras hay `waiting`.
    timer: Timer,
    /// La carpeta que abre «Abrir carpeta» en el aviso 2.
    folder: Option<PathBuf>,
}

struct Waiting {
    launched: Launched,
    hive: Hive,
    key: String,
    name: String,
    size: Option<u64>,
    user_done: bool,
}

thread_local! {
    // Solo en el hilo de la interfaz: los hilos de trabajo vuelven con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

/// Los seis colores de las baldosas de las maquetas.
const TINTS: [u32; 6] = [0x8fb0c4, 0xd68a8a, 0x8fc2c0, 0xc4a7b4, 0xa9c2b8, 0xd6b38a];

/// Color estable para un nombre: no cambia entre ejecuciones ni versiones de Rust.
pub(super) fn tint(name: &str) -> Color {
    let h = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let c = TINTS[h as usize % TINTS.len()];
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub(super) fn letter(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map_or("?".into(), |c| c.to_uppercase().collect())
}

fn icon_key(p: &Program) -> Option<IconKey> {
    p.icon.as_ref().map(|(path, i)| (path.to_lowercase(), *i))
}

fn row(p: &Program, icons: &HashMap<IconKey, Option<SharedPixelBuffer<Rgba8Pixel>>>, admin: bool) -> ProgramRow {
    let icon = icon_key(p).and_then(|k| icons.get(&k).cloned().flatten());
    ProgramRow {
        name: p.name.as_str().into(),
        publisher: p.publisher.as_str().into(),
        date: p.date.map_or("—".into(), format::date_dmy).into(),
        size: p.size.map_or("—".into(), format::bytes).into(),
        icon: icon.map(Image::from_rgba8).unwrap_or_default(),
        letter: letter(&p.name).into(),
        tint: tint(&p.name),
        needs_admin: p.hive != Hive::User && !admin,
        blocked: !programs::may_run(p.hive, admin),
    }
}

fn sort_key(i: i32) -> SortKey {
    match i {
        1 => SortKey::Name,
        2 => SortKey::Date,
        _ => SortKey::Size,
    }
}

#[derive(Clone)]
pub struct Programs(Weak<AppWindow>);

impl Programs {
    pub fn new(app: &AppWindow) -> Programs {
        let c = Programs(app.as_weak());
        let k = c.clone();
        let data = app.global::<ProgramsData>();
        data.on_view_changed(move || k.refresh_view());
        let k = c.clone();
        data.on_uninstall(move |i| k.ask(i));
        let k = c.clone();
        data.on_done_waiting(move || {
            STATE.with_borrow_mut(|st| {
                if let Some(w) = &mut st.waiting {
                    w.user_done = true;
                }
            });
            k.tick();
        });
        let k = c.clone();
        data.on_dismiss(move || {
            if let Some(app) = k.0.upgrade() {
                app.global::<ProgramsData>().set_banner(0);
            }
        });
        data.on_open_folder(|| {
            STATE.with_borrow(|st| {
                if let Some(f) = &st.folder {
                    win::open_folder(f);
                }
            });
        });
        c.reload();
        c
    }

    /// Vuelve a leer el registro en un hilo; la lista de antes se queda hasta que llega la nueva.
    pub fn reload(&self) {
        let k = self.clone();
        std::thread::spawn(move || {
            let raws: Vec<RawEntry> = win::reg::uninstall_entries().into_iter().filter(programs::visible).collect();
            let _ = k.0.clone().upgrade_in_event_loop(move |app| k.loaded(&app, raws));
        });
    }

    fn loaded(&self, app: &AppWindow, raws: Vec<RawEntry>) {
        let programs: Vec<Program> = raws.iter().map(programs::from_raw).collect();
        let total: u64 = programs.iter().filter_map(|p| p.size).sum();
        let n = programs.len();
        let title = format!("{n} {} · {}", if n == 1 { "instalado" } else { "instalados" }, format::bytes(total));
        let biggest = programs.iter().filter_map(|p| Some((p.name.as_str(), p.size?))).max_by_key(|b| b.1);
        summary::set_programs(app, &title, biggest);
        let data = app.global::<ProgramsData>();
        data.set_subtitle(title.into());
        data.set_loading(false);

        // Los iconos que faltan en la caché, sin repetir.
        let pending: HashSet<IconKey> = STATE.with_borrow_mut(|st| {
            let pending = programs.iter().filter_map(icon_key).filter(|k| !st.icons.contains_key(k)).collect();
            st.raws = raws;
            st.programs = programs;
            pending
        });
        self.refresh_view();
        if !pending.is_empty() {
            self.load_icons(pending);
        }
    }

    fn load_icons(&self, pending: HashSet<IconKey>) {
        let k = self.clone();
        std::thread::spawn(move || {
            // SHGetFileInfoW necesita COM en el hilo que lo llama.
            let load = |k: IconKey| {
                let px = win::icon::load(&k.0, k.1);
                (k, px)
            };
            let icons: Vec<_> = win::with_com(|| pending.into_iter().map(load).collect());
            let _ = k.0.clone().upgrade_in_event_loop(move |app| {
                STATE.with_borrow_mut(|st| st.icons.extend(icons));
                k.rows(&app);
            });
        });
    }

    /// «Ir a Programas» desde Carpetas: abre la sección sin búsqueda y elige el programa
    /// instalado en `path` o en una carpeta que lo contiene, si lo hay.
    pub(super) fn show(&self, app: &AppWindow, path: Option<String>) {
        if app.get_locked() {
            return;
        }
        app.set_section(3);
        let data = app.global::<ProgramsData>();
        data.set_query("".into());
        self.refresh_view();
        let i = STATE.with_borrow(|st| {
            let e = &st.raws[programs::owner(&st.raws, path.as_deref()?)?];
            st.view.iter().position(|p| p.hive == e.hive && p.key == e.key)
        });
        if let Some(i) = i {
            data.set_selected(i as i32);
        }
    }

    /// Filtra y ordena según `query` y `sort`, y deja la selección vacía.
    fn refresh_view(&self) {
        let Some(app) = self.0.upgrade() else { return };
        let data = app.global::<ProgramsData>();
        let q = data.get_query();
        STATE.with_borrow_mut(|st| {
            st.view = st.programs.iter().filter(|p| programs::matches(p, &q)).cloned().collect();
            programs::sort(&mut st.view, sort_key(data.get_sort()));
        });
        data.set_selected(-1);
        self.rows(&app);
    }

    /// Rehace las filas de la vista actual (p. ej. al llegar los iconos), sin tocar la selección.
    fn rows(&self, app: &AppWindow) {
        let admin = app.get_admin();
        let rows: Vec<ProgramRow> = STATE.with_borrow(|st| st.view.iter().map(|p| row(p, &st.icons, admin)).collect());
        app.global::<ProgramsData>().set_rows(Rc::new(VecModel::from(rows)).into());
    }

    /// Pide confirmación para desinstalar la fila `i` de la vista.
    fn ask(&self, i: i32) {
        let Some(app) = self.0.upgrade() else { return };
        let Some((hive, key, name)) = STATE.with_borrow(|st| {
            let p = st.view.get(usize::try_from(i).ok()?)?;
            st.waiting.is_none().then(|| (p.hive, p.key.clone(), p.name.clone()))
        }) else {
            return;
        };
        if !programs::may_run(hive, app.get_admin()) {
            return blocked(&app);
        }
        let k = self.clone();
        let ask = Confirm {
            title: &format!("¿Desinstalar {name}?"),
            text: "Se abrirá el desinstalador del propio programa. Sigue sus pasos; cuando se cierre, WinCleaner comprobará si se ha desinstalado.",
            lines: Vec::new(),
            note: "",
            confirm_text: "Abrir desinstalador",
            danger: true,
            alt_text: "",
        };
        confirm(&app, ask, move || k.start(hive, key));
    }

    /// Ya confirmado: vuelve a leer la entrada y los permisos (pueden haber cambiado desde que
    /// se enseñó el botón), comprueba el `.exe` y lo lanza.
    pub(crate) fn start(&self, hive: Hive, key: String) {
        let Some(app) = self.0.upgrade() else { return };
        if STATE.with_borrow(|st| st.waiting.is_some()) {
            return;
        }
        if !programs::may_run(hive, app.get_admin()) {
            return blocked(&app);
        }
        // Si ya no está, otro la quitó: basta con poner la lista al día.
        let Some(raw) = win::reg::uninstall_entry(hive, key.clone()) else { return self.reload() };
        let p = programs::from_raw(&raw);
        let fail = |text: &str, folder: Option<PathBuf>| {
            let title = format!("No se pudo abrir el desinstalador de {}.", p.name);
            banner(&app, 2, &title, text);
            let data = app.global::<ProgramsData>();
            data.set_banner_path(folder.as_deref().map_or("".into(), |f| f.to_string_lossy().as_ref().into()));
            STATE.with_borrow_mut(|st| st.folder = folder);
        };
        let Some(cmd) = win::system32().and_then(|s| programs::uninstall_command(&raw, &s).ok()) else {
            return fail("Su entrada del registro no indica un desinstalador que se pueda abrir.", None);
        };
        let missing = || {
            // La carpeta más cercana que aún existe.
            let folder = Path::new(&cmd.exe).ancestors().skip(1).find(|a| a.is_dir()).map(Path::to_path_buf);
            fail(&format!("El archivo no existe: {}. Puede que se borrara a mano.", cmd.exe), folder);
        };
        if !Path::new(&cmd.exe).is_file() {
            return missing();
        }
        let launched = match uninstall::launch(&cmd) {
            Ok(l) => l,
            Err(LaunchError::NotFound) => return missing(),
            Err(LaunchError::Denied) => return fail("Windows no dejó abrirlo, o se canceló el aviso de permisos.", None),
            Err(LaunchError::Other(e)) => return fail(&format!("Windows devolvió el error {e}."), None),
        };
        let data = app.global::<ProgramsData>();
        data.set_banner(0);
        data.set_banner_title(format!("Esperando al desinstalador de {}…", p.name).into());
        data.set_banner_text(WAIT_TEXT.into());
        data.set_selected(-1);
        data.set_phase(1);
        let k = self.clone();
        STATE.with_borrow_mut(|st| {
            st.waiting = Some(Waiting { launched, hive, key, name: p.name, size: p.size, user_done: false });
            st.timer.start(TimerMode::Repeated, Duration::from_secs(1), move || k.tick());
        });
    }

    /// Mira si la espera ha acabado y, si es así, lo dice y recarga la lista y el resumen.
    fn tick(&self) {
        let Some(app) = self.0.upgrade() else { return };
        let done = STATE.with_borrow_mut(|st| {
            let w = st.waiting.as_ref()?;
            let present = win::reg::key_exists(w.hive, &format!("{}\\{}", win::reg::UNINSTALL, w.key));
            let state = programs::wait_state(present, w.user_done);
            if state == WaitState::Waiting {
                // El seguimiento del proceso solo cambia la ayuda: no cierra la espera.
                let text = if uninstall::alive(&w.launched) { WAIT_TEXT } else { WAIT_GONE_TEXT };
                app.global::<ProgramsData>().set_banner_text(text.into());
                return None;
            }
            st.timer.stop();
            // Al soltarlo se cierran sus handles; el desinstalador, si sigue, no se toca.
            st.waiting.take().map(|w| (state, w))
        });
        let Some((state, w)) = done else { return };
        app.global::<ProgramsData>().set_phase(0);
        if state == WaitState::Removed {
            let text = w.size.map_or(String::new(), |s| format!("Has recuperado {}.", format::bytes(s)));
            banner(&app, 1, &format!("{} se ha desinstalado.", w.name), &text);
        } else {
            banner(&app, 3, &format!("{} sigue instalado.", w.name), "Puede que cancelaras el desinstalador.");
        }
        self.reload();
        app.global::<SummaryData>().invoke_refresh();
    }
}

const WAIT_TEXT: &str = "Termina en su ventana. La lista se actualizará cuando se cierre.";
/// Ya no queda ningún proceso seguido, pero el desinstalador puede seguir (si se elevó solo).
const WAIT_GONE_TEXT: &str = "Si ya has cerrado el desinstalador, pulsa «Ya he terminado».";

fn banner(app: &AppWindow, kind: i32, title: &str, text: &str) {
    let data = app.global::<ProgramsData>();
    data.set_banner_title(title.into());
    data.set_banner_text(text.into());
    data.set_banner_path("".into());
    data.set_banner(kind);
}

/// Aviso 4: una entrada de HKCU no se lanza siendo administrador.
fn blocked(app: &AppWindow) {
    banner(
        app,
        4,
        "Este programa se instaló solo para tu usuario.",
        "Reinicia WinCleaner sin permisos de administrador para desinstalarlo.",
    );
}
