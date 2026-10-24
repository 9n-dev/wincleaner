//! Sección Procesos. «En ejecución»: los procesos agrupados por programa, con firma, CPU y
//! memoria, refrescados cada 2 s solo mientras se ve la sección. La lista, las firmas, las
//! descripciones y los iconos se leen en un hilo; terminar se confirma siempre con el modal.
//! «Arrancan con Windows»: se lee al abrir WinCleaner y al entrar en la sección; el
//! interruptor escribe en `StartupApproved` sin confirmación, porque se puede deshacer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use slint::{Color, ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel, Weak};
use wincleaner::format;
use wincleaner::paths;
use wincleaner::procs::{self, Group};
use wincleaner::startup;
use wincleaner::ui::{AppWindow, Confirm, ProcRow, ProcsData, StartRow, confirm};

use super::{programs, summary};
use crate::win;
use crate::win::proc::TermError;
use crate::win::startup::Item;
use crate::win::trust::{self, Signature};

type Icon = Option<SharedPixelBuffer<Rgba8Pixel>>;

/// Lo que se enseña de un grupo y no cambia mientras corre: nombre, subtítulo e icono.
#[derive(Clone)]
struct Meta {
    name: String,
    sub: String,
    icon: Icon,
}

/// Una vuelta del hilo de trabajo. Sin `full` (la del Resumen), `groups` y `metas` van vacíos.
struct Snap {
    full: bool,
    groups: Vec<Group>,
    metas: Vec<Meta>,
    /// Tiempo de CPU de cada PID, para la siguiente vuelta.
    cpu: HashMap<u32, u64>,
    /// `win::proc::system()`.
    sys: (u64, u64, u64, u64),
    count: usize,
}

#[derive(Default)]
struct State {
    /// Los grupos que se ven; `selected`, `open(i)` y `terminate(i)` son índices de aquí.
    groups: Vec<Group>,
    /// El nombre que se enseña de cada grupo.
    names: Vec<String>,
    /// Tiempos de la vuelta anterior: CPU total, inactivo y de cada PID.
    prev: Option<(u64, u64, HashMap<u32, u64>)>,
    /// Hay una vuelta en marcha: el temporizador no pide otra.
    busy: bool,
    /// El único modelo de `ProcsData.rows`: se cambia fila a fila para que Slint no rehaga la
    /// lista (se perderían clics, foco y el anuncio del lector de pantalla).
    model: Rc<VecModel<ProcRow>>,
    timer: Timer,
    /// Lo que arranca con Windows; `toggle(i)` es un índice de aquí y de `starts_model`.
    starts: Vec<Item>,
    starts_model: Rc<VecModel<StartRow>>,
}

thread_local! {
    // Solo en el hilo de la interfaz: los hilos de trabajo vuelven con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

// ponytail: no se vacía; crece con los ejecutables distintos que se ven (pocos cientos).
static METAS: Mutex<Option<HashMap<String, Meta>>> = Mutex::new(None);

/// Gris de las maquetas para lo de Windows.
const GREY: Color = Color::from_rgb_u8(0x9b, 0xa6, 0xad);

fn sub(sig: &Signature, exe: &str, temp: Option<&str>) -> String {
    let base = match sig {
        Signature::Signed(s) if s.is_empty() => return "Firmado".into(),
        Signature::Signed(s) => return s.clone(),
        // No se pudo comprobar: no se dice «Sin firma»; se enseña el editor, si lo pone.
        Signature::Unknown => return win::proc::company(exe).unwrap_or_default(),
        Signature::Unsigned => "Sin firma",
        Signature::Invalid => "Firma no válida",
    };
    if temp.is_some_and(|t| paths::is_inside(exe, t)) {
        format!("{base} · se ejecuta desde Temp")
    } else {
        base.into()
    }
}

/// Nombre, firma e icono del grupo, con caché por ruta. Va en el hilo de trabajo (con COM).
fn meta(g: &Group, temp: Option<&str>) -> Meta {
    let Some(exe) = g.exe.as_deref().filter(|e| !e.is_empty()) else {
        return Meta { name: g.name.clone(), sub: String::new(), icon: None };
    };
    if let Some(m) = METAS.lock().ok().and_then(|c| c.as_ref()?.get(&g.key).cloned()) {
        return m;
    }
    let m = Meta {
        name: win::proc::description(exe).unwrap_or_else(|| g.name.clone()),
        sub: sub(&trust::check_cached(exe), exe, temp),
        icon: win::icon::load(exe, 0),
    };
    if let Ok(mut c) = METAS.lock() {
        c.get_or_insert_with(HashMap::new).insert(g.key.clone(), m.clone());
    }
    m
}

fn letter(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map_or("?".into(), String::from)
}

fn pct(p: f32) -> String {
    format!("{} %", p.round() as u32)
}

/// Pone `rows` en `model` sin cambiar de modelo: las filas comunes se cambian solo si son
/// distintas, y la cola se añade o se quita.
fn update<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    let old = model.row_count();
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
    for (i, r) in rows.into_iter().enumerate() {
        if i >= old {
            model.push(r);
        } else if model.row_data(i).as_ref() != Some(&r) {
            model.set_row_data(i, r);
        }
    }
}

#[derive(Clone)]
pub struct Procs {
    app: Weak<AppWindow>,
    system_root: String,
    temp: Option<String>,
}

impl Procs {
    pub fn new(app: &AppWindow) -> Procs {
        let temp = std::env::var_os("TEMP").map(|t| win::long_path(Path::new(&t)).to_string_lossy().into_owned());
        // Sin la carpeta de Windows, todo queda bloqueado (`procs::blocked` falla cerrado).
        let c = Procs { app: app.as_weak(), system_root: win::windows_dir().unwrap_or_default(), temp };
        let data = app.global::<ProcsData>();
        data.set_rows(STATE.with_borrow(|st| st.model.clone()).into());
        data.set_starts(STATE.with_borrow(|st| st.starts_model.clone()).into());
        let k = c.clone();
        data.on_toggle(move |i| k.toggle(i));
        let k = c.clone();
        data.on_visible(move |v| k.visible(v));
        let k = c.clone();
        data.on_terminate(move |i| k.ask(i));
        data.on_open(|i| {
            STATE.with_borrow(|st| {
                let exe = usize::try_from(i).ok().and_then(|i| st.groups.get(i)?.exe.clone());
                if let Some(exe) = exe.filter(|e| Path::new(e).is_absolute()) {
                    win::open_in_explorer(Path::new(&exe));
                }
            });
        });
        let weak = app.as_weak();
        data.on_dismiss(move || {
            if let Some(app) = weak.upgrade() {
                let data = app.global::<ProcsData>();
                data.set_denied(false);
                data.set_start_error("".into());
            }
        });
        // Una vuelta para la tarjeta del Resumen.
        c.refresh(false);
        c.load_starts();
        c
    }

    fn visible(&self, v: bool) {
        if v {
            let k = self.clone();
            STATE.with_borrow_mut(|st| {
                // Al volver, la CPU de la primera vuelta es «—»: no se promedia el rato fuera.
                st.prev = None;
                st.timer.start(TimerMode::Repeated, Duration::from_secs(2), move || k.refresh(true));
            });
            self.refresh(true);
            self.load_starts();
        } else {
            STATE.with_borrow(|st| st.timer.stop());
            // Al volver al Resumen, su tarjeta se pone al día.
            if self.app.upgrade().is_some_and(|a| a.get_section() == 0) {
                self.refresh(false);
            }
        }
    }

    /// Pide una vuelta al hilo, salvo si la anterior aún no ha vuelto. Sin `full`, solo el
    /// recuento y la memoria (para el Resumen): ni firmas, ni descripciones, ni iconos.
    fn refresh(&self, full: bool) {
        if STATE.with_borrow_mut(|st| std::mem::replace(&mut st.busy, true)) {
            return;
        }
        let k = self.clone();
        std::thread::spawn(move || {
            let v = win::proc::snapshot();
            let sys = win::proc::system();
            let count = v.len();
            let cpu = v.iter().map(|p| (p.pid, p.cpu_100ns)).collect();
            let (mut groups, mut metas) = (Vec::new(), Vec::new());
            if full {
                groups = procs::group(v, &k.system_root);
                // WinCleaner no se ofrece a terminarse a sí mismo.
                let me = win::proc::own_pid();
                for g in groups.iter_mut().filter(|g| g.pids.contains(&me)) {
                    g.blocked = true;
                }
                // SHGetFileInfoW (el icono de reserva) necesita COM en este hilo.
                metas = win::with_com(|| groups.iter().map(|g| meta(g, k.temp.as_deref())).collect());
            }
            let snap = Snap { full, groups, metas, cpu, sys, count };
            let _ = k.app.clone().upgrade_in_event_loop(move |app| k.loaded(&app, snap));
        });
    }

    fn loaded(&self, app: &AppWindow, s: Snap) {
        let data = app.global::<ProcsData>();
        let (total, idle, used, all) = s.sys;
        let n = s.count;
        let procesos = if n == 1 { "1 proceso".to_string() } else { format!("{n} procesos") };
        summary::set_procs(app, &format!("{procesos} · {}", format::bytes(used)));
        if !s.full {
            STATE.with_borrow_mut(|st| st.busy = false);
            // Si mientras tanto se entró en Procesos, su vuelta se saltó: se pide ahora.
            if app.get_section() == 4 {
                self.refresh(true);
            }
            return;
        }
        STATE.with_borrow_mut(|st| {
            st.busy = false;
            // Se mantiene la fila elegida aunque cambie de sitio.
            let key = usize::try_from(data.get_selected()).ok().and_then(|i| st.groups.get(i)).map(|g| g.key.clone());
            let selected = key.and_then(|k| s.groups.iter().position(|g| g.key == k));
            let prev = st.prev.take();
            let elapsed = prev.as_ref().map(|p| total.saturating_sub(p.0)).filter(|&d| d > 0);
            let cpu_total = match (&prev, elapsed) {
                (Some(p), Some(d)) => pct(100.0 - procs::cpu_percent(p.1, idle, d, 1)),
                _ => "—".into(),
            };
            let rows: Vec<ProcRow> = s
                .groups
                .iter()
                .zip(&s.metas)
                .map(|(g, m)| {
                    let cpu = match (&prev, elapsed) {
                        (Some(p), Some(d)) => {
                            let now: u64 = g.pids.iter().filter_map(|pid| s.cpu.get(pid)).sum();
                            // Un PID nuevo empieza a contar desde ahora.
                            let before: u64 =
                                g.pids.iter().filter_map(|pid| p.2.get(pid).or_else(|| s.cpu.get(pid))).sum();
                            pct(procs::cpu_percent(before, now, d, 1))
                        }
                        _ => "—".into(),
                    };
                    ProcRow {
                        name: m.name.as_str().into(),
                        count: g.pids.len() as i32,
                        sub: m.sub.as_str().into(),
                        cpu: cpu.into(),
                        mem: format::bytes(g.mem).into(),
                        icon: m.icon.clone().map(Image::from_rgba8).unwrap_or_default(),
                        letter: letter(&m.name).into(),
                        tint: if g.blocked { GREY } else { programs::tint(&m.name) },
                        blocked: g.blocked,
                        // La regla llega con las comprobaciones de seguridad.
                        review: false,
                    }
                })
                .collect();
            data.set_subtitle(
                format!("{procesos} · CPU {cpu_total} · Memoria {} de {}", format::bytes(used), format::bytes(all)).into(),
            );
            update(&st.model, rows);
            data.set_selected(selected.map_or(-1, |i| i as i32));
            st.prev = Some((total, idle, s.cpu));
            st.groups = s.groups;
            st.names = s.metas.into_iter().map(|m| m.name).collect();
        });
    }

    /// Pide confirmación para terminar el grupo `i`; los de Windows no tienen botón ni pasan de aquí.
    fn ask(&self, i: i32) {
        let Some(app) = self.app.upgrade() else { return };
        let Some((key, n, name)) = STATE.with_borrow(|st| {
            let i = usize::try_from(i).ok()?;
            let g = st.groups.get(i)?;
            if g.blocked || g.exe.is_none() {
                return None;
            }
            Some((g.key.clone(), g.pids.len(), st.names.get(i)?.clone()))
        }) else {
            return;
        };
        let text = if n == 1 {
            format!("Se cerrará. Lo que no hayas guardado en {name} se perderá.")
        } else {
            format!("Se cerrarán sus {n} procesos. Lo que no hayas guardado en {name} se perderá.")
        };
        let k = self.clone();
        let ask = Confirm {
            title: &format!("¿Terminar {name}?"),
            text: &text,
            lines: Vec::new(),
            note: "",
            confirm_text: "Terminar",
            danger: true,
        };
        confirm(&app, ask, move || k.kill(&key, name));
    }

    /// Ya confirmado: vuelve a buscar el grupo en la última vuelta (con sus PID de ahora) y
    /// comprueba otra vez que no es de Windows. `terminate` mira cada PID justo antes.
    fn kill(&self, key: &str, name: String) {
        let found = STATE.with_borrow(|st| {
            let g = st.groups.iter().find(|g| g.key == key)?;
            let exe = g.exe.clone().filter(|e| !g.blocked && !procs::blocked(Some(e), false, &self.system_root))?;
            Some((g.pids.clone(), exe))
        });
        // Ya no está (o se ha vuelto intocable): basta con refrescar.
        let Some((pids, exe)) = found else { return self.refresh(true) };
        let k = self.clone();
        std::thread::spawn(move || {
            let r = win::proc::terminate(&pids, &exe);
            let _ = k.app.clone().upgrade_in_event_loop(move |app| {
                let data = app.global::<ProcsData>();
                data.set_selected(-1);
                match r {
                    Ok(()) => data.set_denied(false),
                    Err(e) => {
                        let text = match e {
                            TermError::Denied if !app.get_admin() => String::new(),
                            TermError::Denied => "Windows ha denegado el acceso. Puede que sea un proceso protegido.".into(),
                            TermError::Other(c) => format!("Windows devolvió el error {c}."),
                        };
                        data.set_denied_name(name.into());
                        data.set_error_text(text.into());
                        data.set_denied(true);
                    }
                }
                k.refresh(true);
            });
        });
    }

    /// Lee en un hilo lo que arranca con Windows, con los iconos.
    fn load_starts(&self) {
        let k = self.clone();
        std::thread::spawn(move || {
            // Los accesos directos y SHGetFileInfoW (el icono de reserva) necesitan COM.
            let (items, icons): (Vec<Item>, Vec<Icon>) = win::with_com(|| {
                let items = win::startup::list();
                let icons = items.iter().map(|it| it.exe.as_deref().and_then(|e| win::icon::load(e, 0))).collect();
                (items, icons)
            });
            let _ = k.app.clone().upgrade_in_event_loop(move |app| {
                let admin = app.get_admin();
                let rows = items
                    .iter()
                    .zip(icons)
                    .map(|(it, icon)| StartRow {
                        name: it.name.as_str().into(),
                        r#where: startup::where_text(it.source).into(),
                        on: it.enabled,
                        needs_admin: startup::needs_admin(it.source) && !admin,
                        // La regla llega con las comprobaciones de seguridad.
                        review: false,
                        icon: icon.map(Image::from_rgba8).unwrap_or_default(),
                        letter: letter(&it.name).into(),
                        tint: programs::tint(&it.name),
                    })
                    .collect();
                set_starts_summary(&app, &items);
                STATE.with_borrow_mut(|st| {
                    update(&st.starts_model, rows);
                    st.starts = items;
                });
            });
        });
    }

    /// El interruptor `i` ya ha cambiado `on` en su fila: se escribe eso. Si falla, se avisa
    /// y se vuelve a leer la lista, que devuelve el interruptor a como está de verdad.
    fn toggle(&self, i: i32) {
        let Some(app) = self.app.upgrade() else { return };
        let Some((i, item, on)) = STATE.with_borrow(|st| {
            let i = usize::try_from(i).ok()?;
            Some((i, st.starts.get(i)?.clone(), st.starts_model.row_data(i)?.on))
        }) else {
            return;
        };
        let data = app.global::<ProcsData>();
        // `set_enabled` vuelve a mirar si hace falta ser administrador.
        match win::startup::set_enabled(&item, on) {
            Ok(()) => {
                data.set_start_error("".into());
                STATE.with_borrow_mut(|st| {
                    if let Some(it) = st.starts.get_mut(i) {
                        it.enabled = on;
                    }
                    set_starts_summary(&app, &st.starts);
                });
            }
            Err(_) => {
                data.set_start_error(item.name.as_str().into());
                self.load_starts();
            }
        }
    }
}

/// Segunda línea de la tarjeta del Resumen: los que están activados.
fn set_starts_summary(app: &AppWindow, items: &[Item]) {
    let n = items.iter().filter(|it| it.enabled).count();
    let text = if n == 1 { "1 programa arranca con Windows".to_string() } else { format!("{n} programas arrancan con Windows") };
    summary::set_procs_text(app, &text);
}
