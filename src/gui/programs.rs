//! Sección Programas: la lista de las claves `Uninstall`, con búsqueda, orden e iconos.
//! El registro y los iconos se leen en hilos; la interfaz no espera a ninguno.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use slint::{Color, ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, VecModel, Weak};
use wincleaner::format;
use wincleaner::programs::{self, Hive, Program, RawEntry, SortKey};
use wincleaner::ui::{AppWindow, ProgramRow, ProgramsData};

use super::summary;
use crate::win;

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
}

thread_local! {
    // Solo en el hilo de la interfaz: los hilos de trabajo vuelven con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

/// Los seis colores de las baldosas de las maquetas.
const TINTS: [u32; 6] = [0x8fb0c4, 0xd68a8a, 0x8fc2c0, 0xc4a7b4, 0xa9c2b8, 0xd6b38a];

/// Color estable para un nombre: no cambia entre ejecuciones ni versiones de Rust.
fn tint(name: &str) -> Color {
    let h = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let c = TINTS[h as usize % TINTS.len()];
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn letter(name: &str) -> String {
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
        app.global::<ProgramsData>().on_view_changed(move || k.refresh_view());
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
}
