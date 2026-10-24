//! Lectura del registro, y una sola escritura (`write_binary`, para `StartupApproved`).
//! Nada aquí hace panic: un tipo equivocado, un valor que falta, una cadena sin terminar o
//! unos datos enormes se toman como ausentes.

use std::iter::once;
use std::ptr::null_mut;

use winapi::shared::minwindef::{DWORD, HKEY};
use winapi::shared::winerror::{ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_SUCCESS};
use winapi::um::processenv::ExpandEnvironmentStringsW;
use winapi::um::winnt::{
    KEY_READ, KEY_SET_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_SZ,
};
use winapi::um::winreg::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, REGSAM,
};
use wincleaner::programs::{Hive, RawEntry};

use super::wide;

/// Ningún valor que leemos se acerca a esto: más grande, se toma como ausente.
const MAX_DATA: DWORD = 1 << 20;

pub const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

/// Raíz y vista: `Machine64`/`Machine32` son HKLM con la vista de 64 o de 32 bits; `User`, HKCU.
fn view(hive: Hive) -> (HKEY, REGSAM) {
    match hive {
        Hive::Machine64 => (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        Hive::Machine32 => (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
        Hive::User => (HKEY_CURRENT_USER, 0),
    }
}

/// Clave abierta; se cierra al soltarla.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: `self.0` lo abrió `RegOpenKeyExW` y nadie más lo cierra.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    fn open(hive: Hive, path: &str, access: REGSAM) -> Result<Key, u32> {
        let (root, view) = view(hive);
        let p = wide(path);
        let mut h: HKEY = null_mut();
        // SAFETY: `p` es UTF-16 terminada en nulo; `h` recibe la clave, que cierra `Drop`.
        let r = unsafe { RegOpenKeyExW(root, p.as_ptr(), 0, access | view, &mut h) };
        if r as u32 == ERROR_SUCCESS && !h.is_null() { Ok(Key(h)) } else { Err(r as u32) }
    }

    /// Como `open`, con permiso de escritura, y la crea si no está.
    fn create(hive: Hive, path: &str) -> Result<Key, u32> {
        let (root, view) = view(hive);
        let p = wide(path);
        let mut h: HKEY = null_mut();
        // SAFETY: `p` es UTF-16 terminada en nulo; sin clase ni seguridad; `h` recibe la clave,
        // que cierra `Drop`.
        let r = unsafe {
            RegCreateKeyExW(root, p.as_ptr(), 0, null_mut(), 0, KEY_SET_VALUE | view, null_mut(), &mut h, null_mut())
        };
        if r as u32 == ERROR_SUCCESS && !h.is_null() { Ok(Key(h)) } else { Err(r as u32) }
    }

    /// Tipo y datos del valor `name`. Si cambia de tamaño entre las dos llamadas, se reintenta.
    fn raw(&self, name: &str) -> Option<(DWORD, Vec<u8>)> {
        let n = wide(name);
        let mut len: DWORD = 0;
        // SAFETY: sin búfer de datos: solo pide el tamaño en `len`.
        let r = unsafe { RegQueryValueExW(self.0, n.as_ptr(), null_mut(), null_mut(), null_mut(), &mut len) };
        if r as u32 != ERROR_SUCCESS {
            return None;
        }
        for _ in 0..3 {
            if len > MAX_DATA {
                return None;
            }
            let mut buf = vec![0u8; len as usize];
            let (mut kind, mut got) = (0, len);
            // SAFETY: `buf` mide `got` bytes; la función escribe como mucho eso.
            let r = unsafe { RegQueryValueExW(self.0, n.as_ptr(), null_mut(), &mut kind, buf.as_mut_ptr(), &mut got) };
            match r as u32 {
                ERROR_SUCCESS => {
                    buf.truncate(got.min(len) as usize);
                    return Some((kind, buf));
                }
                ERROR_MORE_DATA => len = got,
                _ => return None,
            }
        }
        None
    }

    fn str(&self, name: &str) -> Option<String> {
        let (kind, data) = self.raw(name)?;
        string(kind, &data)
    }

    fn dword(&self, name: &str) -> Option<u32> {
        let (kind, data) = self.raw(name)?;
        if kind != REG_DWORD {
            return None;
        }
        Some(u32::from_le_bytes(data.get(..4)?.try_into().ok()?))
    }
}

/// `REG_SZ` tal cual y `REG_EXPAND_SZ` expandido; hasta el primer nulo, si lo hay.
fn string(kind: DWORD, data: &[u8]) -> Option<String> {
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return None;
    }
    let mut w: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    if let Some(fin) = w.iter().position(|&c| c == 0) {
        w.truncate(fin);
    }
    let expanded = if kind == REG_EXPAND_SZ { expand(&w) } else { None };
    Some(expanded.unwrap_or_else(|| String::from_utf16_lossy(&w)))
}

/// `%SystemRoot%\…` → `C:\Windows\…`. Si falla, `None` y se usa el texto sin expandir.
fn expand(w: &[u16]) -> Option<String> {
    let src: Vec<u16> = w.iter().copied().chain(once(0)).collect();
    // SAFETY: `src` termina en nulo; sin búfer, devuelve el tamaño necesario con el nulo.
    let n = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), null_mut(), 0) };
    if n == 0 || n > MAX_DATA {
        return None;
    }
    let mut buf = vec![0u16; n as usize];
    // SAFETY: `buf` tiene `n` u16, que es lo que se le dice.
    let m = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), n) };
    if m == 0 || m > n {
        return None;
    }
    let fin = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..fin]))
}

/// `%SystemRoot%\…` → `C:\Windows\…`; si falla, el texto tal cual.
pub fn expand_env(s: &str) -> String {
    let w: Vec<u16> = s.encode_utf16().collect();
    expand(&w).unwrap_or_else(|| s.to_string())
}

pub fn read_str(hive: Hive, path: &str, name: &str) -> Option<String> {
    Key::open(hive, path, KEY_READ).ok()?.str(name)
}

pub fn read_dword(hive: Hive, path: &str, name: &str) -> Option<u32> {
    Key::open(hive, path, KEY_READ).ok()?.dword(name)
}

/// Solo `REG_BINARY`.
pub fn read_binary(hive: Hive, path: &str, name: &str) -> Option<Vec<u8>> {
    let (kind, data) = Key::open(hive, path, KEY_READ).ok()?.raw(name)?;
    (kind == REG_BINARY).then_some(data)
}

/// Escribe un `REG_BINARY`; la clave se crea si no está (un usuario que nunca ha desactivado
/// nada no tiene `StartupApproved\Run`). El error es el código de Win32.
pub fn write_binary(hive: Hive, path: &str, name: &str, data: &[u8]) -> Result<(), u32> {
    let k = Key::create(hive, path)?;
    let len = DWORD::try_from(data.len()).map_err(|_| ERROR_INVALID_PARAMETER)?;
    let n = wide(name);
    // SAFETY: `n` termina en nulo y `data` mide `len` bytes.
    let r = unsafe { RegSetValueExW(k.0, n.as_ptr(), 0, REG_BINARY, data.as_ptr(), len) };
    if r as u32 == ERROR_SUCCESS { Ok(()) } else { Err(r as u32) }
}

/// Nombres de las subclaves; vacío si la clave no se puede abrir.
pub fn subkeys(hive: Hive, path: &str) -> Vec<String> {
    let Ok(k) = Key::open(hive, path, KEY_READ) else { return Vec::new() };
    // Un nombre de clave mide como mucho 255 caracteres.
    let mut buf = [0u16; 256];
    let mut out = Vec::new();
    for i in 0.. {
        let mut len = buf.len() as DWORD;
        // SAFETY: `buf` mide `len` u16; lo demás es opcional y va nulo.
        let r = unsafe {
            RegEnumKeyExW(k.0, i, buf.as_mut_ptr(), &mut len, null_mut(), null_mut(), null_mut(), null_mut())
        };
        match r as u32 {
            ERROR_SUCCESS => out.push(String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())])),
            ERROR_MORE_DATA => continue,
            // ERROR_NO_MORE_ITEMS o cualquier otro (p. ej. la clave se ha borrado): se para.
            _ => break,
        }
    }
    out
}

/// Valores de texto (`REG_SZ`, `REG_EXPAND_SZ` expandido) como `(nombre, dato)`; los demás se saltan.
pub fn values(hive: Hive, path: &str) -> Vec<(String, String)> {
    let Ok(k) = Key::open(hive, path, KEY_READ) else { return Vec::new() };
    // Un nombre de valor mide como mucho 16 383 caracteres.
    let mut buf = vec![0u16; 16_384];
    let mut out = Vec::new();
    for i in 0.. {
        let mut len = buf.len() as DWORD;
        // SAFETY: `buf` mide `len` u16; sin datos: se leen luego por nombre.
        let r = unsafe {
            RegEnumValueW(k.0, i, buf.as_mut_ptr(), &mut len, null_mut(), null_mut(), null_mut(), null_mut())
        };
        match r as u32 {
            ERROR_SUCCESS => {
                let name = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
                if let Some(v) = k.str(&name) {
                    out.push((name, v));
                }
            }
            ERROR_MORE_DATA => continue,
            _ => break,
        }
    }
    out
}

/// Solo «no existe» cuenta como ausente: si no se puede abrir por otro motivo (permisos…),
/// la desinstalación no se da por hecha.
pub fn key_exists(hive: Hive, path: &str) -> bool {
    !matches!(Key::open(hive, path, KEY_READ), Err(ERROR_FILE_NOT_FOUND))
}

/// Todas las claves `Uninstall` de HKLM (64 y 32 bits) y de HKCU, tal como están.
pub fn uninstall_entries() -> Vec<RawEntry> {
    [Hive::Machine64, Hive::Machine32, Hive::User]
        .into_iter()
        .flat_map(|hive| subkeys(hive, UNINSTALL).into_iter().filter_map(move |key| uninstall_entry(hive, key)))
        .collect()
}

/// Una clave `Uninstall`, leída de nuevo (p. ej. al confirmar la desinstalación).
pub fn uninstall_entry(hive: Hive, key: String) -> Option<RawEntry> {
    let k = Key::open(hive, &format!("{UNINSTALL}\\{key}"), KEY_READ).ok()?;
    let flag = |n| k.dword(n) == Some(1);
    Some(RawEntry {
        hive,
        display_name: k.str("DisplayName"),
        publisher: k.str("Publisher"),
        install_date: k.str("InstallDate"),
        // Si no es REG_DWORD, `dword` da `None`: se toma como ausente.
        estimated_size_kb: k.dword("EstimatedSize"),
        uninstall_string: k.str("UninstallString"),
        windows_installer: flag("WindowsInstaller"),
        system_component: flag("SystemComponent"),
        parent_key_name: k.str("ParentKeyName"),
        display_icon: k.str("DisplayIcon"),
        key,
    })
}
