//! Sección Seguridad: análisis de Microsoft Defender (rápido o completo, y se puede cancelar) y
//! las seis comprobaciones de WinCleaner, que solo leen. WMI, servicios, tareas, firmas y
//! cancelar van en hilos; un temporizador de 1 s pone al día el tiempo y mira si Defender ha
//! terminado. Solo hay un análisis a la vez, y sigue aunque se cambie de sección.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use wincleaner::checks::{self, Action, Finding, Level, Places};
use wincleaner::defender::{self, Availability, Detection, ScanExit, ScanKind};
use wincleaner::format;
use wincleaner::procs::{Group, TermError};
use wincleaner::programs::Hive;
use wincleaner::startup;
use wincleaner::ui::{AppWindow, CheckRow, FindingRow, Nav, SecPhase, SecurityData};

use super::{procs, summary};
use crate::win;
use crate::win::defender::Scan;
use crate::win::startup::Item;
use crate::win::trust::{self, Signature};

/// En el orden en que se hacen.
const CHECKS: [&str; 6] = [
    "Programas que arrancan con Windows",
    "Procesos en ejecución",
    "Servicios que no son de Microsoft",
    "Archivo hosts",
    "Proxy del sistema",
    "Copias falsas de procesos de Windows",
];

const SECURITY_URL: &str = "windowsdefender://threat";
const INTERNET: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// Lo que devuelve el hilo de las comprobaciones.
struct Own {
    findings: Vec<Finding>,
    /// Lo que arranca con Windows: `DisableStartup(i)` es un índice de aquí.
    starts: Vec<Item>,
    /// Los procesos de ese momento, para el texto de «¿Terminar…?».
    groups: Vec<Group>,
    /// Las comprobaciones que no se han podido hacer, por su nombre.
    failed: Vec<&'static str>,
}

/// Lo que deja el análisis de Defender.
#[derive(Default)]
struct Def {
    threats: Vec<Finding>,
    /// No arrancó o falló: va en el aviso.
    problem: Option<String>,
    /// Pidió permisos de administrador (y WinCleaner no los tiene).
    needs_admin: bool,
}

#[derive(Default)]
struct State {
    /// Sube con cada análisis: lo que llega de uno anterior (cancelado) se tira.
    run: u32,
    /// Hay un análisis o unas comprobaciones en marcha: no se lanza otro.
    busy: bool,
    /// `None`: solo las comprobaciones.
    kind: Option<ScanKind>,
    scan: Option<Scan>,
    cancelling: bool,
    started: Option<(Instant, SystemTime)>,
    def: Option<Def>,
    own: Option<Own>,
    /// Los hallazgos que se ven; `action(i, j)` es la acción `j` del hallazgo `i`.
    findings: Vec<Finding>,
    starts: Vec<Item>,
    groups: Vec<Group>,
    /// Fase y subtítulo del último resultado, para volver a ellos si se cancela.
    last: Option<(SecPhase, SharedString)>,
    /// Tarjeta del Resumen: resultado y antivirus.
    summary: (String, String),
    checks: Rc<VecModel<CheckRow>>,
    rows: Rc<VecModel<FindingRow>>,
    timer: Timer,
}

thread_local! {
    // Solo en el hilo de la interfaz: los hilos de trabajo vuelven con `upgrade_in_event_loop`.
    static STATE: RefCell<State> = RefCell::default();
}

/// Un programa que arranca con Windows y merece un vistazo (también la pastilla de Procesos).
/// Solo los activados, y la firma solo cuenta si se ha podido comprobar (R16).
pub(super) fn startup_check(i: usize, it: &Item, p: &Places) -> Option<Finding> {
    let exe = it.exe.as_deref().filter(|_| it.enabled)?;
    // Solo «no existe» de verdad: si no se puede mirar (acceso…), no hay hallazgo; y si el `Run`
    // traía un nombre suelto o sin `.exe`, Windows lo encuentra por otro lado.
    let exists = !matches!(std::fs::metadata(exe), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
    if !exists && !it.sure {
        return None;
    }
    let signed = if exists { trust::check_cached(exe).verified()? } else { true };
    checks::startup_finding(i, &it.name, Some(exe), exists, signed, p)
}

/// Pastilla «Revisar» de un proceso: lo que no depende de la firma (copia falsa de un proceso de
/// Windows) siempre; lo demás, solo con la firma comprobada (R16).
pub(super) fn process_review(exe: &str, sig: &Signature, p: &Places) -> bool {
    checks::fake_system_process(exe, &p.system_root).is_some()
        || sig.verified().is_some_and(|signed| checks::process_finding(exe, signed, p).is_some())
}

/// Las seis comprobaciones, en orden; `mark(i, estado)` va marcando su fila (3: no se pudo
/// revisar). Con COM.
fn own_checks(mark: &dyn Fn(usize, i32)) -> Own {
    let p = win::security_places();
    let mut v = Vec::new();
    let mut failed = Vec::new();
    let mut end = |i: usize, ok: bool| {
        mark(i, if ok { 2 } else { 3 });
        if !ok {
            failed.push(CHECKS[i]);
        }
    };

    mark(0, 1);
    let (starts, ok) = win::startup::list();
    v.extend(starts.iter().enumerate().filter_map(|(i, it)| startup_check(i, it, &p)));
    end(0, ok);

    mark(1, 1);
    let me = win::proc::own_pid();
    let snap = win::proc::snapshot();
    // Siempre está WinCleaner: vacía es que no se ha podido leer.
    let procs_ok = !snap.is_empty();
    let groups = wincleaner::procs::group(snap, &p.system_root);
    // WinCleaner no se señala a sí mismo (portable y sin firma, puede estar en Descargas).
    let exes: Vec<&str> = groups
        .iter()
        .filter(|g| !g.pids.contains(&me))
        .filter_map(|g| g.exe.as_deref())
        .filter(|e| !e.is_empty())
        .collect();
    for exe in &exes {
        if let Some(signed) = trust::check_cached(exe).verified() {
            v.extend(checks::process_finding(exe, signed, &p));
        }
    }
    end(1, procs_ok);

    mark(2, 1);
    let (services, tasks) = (win::services::services(), win::services::boot_tasks());
    let ok = services.is_some() && tasks.is_some();
    for (name, exe) in services.into_iter().chain(tasks).flatten() {
        // Con firma (de quien sea) no hay hallazgo; si no se sabe, tampoco (R16).
        if trust::check_cached(&exe).verified() == Some(false) {
            v.extend(checks::service_finding(&name, &exe, None));
        }
    }
    end(2, ok);

    mark(3, 1);
    // Sin archivo hosts no hay entradas añadidas; si no se puede leer, no se sabe.
    let hosts = win::system32().map(|s| std::fs::read(Path::new(&s).join(r"drivers\etc\hosts")));
    let ok = match hosts {
        Some(Ok(b)) => {
            v.extend(checks::hosts_finding(&checks::hosts_entries(&String::from_utf8_lossy(&b))));
            true
        }
        Some(Err(e)) => e.kind() == std::io::ErrorKind::NotFound,
        None => false,
    };
    end(3, ok);

    mark(4, 1);
    v.extend(checks::proxy_finding(
        win::reg::read_dword(Hive::User, INTERNET, "ProxyEnable"),
        win::reg::read_str(Hive::User, INTERNET, "ProxyServer").as_deref(),
        win::reg::read_str(Hive::User, INTERNET, "AutoConfigURL").as_deref(),
    ));
    end(4, true);

    mark(5, 1);
    for exe in &exes {
        v.extend(checks::fake_system_process(exe, &p.system_root));
    }
    end(5, procs_ok);

    // «Terminar proceso» solo si sigue en marcha y no es de Windows.
    let killable = |exe: &str| groups.iter().any(|g| !g.blocked && g.key == exe.to_ascii_lowercase());
    for f in &mut v {
        f.actions.retain(|a| !matches!(a, Action::Kill(e) if !killable(e)));
    }
    Own { findings: v, starts, groups, failed }
}

fn threat(name: &str, path: String) -> Finding {
    Finding {
        level: Level::Threat,
        title: format!("Defender ha encontrado {name}"),
        path,
        why: "Defender ya la ha puesto en cuarentena. Desde Seguridad de Windows puedes eliminarla o restaurarla.".into(),
        actions: vec![Action::OpenWindowsSecurity],
    }
}

fn label(a: &Action) -> &'static str {
    match a {
        Action::DisableStartup(_) => "Desactivar inicio",
        Action::Kill(_) => "Terminar proceso",
        Action::OpenLocation(_) => "Abrir ubicación",
        Action::Search(_) => "Buscar en internet",
        Action::OpenHosts => "Ver el archivo",
        Action::OpenWindowsSecurity => "Abrir Seguridad de Windows",
    }
}

fn row(f: &Finding) -> FindingRow {
    let actions: Vec<SharedString> = f.actions.iter().map(|a| label(a).into()).collect();
    FindingRow {
        level: match f.level {
            Level::Threat => 0,
            Level::Suspicious => 1,
            Level::Review => 2,
        },
        title: f.title.as_str().into(),
        path: f.path.as_str().into(),
        why: f.why.as_str().into(),
        actions: ModelRc::new(VecModel::from(actions)),
    }
}

/// Pone todas las comprobaciones en `state`.
fn reset_checks(st: &State, state: i32) {
    for (i, name) in CHECKS.iter().enumerate() {
        st.checks.set_row_data(i, CheckRow { name: (*name).into(), state });
    }
}

#[derive(Clone)]
pub struct Security {
    app: Weak<AppWindow>,
    system_root: String,
}

impl Security {
    pub fn new(app: &AppWindow) -> Security {
        // Sin la carpeta de Windows, todo queda bloqueado (`procs::blocked` falla cerrado).
        let k = Security { app: app.as_weak(), system_root: win::windows_dir().unwrap_or_default() };
        let data = app.global::<SecurityData>();
        STATE.with_borrow_mut(|st| {
            st.checks.set_vec(CHECKS.map(|n| CheckRow { name: n.into(), state: 0 }).to_vec());
            st.summary = ("Sin revisar todavía".into(), String::new());
            data.set_checks(st.checks.clone().into());
            data.set_findings(st.rows.clone().into());
        });
        let c = k.clone();
        data.on_quick(move || c.start(Some(ScanKind::Quick)));
        let c = k.clone();
        data.on_full(move || c.start(Some(ScanKind::Full)));
        let c = k.clone();
        data.on_checks_only(move || c.start(None));
        let c = k.clone();
        data.on_again(move || c.start(STATE.with_borrow(|st| st.kind)));
        let c = k.clone();
        data.on_cancel(move || c.cancel());
        let c = k.clone();
        data.on_action(move |i, j| c.act(i, j));
        data.on_open_security(|| win::open_url(SECURITY_URL));
        k.show_summary(app);
        // WMI puede tardar: el estado del antivirus se lee en un hilo.
        let c = k.clone();
        std::thread::spawn(move || {
            let a = defender::availability(win::defender::status(), win::defender::other_avs().as_deref());
            // Sin saber qué antivirus hay, se puede analizar igual si está MpCmdRun.
            let scan = a == Availability::Unknown && win::defender::mpcmdrun().is_some();
            let _ = c.app.clone().upgrade_in_event_loop(move |app| {
                app.global::<SecurityData>().set_scan_anyway(scan);
                c.set_av(&app, a);
            });
        });
        k
    }

    fn show_summary(&self, app: &AppWindow) {
        STATE.with_borrow(|st| summary::set_security(app, &st.summary.0, &st.summary.1));
    }

    fn set_av(&self, app: &AppWindow, a: Availability) {
        let (av, text, line) = match a {
            Availability::Defender(s) => {
                let firmas = match s.signature_age_days {
                    Some(0) => "Firmas de hoy".to_string(),
                    Some(1) => "Firmas de hace 1 día".to_string(),
                    Some(n) => format!("Firmas de hace {n} días"),
                    None => String::new(),
                };
                let line = if firmas.is_empty() {
                    "Defender activo".to_string()
                } else {
                    format!("Defender activo · {}", firmas.to_lowercase())
                };
                (0, firmas, line)
            }
            Availability::OtherAv(v) => {
                let names = v.join(", ");
                let line = format!("Usas {names}");
                (1, names, line)
            }
            Availability::NoAv => (2, String::new(), "No hay ningún antivirus activo".to_string()),
            Availability::Unknown => (3, String::new(), "No se pudo saber qué antivirus tienes".to_string()),
        };
        let data = app.global::<SecurityData>();
        data.set_av(av);
        data.set_av_text(text.into());
        STATE.with_borrow_mut(|st| st.summary.1 = line);
        self.show_summary(app);
    }

    /// Lanza el análisis (`None`: solo las comprobaciones), salvo si ya hay uno en marcha.
    fn start(&self, kind: Option<ScanKind>) {
        let Some(app) = self.app.upgrade() else { return };
        let data = app.global::<SecurityData>();
        let av = data.get_av();
        if av < 0 {
            return; // aún no se sabe si hay Defender
        }
        let kind = kind.filter(|_| av == 0 || (av == 3 && data.get_scan_anyway()));
        let started = STATE.with_borrow_mut(|st| {
            if st.busy {
                return None;
            }
            st.busy = true;
            st.run += 1;
            st.kind = kind;
            st.own = None;
            // Un segundo antes: WMI da la hora de la detección sin fracciones.
            let now = SystemTime::now();
            st.started = Some((Instant::now(), now.checked_sub(Duration::from_secs(1)).unwrap_or(now)));
            st.scan = kind.and_then(win::defender::start);
            st.def = match (kind, &st.scan) {
                (None, _) => Some(Def::default()),
                (Some(_), None) => {
                    Some(Def { problem: Some("No se ha podido lanzar el análisis de Defender.".into()), ..Def::default() })
                }
                (Some(_), Some(_)) => None,
            };
            reset_checks(st, 0);
            let k = self.clone();
            st.timer.start(TimerMode::Repeated, Duration::from_secs(1), move || k.tick());
            Some((st.run, st.scan.is_some()))
        });
        let Some((run, running)) = started else { return };
        data.set_defender_running(running);
        data.set_phase(SecPhase::Scanning);
        self.tick();
        self.run_checks(run);
    }

    /// Cada segundo: el tiempo del subtítulo y, si Defender ha terminado, sus amenazas.
    fn tick(&self) {
        let Some(app) = self.app.upgrade() else { return };
        let (header, done) = STATE.with_borrow_mut(|st| {
            let t = format::duration(st.started.map_or(Duration::ZERO, |s| s.0.elapsed()));
            let header = match (st.cancelling, st.kind) {
                (true, _) => "Cancelando…".to_string(),
                (_, Some(ScanKind::Quick)) => format!("Análisis rápido en curso · {t}"),
                (_, Some(ScanKind::Full)) => format!("Análisis completo en curso · {t}"),
                (_, None) => format!("Revisando cosas raras · {t}"),
            };
            let done = st.scan.as_mut().and_then(Scan::finished);
            // Ya ha terminado: soltarlo no hace nada.
            let done = done.inspect(|_| st.scan = None);
            (header, done.zip(st.started).map(|(d, s)| (d, s.1, st.run)))
        });
        app.global::<SecurityData>().set_header(header.into());
        let Some(((code, out), since, run)) = done else { return };
        let exit = defender::scan_exit(code, &out, false);
        let k = self.clone();
        std::thread::spawn(move || {
            let found = match exit {
                ScanExit::Clean | ScanExit::Threats => win::defender::detections_since(since),
                _ => Vec::new(),
            };
            let _ = k.app.clone().upgrade_in_event_loop(move |app| k.defender_done(&app, run, exit, found));
        });
    }

    fn defender_done(&self, app: &AppWindow, run: u32, exit: ScanExit, found: Vec<Detection>) {
        let mut def = Def { threats: found.into_iter().map(|d| threat(&d.name, d.path)).collect(), ..Def::default() };
        match exit {
            // Defender dice que hay algo, pero WMI no lo ha dado: se avisa igual.
            ScanExit::Threats if def.threats.is_empty() => def.threats.push(threat("una amenaza", String::new())),
            ScanExit::NeedsAdmin if !app.get_admin() => def.needs_admin = true,
            ScanExit::NeedsAdmin => def.problem = Some("Windows no ha dejado analizar a Defender.".into()),
            ScanExit::Failed(c) => def.problem = Some(format!("Defender no ha podido terminar el análisis (error {c}).")),
            _ => {}
        }
        if STATE.with_borrow_mut(|st| st.run == run && st.def.replace(def).is_none()) {
            self.finish(app);
        }
    }

    /// Las comprobaciones, en un hilo con COM (accesos directos, tareas programadas).
    fn run_checks(&self, run: u32) {
        let k = self.clone();
        std::thread::spawn(move || {
            let weak = k.app.clone();
            let mark = move |i: usize, state: i32| {
                let _ = weak.upgrade_in_event_loop(move |_| {
                    STATE.with_borrow(|st| {
                        if st.run == run {
                            st.checks.set_row_data(i, CheckRow { name: CHECKS[i].into(), state });
                        }
                    });
                });
            };
            let own = win::with_com(|| own_checks(&mark));
            let _ = k.app.clone().upgrade_in_event_loop(move |app| {
                if STATE.with_borrow_mut(|st| st.run == run && st.own.replace(own).is_none()) {
                    k.finish(&app);
                }
            });
        });
    }

    /// Cuando han vuelto Defender y las comprobaciones: junta, ordena y enseña los hallazgos.
    fn finish(&self, app: &AppWindow) {
        let Some((def, n, threats, bad, kind, failed)) = STATE.with_borrow_mut(|st| {
            if st.def.is_none() || st.own.is_none() {
                return None;
            }
            let (mut def, own) = (st.def.take()?, st.own.take()?);
            st.timer.stop();
            st.busy = false;
            let mut all = std::mem::take(&mut def.threats);
            all.extend(own.findings);
            let failed = own.failed;
            st.findings = checks::merge(all);
            st.starts = own.starts;
            st.groups = own.groups;
            st.rows.set_vec(st.findings.iter().map(row).collect::<Vec<_>>());
            let threats = st.findings.iter().filter(|f| f.level == Level::Threat).count();
            let bad = st.findings.iter().any(|f| f.level != Level::Review);
            Some((def, st.findings.len(), threats, bad, st.kind, failed))
        }) else {
            return;
        };
        let data = app.global::<SecurityData>();
        let (h, m) = win::local_hm();
        let header = match kind {
            Some(ScanKind::Quick) => format!("Último análisis: hoy, {h}:{m:02} · rápido"),
            Some(ScanKind::Full) => format!("Último análisis: hoy, {h}:{m:02} · completo"),
            None => format!("Última revisión: hoy, {h}:{m:02}"),
        };
        let defender_ok = kind.is_some() && def.problem.is_none() && !def.needs_admin;
        let phase = if n > 0 {
            let (title, mut text) = checks::summary(threats, n - threats);
            if let Some(p) = &def.problem {
                text = format!("{text} {p}");
            }
            if !failed.is_empty() {
                text = format!("{text} No se pudo revisar: {}.", failed.join(", "));
            }
            data.set_banner_title(title.into());
            data.set_banner_text(text.into());
            data.set_banner_bad(bad);
            SecPhase::Findings
        } else {
            let text = if def.problem.is_some() { "Las comprobaciones de WinCleaner sí se han hecho." } else { "" };
            data.set_banner_title(def.problem.clone().unwrap_or_default().into());
            data.set_banner_text(text.into());
            data.set_banner_bad(false);
            SecPhase::Clean
        };
        data.set_clean_text(checks::clean_text(defender_ok, CHECKS.len(), &failed).into());
        data.set_incomplete(!failed.is_empty() || (kind.is_some() && !defender_ok));
        data.set_needs_admin(def.needs_admin);
        data.set_defender_running(false);
        data.set_header(header.as_str().into());
        data.set_phase(phase);
        app.global::<Nav>().set_security_badge(n as i32);
        let title = match n {
            0 if kind.is_some() && !defender_ok => "Defender no ha podido analizar".to_string(),
            0 if !failed.is_empty() => format!("{} de {} comprobaciones bien", CHECKS.len() - failed.len(), CHECKS.len()),
            0 => "Todo bien".to_string(),
            1 => "1 cosa que revisar".to_string(),
            n => format!("{n} cosas que revisar"),
        };
        STATE.with_borrow_mut(|st| {
            st.last = Some((phase, header.into()));
            st.summary.0 = title;
        });
        self.show_summary(app);
    }

    /// Para Defender en un hilo (puede tardar 5 s) y vuelve al estado de antes del análisis.
    fn cancel(&self) {
        let scan = STATE.with_borrow_mut(|st| {
            let s = st.scan.take()?;
            st.cancelling = true;
            Some(s)
        });
        let Some(mut scan) = scan else { return };
        self.tick();
        let k = self.clone();
        std::thread::spawn(move || {
            scan.cancel();
            drop(scan);
            let _ = k.app.clone().upgrade_in_event_loop(move |app| k.cancelled(&app));
        });
    }

    fn cancelled(&self, app: &AppWindow) {
        let last = STATE.with_borrow_mut(|st| {
            // Lo que llegue aún de las comprobaciones de este análisis se tira.
            st.run += 1;
            st.busy = false;
            st.cancelling = false;
            st.def = None;
            st.own = None;
            st.timer.stop();
            reset_checks(st, 0);
            st.last.clone()
        });
        let data = app.global::<SecurityData>();
        data.set_defender_running(false);
        let (phase, header) = last.unwrap_or((SecPhase::Idle, SharedString::new()));
        data.set_header(header);
        data.set_phase(phase);
    }

    /// La acción `j` del hallazgo `i`.
    fn act(&self, i: i32, j: i32) {
        let Some(app) = self.app.upgrade() else { return };
        let Some((run, i, a)) = STATE.with_borrow(|st| {
            let i = usize::try_from(i).ok()?;
            let a = st.findings.get(i)?.actions.get(usize::try_from(j).ok()?)?.clone();
            Some((st.run, i, a))
        }) else {
            return;
        };
        match a {
            Action::DisableStartup(n) => {
                let Some(item) = STATE.with_borrow(|st| st.starts.get(n).cloned()) else { return };
                // `set_enabled` vuelve a mirar si hace falta ser administrador.
                match win::startup::set_enabled(&item, false) {
                    Ok(()) => self.done(run, i, &a),
                    Err(_) => {
                        let text = if startup::needs_admin(item.source) && !app.get_admin() {
                            "Es del equipo: reinicia WinCleaner como administrador para desactivarlo."
                        } else {
                            "Windows no ha dejado cambiarlo."
                        };
                        self.failed(&app, &format!("No se pudo desactivar «{}».", item.name), text);
                    }
                }
            }
            Action::Kill(ref exe) => {
                let key = exe.to_ascii_lowercase();
                let n = STATE.with_borrow(|st| st.groups.iter().find(|g| g.key == key).map_or(1, |g| g.pids.len()));
                let name = Path::new(exe).file_name().map_or(exe.clone(), |f| f.to_string_lossy().into_owned());
                let k = self.clone();
                let shown = name.clone();
                procs::ask_kill(&app, key, n, &name, self.system_root.clone(), move |app, r| match r {
                    Ok(()) => k.done(run, i, &a),
                    Err(e) => {
                        let text = match e {
                            TermError::Denied if !app.get_admin() => {
                                "Windows ha denegado el acceso. Reinicia WinCleaner como administrador para intentarlo.".into()
                            }
                            e => procs::kill_error(e, &shown),
                        };
                        k.failed(app, &format!("No se pudo terminar «{shown}»."), &text);
                    }
                });
            }
            Action::OpenLocation(p) => {
                if Path::new(&p).is_absolute() {
                    win::open_in_explorer(Path::new(&p), false, false);
                }
            }
            // Solo el nombre del archivo, nunca la ruta (lleva el nombre de usuario).
            Action::Search(name) => win::open_url(&checks::search_url(&name)),
            Action::OpenHosts => {
                if let Some(dir) = win::system32() {
                    win::open_with_notepad(&Path::new(&dir).join(r"drivers\etc\hosts"));
                }
            }
            Action::OpenWindowsSecurity => win::open_url(SECURITY_URL),
        }
    }

    /// La acción `a` del hallazgo `i` ya está hecha: se quita su botón.
    fn done(&self, run: u32, i: usize, a: &Action) {
        STATE.with_borrow_mut(|st| {
            if st.run != run {
                return;
            }
            if let Some(f) = st.findings.get_mut(i) {
                f.actions.retain(|b| b != a);
                st.rows.set_row_data(i, row(f));
            }
        });
    }

    /// Una acción ha fallado: lo dice el aviso de arriba.
    fn failed(&self, app: &AppWindow, title: &str, text: &str) {
        let data = app.global::<SecurityData>();
        data.set_banner_title(title.into());
        data.set_banner_text(text.into());
        data.set_banner_bad(true);
    }
}

/// Al cerrar WinCleaner, un análisis en marcha se para (lo hace `Drop` de `Scan`).
pub fn shutdown() {
    drop(STATE.with_borrow_mut(|st| st.scan.take()));
}
