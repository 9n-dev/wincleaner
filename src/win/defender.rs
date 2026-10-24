//! Microsoft Defender en Windows: estado y amenazas por WMI, otros antivirus del Centro de
//! seguridad y análisis con `MpCmdRun.exe`, buscado solo en sus rutas absolutas conocidas.
#![expect(dead_code, reason = "lo usa la Task 14")]

use std::collections::HashMap;
use std::io::{self, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use winapi::um::knownfolders::{FOLDERID_ProgramData, FOLDERID_ProgramFiles};
use winapi::um::winbase::CREATE_NO_WINDOW;
use wincleaner::categories::is_link;
use wincleaner::defender::{self, Detection, ScanKind, Status};

use super::known_folder;
use super::wmi::{self, Value};

const DEFENDER_NS: &str = r"root\Microsoft\Windows\Defender";

/// Lo que se guarda de la salida de cada flujo; el resto se lee y se tira, para que el proceso
/// no se quede bloqueado escribiendo.
const OUTPUT_LIMIT: u64 = 256 * 1024;

/// Estado de Defender, o `None` si WMI no lo da (desactivado del todo, otro antivirus, Server…).
pub fn status() -> Option<Status> {
    let rows = wmi::query(
        DEFENDER_NS,
        "SELECT AntivirusEnabled, AntivirusSignatureAge FROM MSFT_MpComputerStatus",
        &["AntivirusEnabled", "AntivirusSignatureAge"],
    )
    .ok()?;
    let row = rows.first()?;
    let Value::Bool(enabled) = row[0] else { return None };
    let signature_age_days = match row[1] {
        Value::Int(d) => u32::try_from(d).ok(),
        _ => None,
    };
    Some(Status { enabled, signature_age_days })
}

/// Nombres de los antivirus que conoce el Centro de seguridad (Defender incluido).
pub fn other_avs() -> Vec<String> {
    let rows = wmi::query(r"root\SecurityCenter2", "SELECT displayName FROM AntiVirusProduct", &["displayName"]);
    rows.unwrap_or_default()
        .into_iter()
        .filter_map(|r| match r.into_iter().next() {
            Some(Value::Str(s)) if !s.is_empty() => Some(s),
            _ => None,
        })
        .collect()
}

/// `ThreatID` es `sint64`, que WMI da como texto; por si acaso, también se acepta un número.
fn id(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.clone()),
        Value::Int(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Amenazas que Defender detectó a partir de `t`, una por detección, con su primer recurso.
/// Vacío si WMI falla.
pub fn detections_since(t: SystemTime) -> Vec<Detection> {
    let Ok(found) = wmi::query(
        DEFENDER_NS,
        "SELECT ThreatID, Resources, InitialDetectionTime FROM MSFT_MpThreatDetection",
        &["ThreatID", "Resources", "InitialDetectionTime"],
    ) else {
        return Vec::new();
    };
    let names: HashMap<String, String> = wmi::query(
        DEFENDER_NS,
        "SELECT ThreatID, ThreatName FROM MSFT_MpThreat",
        &["ThreatID", "ThreatName"],
    )
    .unwrap_or_default()
    .into_iter()
    .filter_map(|r| match (id(&r[0]), &r[1]) {
        (Some(k), Value::Str(n)) => Some((k, n.clone())),
        _ => None,
    })
    .collect();
    found
        .into_iter()
        .filter_map(|r| {
            let Value::Str(when) = &r[2] else { return None };
            let when = defender::parse_wmi_time(when).filter(|w| *w >= t)?;
            let path = match &r[1] {
                Value::StrList(v) => v.first().map(|s| defender::parse_resource(s)).unwrap_or_default(),
                Value::Str(s) => defender::parse_resource(s),
                _ => String::new(),
            };
            let name = id(&r[0]).and_then(|k| names.get(&k).cloned()).unwrap_or_else(|| "Amenaza".into());
            Some(Detection { name, path, when })
        })
        .collect()
}

/// `MpCmdRun.exe`: el de la plataforma más reciente y, si no está, el de `Program Files`. Solo
/// con rutas de `known_folder` y si es un archivo de verdad (no un enlace ni otro punto de análisis).
pub fn mpcmdrun() -> Option<PathBuf> {
    let latest = known_folder(&FOLDERID_ProgramData).and_then(|pd| {
        let dir = pd.join(r"Microsoft\Windows Defender\Platform");
        let names: Vec<String> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter(|e| std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir() && !is_link(&m)))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        Some(dir.join(defender::latest_platform(&names)?).join("MpCmdRun.exe"))
    });
    let classic = known_folder(&FOLDERID_ProgramFiles).map(|p| p.join(r"Windows Defender\MpCmdRun.exe"));
    [latest, classic]
        .into_iter()
        .flatten()
        .find(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && !is_link(&m)))
}

/// Un análisis de `MpCmdRun.exe` en marcha.
pub struct Scan {
    exe: PathBuf,
    child: Child,
    /// Los hilos que leen la salida y los errores.
    out: Vec<JoinHandle<Vec<u8>>>,
    done: Option<(i32, String)>,
}

/// Lee `r` hasta el final en un hilo, guardando como mucho `OUTPUT_LIMIT` bytes.
fn drain(r: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut r = r;
        let mut buf = Vec::new();
        let _ = (&mut r).take(OUTPUT_LIMIT).read_to_end(&mut buf);
        let _ = io::copy(&mut r, &mut io::sink());
        buf
    })
}

fn quiet(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null());
    c
}

/// Lanza un análisis sin ventana, con la salida capturada. `None` si no hay `MpCmdRun.exe` o no arranca.
pub fn start(k: ScanKind) -> Option<Scan> {
    let exe = mpcmdrun()?;
    let mut child = quiet(&exe).args(defender::scan_args(k)).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let mut out = Vec::new();
    out.extend(child.stdout.take().map(drain));
    out.extend(child.stderr.take().map(drain));
    Some(Scan { exe, child, out, done: None })
}

impl Scan {
    /// Sin bloquear: `Some((código, salida))` cuando ha terminado (-1 si no se sabe el código).
    pub fn finished(&mut self) -> Option<(i32, String)> {
        if self.done.is_none() {
            let code = match self.child.try_wait() {
                Ok(None) => return None,
                Ok(Some(s)) => s.code().unwrap_or(-1),
                Err(_) => -1,
            };
            // Con el proceso terminado, sus tuberías se cierran y los hilos acaban enseguida.
            let mut text = Vec::new();
            for h in std::mem::take(&mut self.out) {
                text.extend(h.join().unwrap_or_default());
            }
            self.done = Some((code, String::from_utf8_lossy(&text).into_owned()));
        }
        self.done.clone()
    }

    /// Pide a Defender que pare (`MpCmdRun.exe -Cancel`), espera hasta 5 s y, si el análisis
    /// sigue vivo, lo termina. Bloquea ese tiempo: llámalo fuera del hilo de la interfaz.
    pub fn cancel(&mut self) {
        let mut stop = quiet(&self.exe).arg("-Cancel").stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && matches!(self.child.try_wait(), Ok(None)) {
            thread::sleep(Duration::from_millis(100));
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        if let Some(s) = stop.as_mut()
            && matches!(s.try_wait(), Ok(None))
        {
            let _ = s.kill();
        }
    }
}
