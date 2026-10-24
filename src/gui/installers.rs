//! Pestaña Instaladores viejos de Espacio: los instaladores de Descargas y Escritorio, leídos en
//! un hilo al entrar, con los de más de 30 días marcados, y su envío a la papelera.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use slint::{ComponentHandle, Model, VecModel, Weak};
use wincleaner::format;
use wincleaner::programs::visible;
use wincleaner::space::installers::{self, Installer, is_old};
use wincleaner::ui::{AppWindow, Confirm, InstallerRow, InstallersData, ModalLine, confirm};

use super::space::{Retry, Space};
use crate::win;

struct Item {
    inst: Installer,
    checked: bool,
    /// No pasa `zones.check`: nunca se marca.
    protected: bool,
}

#[derive(Default)]
struct State {
    /// Las filas de `InstallersData.rows`, en el mismo orden.
    items: Vec<Item>,
    /// Hay una lectura en marcha: no se empieza otra.
    loading: bool,
    /// Se pidió otra lectura mientras había una en marcha (p. ej. tras enviar): se repite al acabar.
    again: bool,
}

thread_local! {
    // Solo en el hilo de la interfaz: el hilo de trabajo vuelve con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn plural(n: usize) -> String {
    if n == 1 { "1 instalador".into() } else { format!("{n} instaladores") }
}

/// «Descargas · hace 7 meses · ya instalado»
fn row(it: &Item, now: SystemTime) -> InstallerRow {
    let i = &it.inst;
    let ago = format::ago(now.duration_since(i.when).unwrap_or_default());
    let done = if i.installed { " · ya instalado" } else { "" };
    InstallerRow {
        name: file_name(&i.path).into(),
        sub: format!("{} · {ago}{done}", i.place).into(),
        size: format::bytes(i.size).into(),
        checked: it.checked,
        protected: it.protected,
    }
}

/// Los marcados que se pueden enviar.
fn picked(items: &[Item]) -> Vec<&Installer> {
    items.iter().filter(|i| i.checked && !i.protected).map(|i| &i.inst).collect()
}

/// Botón de enviar: texto y si se puede pulsar.
fn send_text(items: &[Item]) -> (String, bool) {
    let p = picked(items);
    match p.len() {
        0 => ("Enviar a la papelera".into(), false),
        n => (format!("Enviar {n} a la papelera · {}", format::bytes(p.iter().map(|i| i.size).sum())), true),
    }
}

fn set_send(app: &AppWindow, (text, can): (String, bool)) {
    let data = app.global::<InstallersData>();
    data.set_send_text(text.into());
    data.set_can_send(can);
}

#[derive(Clone)]
pub struct Installers {
    app: Weak<AppWindow>,
    space: Space,
}

impl Installers {
    pub fn new(app: &AppWindow, space: Space) -> Installers {
        let k = Installers { app: app.as_weak(), space };
        let data = app.global::<InstallersData>();
        let t = k.clone();
        data.on_toggle(move |i| t.toggle(i));
        let t = k.clone();
        data.on_send(move || t.ask());
        k
    }

    /// Lee la lista en un hilo, al entrar en la pestaña y después de enviar. La lista de antes se
    /// queda hasta que llega la nueva.
    pub(super) fn load(&self) {
        if STATE.with_borrow_mut(|st| {
            st.again = st.loading;
            std::mem::replace(&mut st.loading, true)
        }) {
            return;
        }
        let zones = self.space.zones();
        let k = self.clone();
        std::thread::spawn(move || {
            let dirs: Vec<_> = win::known_dirs().into_iter().filter(|d| matches!(d.1, "Descargas" | "Escritorio")).collect();
            let programs: Vec<String> = win::reg::uninstall_entries().into_iter().filter(visible).filter_map(|e| e.display_name).collect();
            let me = std::env::current_exe().ok();
            let now = SystemTime::now();
            let items: Vec<Item> = installers::list(&dirs, &programs, &|p| p.to_str().and_then(win::proc::product_name))
                .into_iter()
                .filter(|i| me.as_ref().is_none_or(|me| !wincleaner::paths::same(&i.path.to_string_lossy(), &me.to_string_lossy())))
                .map(|inst| {
                    // Sin zonas no se sabe qué es del sistema: nada se marca.
                    let protected = zones.as_ref().is_none_or(|z| inst.path.to_str().is_none_or(|s| z.check(s).is_some()));
                    Item { checked: !protected && is_old(inst.when, now), protected, inst }
                })
                .collect();
            let weak = k.app.clone();
            let _ = weak.upgrade_in_event_loop(move |app| k.loaded(&app, items));
        });
    }

    fn loaded(&self, app: &AppWindow, items: Vec<Item>) {
        let now = SystemTime::now();
        let rows: Vec<InstallerRow> = items.iter().map(|i| row(i, now)).collect();
        let send = send_text(&items);
        let again = STATE.with_borrow_mut(|st| {
            (st.items, st.loading) = (items, false);
            std::mem::take(&mut st.again)
        });
        let data = app.global::<InstallersData>();
        data.set_rows(Rc::new(VecModel::from(rows)).into());
        set_send(app, send);
        data.set_phase(1);
        if again {
            self.load();
        }
    }

    /// Casilla de la fila `i`. La fila se rehace siempre: así vuelve a su sitio si no se podía cambiar.
    fn toggle(&self, i: i32) {
        let Some(app) = self.app.upgrade() else { return };
        let locked = app.get_locked();
        let Some((r, send)) = STATE.with_borrow_mut(|st| {
            let it = st.items.get_mut(usize::try_from(i).ok()?)?;
            if !locked && !it.protected {
                it.checked = !it.checked;
            }
            let r = row(it, SystemTime::now());
            Some((r, send_text(&st.items)))
        }) else {
            return;
        };
        app.global::<InstallersData>().get_rows().set_row_data(i as usize, r);
        set_send(&app, send);
    }

    /// «Enviar … a la papelera»: pregunta con una línea por instalador (como mucho 5).
    fn ask(&self) {
        let Some(app) = self.app.upgrade().filter(|a| !a.get_locked()) else { return };
        let Some(zones) = self.space.zones() else { return };
        // Se vuelven a mirar las zonas al pulsar.
        let items: Vec<(PathBuf, u64)> = STATE.with_borrow(|st| {
            picked(&st.items)
                .into_iter()
                .filter(|i| i.path.to_str().is_some_and(|s| zones.check(s).is_none()))
                .map(|i| (i.path.clone(), i.size))
                .collect()
        });
        if items.is_empty() {
            return;
        }
        let mut lines: Vec<ModalLine> =
            items.iter().take(5).map(|(p, s)| ModalLine { label: file_name(p).into(), value: format::bytes(*s).into() }).collect();
        if items.len() > 5 {
            lines.push(ModalLine { label: format!("y {} más", items.len() - 5).into(), value: "".into() });
        }
        let ask = Confirm {
            title: &format!("¿Enviar {} a la papelera?", plural(items.len())),
            text: "",
            lines,
            note: "Podrás recuperarlos desde la papelera mientras no la vacíes. El espacio no se libera hasta entonces.",
            confirm_text: "Enviar a la papelera",
            danger: false,
            alt_text: "",
        };
        let k = self.clone();
        confirm(&app, ask, move || k.send(items));
    }

    /// Envía `items` y, al volver, los quita de Carpetas y vuelve a leer la lista.
    fn send(&self, items: Vec<(PathBuf, u64)>) {
        let paths: Vec<PathBuf> = items.iter().map(|i| i.0.clone()).collect();
        let names: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
        let size = items.iter().map(|i| i.1).sum();
        let k = self.clone();
        let retry: Retry = Rc::new(move |_, _| k.ask());
        let k = self.clone();
        let after = move |_: &AppWindow, rep: &win::recycle::Report| {
            let failed: HashSet<&PathBuf> = rep.failed.iter().map(|f| &f.0).collect();
            let gone: Vec<(PathBuf, u64)> = items.into_iter().filter(|i| !failed.contains(&i.0)).collect();
            k.space.forget(&gone);
            k.load();
        };
        self.space.send_to_bin(paths, names, size, retry, None, after);
    }
}
