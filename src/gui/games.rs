//! Pestaña Juegos de Espacio: los juegos de los lanzadores, leídos en un hilo, y su
//! desinstalación con el propio lanzador (o con Programas, si es una entrada `Uninstall`).

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Model, VecModel, Weak};
use wincleaner::format;
use wincleaner::space::games::{Game, Launcher, Uninstall};
use wincleaner::space::tree::Tree;
use wincleaner::ui::{AppWindow, Confirm, GameRow, GamesData, confirm};

use super::programs::{self, Programs};
use super::space::Space;
use crate::win;

#[derive(Default)]
struct State {
    /// `None` hasta la primera lectura; las filas de `GamesData.rows` van en el mismo orden.
    games: Option<Vec<Game>>,
    /// Hay una lectura en marcha (con sus tamaños): no se empieza otra.
    loading: bool,
}

thread_local! {
    // Solo en el hilo de la interfaz: el hilo de trabajo vuelve con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

/// Lo que ocupan los juegos, para el Resumen; `None` hasta la primera lectura.
pub(super) fn total() -> Option<u64> {
    STATE.with_borrow(|st| st.games.as_deref().map(sum))
}

fn sum(games: &[Game]) -> u64 {
    games.iter().filter_map(|g| g.size).sum()
}

/// «Steam · jugado hace 3 días», «Steam · nunca jugado» o solo el lanzador.
fn sub(g: &Game) -> String {
    let name = g.launcher.name();
    match (g.launcher, g.last_played) {
        (_, Some(t)) => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            format!("{name} · jugado {}", format::ago(Duration::from_secs(now.saturating_sub(t))))
        }
        (Launcher::Steam, None) => format!("{name} · nunca jugado"),
        _ => name.into(),
    }
}

fn row(g: &Game) -> GameRow {
    GameRow {
        name: g.name.as_str().into(),
        sub: sub(g).into(),
        size: g.size.map_or("—".into(), format::bytes).into(),
        letter: programs::letter(&g.name).into(),
        tint: programs::tint(&g.name),
    }
}

/// «6 juegos instalados · 382 GB en total» o «Ningún juego encontrado».
fn subtitle(games: &[Game]) -> String {
    match games.len() {
        0 => "Ningún juego encontrado".into(),
        1 => format!("1 juego instalado · {} en total", format::bytes(sum(games))),
        n => format!("{n} juegos instalados · {} en total", format::bytes(sum(games))),
    }
}

/// (texto, botón) de la confirmación: en Xbox se desinstala desde Configuración.
fn ask_text(g: &Game) -> (String, String) {
    let (who, button) = match g.launcher {
        Launcher::Xbox => ("Configuración › Aplicaciones", "Abrir Configuración".to_string()),
        l => (l.name(), format!("Abrir {}", l.name())),
    };
    let free = g.size.map_or(String::new(), |s| format!(" Liberarás unos {}.", format::bytes(s)));
    (format!("Se abrirá {who} para desinstalarlo: así el lanzador sabe que ya no está.{free}"), button)
}

#[derive(Clone)]
pub struct Games {
    app: Weak<AppWindow>,
    programs: Programs,
    space: Space,
}

impl Games {
    pub fn new(app: &AppWindow, programs: Programs, space: Space) -> Games {
        let g = Games { app: app.as_weak(), programs, space };
        let k = g.clone();
        app.global::<GamesData>().on_uninstall(move |i| k.ask(i));
        // También al abrir: así Carpetas sabe desde el principio qué carpetas son de juegos.
        g.load();
        g
    }

    /// Lee la lista en un hilo y, después, en el mismo hilo, mide los juegos que no dicen su tamaño.
    /// La lista de antes se queda hasta que llega la nueva. También al entrar en la pestaña.
    pub(super) fn load(&self) {
        if STATE.with_borrow_mut(|st| std::mem::replace(&mut st.loading, true)) {
            return;
        }
        let k = self.clone();
        std::thread::spawn(move || {
            let games = win::games::list();
            let todo: Vec<(usize, String)> =
                games.iter().enumerate().filter(|(_, g)| g.size.is_none()).filter_map(|(i, g)| Some((i, g.dir.clone()?))).collect();
            let k2 = k.clone();
            let _ = k.app.upgrade_in_event_loop(move |app| k2.loaded(&app, games));
            for (i, dir) in todo {
                let size = Tree::scan(Path::new(&dir), &AtomicBool::new(false), &mut |_, _, _| {}).map(|t| t.node(t.root()).size);
                if let Some(size) = size {
                    let _ = k.app.upgrade_in_event_loop(move |app| Self::sized(&app, i, size));
                }
            }
            let _ = k.app.upgrade_in_event_loop(|_| STATE.with_borrow_mut(|st| st.loading = false));
        });
    }

    fn loaded(&self, app: &AppWindow, games: Vec<Game>) {
        let dirs: Vec<String> = games.iter().filter_map(|g| g.dir.clone()).collect();
        self.space.set_games(&dirs);
        let data = app.global::<GamesData>();
        data.set_rows(Rc::new(VecModel::from(games.iter().map(row).collect::<Vec<_>>())).into());
        data.set_subtitle(subtitle(&games).into());
        data.set_selected(-1);
        data.set_phase(1);
        STATE.with_borrow_mut(|st| st.games = Some(games));
        super::summary::refresh_space(app);
    }

    /// El juego `i` ya tiene tamaño: se rehace su fila (sin tocar la selección) y el total.
    fn sized(app: &AppWindow, i: usize, size: u64) {
        let Some((r, sub)) = STATE.with_borrow_mut(|st| {
            let games = st.games.as_mut()?;
            games.get_mut(i)?.size = Some(size);
            Some((row(&games[i]), subtitle(games)))
        }) else {
            return;
        };
        let data = app.global::<GamesData>();
        data.get_rows().set_row_data(i, r);
        data.set_subtitle(sub.into());
        super::summary::refresh_space(app);
    }

    /// «Desinstalar» en la fila `i`: pregunta y abre el lanzador, Configuración o Programas.
    fn ask(&self, i: i32) {
        let Some(app) = self.app.upgrade() else { return };
        let Some(g) = STATE.with_borrow(|st| st.games.as_ref()?.get(usize::try_from(i).ok()?).cloned()) else { return };
        let (text, button) = ask_text(&g);
        let ask = Confirm {
            title: &format!("¿Desinstalar {}?", g.name),
            text: &text,
            lines: Vec::new(),
            note: "",
            confirm_text: &button,
            danger: true,
            alt_text: "",
        };
        let k = self.clone();
        confirm(&app, ask, move || k.uninstall(g.uninstall));
    }

    fn uninstall(&self, u: Uninstall) {
        match u {
            Uninstall::Url(url) => win::open_url(&url),
            Uninstall::Program { hive, key } => {
                // Mientras se borra no se cambia de sección.
                let Some(app) = self.app.upgrade().filter(|a| !a.get_locked()) else { return };
                app.set_section(3);
                self.programs.start(hive, key);
            }
        }
    }
}

