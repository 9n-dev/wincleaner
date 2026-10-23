//! Lanzar el desinstalador de un programa y saber si sigue en marcha. Siempre por la ruta
//! absoluta que da `programs::uninstall_command`: nada se busca en `PATH` ni en la carpeta
//! actual.

use std::mem::zeroed;
use std::path::Path;
use std::ptr::{null, null_mut};

use winapi::shared::minwindef::{DWORD, FALSE, LPVOID};
use winapi::shared::winerror::{
    ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_ELEVATION_REQUIRED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
    WAIT_TIMEOUT,
};
use winapi::um::handleapi::CloseHandle;
use winapi::um::jobapi2::{AssignProcessToJobObject, CreateJobObjectW, QueryInformationJobObject};
use winapi::um::processthreadsapi::{CreateProcessW, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, TerminateProcess};
use winapi::um::shellapi::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use winapi::um::synchapi::WaitForSingleObject;
use winapi::um::winbase::CREATE_SUSPENDED;
use winapi::um::winnt::{HANDLE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation};
use winapi::um::winuser::SW_SHOWNORMAL;
use wincleaner::programs::UninstallCmd;

use super::wide;

/// Un HANDLE propio (o nulo); se cierra al soltarlo.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: lo abrió Windows para nosotros y solo se cierra aquí.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// El desinstalador lanzado. Con job se siguen también los procesos que lance él (NSIS se
/// copia a Temp y sigue desde ahí); con `runas` no hay job. Solo cambia la ayuda de la
/// espera: la cierran la clave o «Ya he terminado» (R12).
pub struct Launched {
    process: Handle,
    job: Option<Handle>,
}

#[derive(Debug)]
pub enum LaunchError {
    NotFound,
    /// Acceso denegado o aviso de UAC cancelado.
    Denied,
    Other(u32),
}

fn error(code: u32) -> LaunchError {
    match code {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => LaunchError::NotFound,
        ERROR_ACCESS_DENIED | ERROR_CANCELLED => LaunchError::Denied,
        c => LaunchError::Other(c),
    }
}

fn last_error() -> u32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
}

/// Lanza `cmd` con su carpeta como carpeta actual (no la de WinCleaner). Si pide elevación,
/// lo repite con `runas`.
pub fn launch(cmd: &UninstallCmd) -> Result<Launched, LaunchError> {
    // Una comilla rompería la línea de comandos; en un nombre de archivo no puede haberla.
    if cmd.exe.contains('"') {
        return Err(LaunchError::NotFound);
    }
    let dir = Path::new(&cmd.exe).parent().map(wide);
    let dir_ptr = dir.as_ref().map_or(null(), |d| d.as_ptr());
    let app = wide(&cmd.exe);
    let mut line = wide(format!("\"{}\" {}", cmd.exe, cmd.args).trim_end());
    // SAFETY: estructuras C planas; se rellenan los campos que pide la API.
    let mut si: STARTUPINFOW = unsafe { zeroed() };
    si.cb = size_of::<STARTUPINFOW>() as DWORD;
    // SAFETY: igual que `si`.
    let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
    // SAFETY: `app` y `dir` terminan en nulo y `line` es un búfer propio y modificable, como
    // pide la API; sin heredar handles. Con `lpApplicationName` no se busca nada en PATH.
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            null_mut(),
            null_mut(),
            FALSE,
            CREATE_SUSPENDED,
            null_mut(),
            dir_ptr,
            &mut si,
            &mut pi,
        )
    };
    if ok == 0 {
        return match last_error() {
            ERROR_ELEVATION_REQUIRED => runas(cmd, dir_ptr),
            e => Err(error(e)),
        };
    }
    let (process, thread) = (Handle(pi.hProcess), Handle(pi.hThread));
    // Antes de que arranque, para que todo lo que lance quede dentro. Si falla, se sigue sin
    // job: la espera acaba al desaparecer la clave o con «Ya he terminado».
    // SAFETY: sin atributos ni nombre; el handle (o nulo) lo cierra `Handle`.
    let job = Handle(unsafe { CreateJobObjectW(null_mut(), null()) });
    // SAFETY: `job` es nulo (no se llama) o un job recién creado; `process`, el proceso suspendido.
    let job = (!job.0.is_null() && unsafe { AssignProcessToJobObject(job.0, process.0) } != 0).then_some(job);
    // SAFETY: `thread` es el hilo principal, suspendido.
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        let e = last_error();
        // SAFETY: el proceso no ha empezado; se termina para no dejarlo colgado.
        unsafe { TerminateProcess(process.0, 1) };
        return Err(LaunchError::Other(e));
    }
    Ok(Launched { process, job })
}

/// El mismo `exe` y argumentos, elevado. Sin job: el proceso elevado no se puede meter en uno.
fn runas(cmd: &UninstallCmd, dir: *const u16) -> Result<Launched, LaunchError> {
    let (verb, file, args) = (wide("runas"), wide(&cmd.exe), wide(&cmd.args));
    // SAFETY: estructura C plana; se rellenan los campos que pide la API.
    let mut sei: SHELLEXECUTEINFOW = unsafe { zeroed() };
    sei.cbSize = size_of::<SHELLEXECUTEINFOW>() as DWORD;
    // Los errores los enseña WinCleaner, no un cuadro de Windows.
    sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = args.as_ptr();
    sei.lpDirectory = dir;
    sei.nShow = SW_SHOWNORMAL;
    // SAFETY: las cadenas terminan en nulo y viven durante la llamada; `hProcess` (o nulo)
    // lo cierra `Handle`.
    if unsafe { ShellExecuteExW(&mut sei) } == 0 {
        return Err(error(last_error()));
    }
    Ok(Launched { process: Handle(sei.hProcess), job: None })
}

/// Con job, si queda algún proceso dentro; sin él, si el proceso lanzado sigue vivo. Si no
/// se puede saber, se toma como vivo: la espera la cierra la clave o «Ya he terminado».
pub fn alive(l: &Launched) -> bool {
    if let Some(job) = &l.job {
        // SAFETY: estructura C plana que rellena la llamada.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        // SAFETY: `info` mide lo que se le dice; `job` es un job abierto.
        let ok = unsafe {
            QueryInformationJobObject(
                job.0,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as LPVOID,
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as DWORD,
                null_mut(),
            )
        };
        return ok == 0 || info.ActiveProcesses > 0;
    }
    // SAFETY: `process` es un proceso abierto; espera 0 ms, solo consulta.
    l.process.0.is_null() || unsafe { WaitForSingleObject(l.process.0, 0) } == WAIT_TIMEOUT
}
