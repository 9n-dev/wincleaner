//! Firma Authenticode de un ejecutable, propia o por catálogo de Windows. Sin red: no se
//! comprueba la revocación y solo se usa lo que ya está en la caché de URL.

use std::collections::HashMap;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::sync::Mutex;

use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::{BOOL, BYTE, DWORD, FILETIME};
use winapi::shared::winerror::{
    CERT_E_REVOKED, TRUST_E_BAD_DIGEST, TRUST_E_EXPLICIT_DISTRUST, TRUST_E_NOSIGNATURE, TRUST_E_SUBJECT_NOT_TRUSTED,
};
use winapi::um::fileapi::{CreateFileW, OPEN_EXISTING};
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::softpub::WINTRUST_ACTION_GENERIC_VERIFY_V2;
use winapi::um::wincrypt::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW, PCCERT_CONTEXT};
use winapi::um::winnt::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ, HANDLE, LPCWSTR};
use winapi::um::wintrust::{
    WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE,
    WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WinVerifyTrust,
};

use super::{Handle, wide};

#[derive(Clone, Debug, PartialEq)]
pub enum Signature {
    /// Firma válida; el nombre del firmante (puede ir vacío).
    Signed(String),
    /// Ni firma propia ni catálogo que lo incluya.
    Unsigned,
    /// Firma mala de forma explícita.
    Invalid,
    /// No se pudo comprobar (E/S, acceso, cadena sin red, archivo que ya no está…).
    Unknown,
}

impl Signature {
    /// Para las reglas de seguridad: `Some(true)` si está firmado, `Some(false)` si se ha
    /// comprobado que no (sin firma o con firma mala) y `None` si no se sabe (R16: sin hallazgo).
    pub fn verified(&self) -> Option<bool> {
        match self {
            Signature::Signed(_) => Some(true),
            Signature::Unsigned | Signature::Invalid => Some(false),
            Signature::Unknown => None,
        }
    }
}

type HCatAdmin = HANDLE;
type HCatInfo = HANDLE;

/// `WINTRUST_CATALOG_INFO`, que winapi 0.3.9 no trae.
#[repr(C)]
struct CatalogInfo {
    cb_struct: DWORD,
    catalog_version: DWORD,
    catalog_file_path: LPCWSTR,
    member_tag: LPCWSTR,
    member_file_path: LPCWSTR,
    member_file: HANDLE,
    calculated_file_hash: *mut BYTE,
    cb_calculated_file_hash: DWORD,
    catalog_context: *const std::ffi::c_void,
    cat_admin: HCatAdmin,
}

/// `CATALOG_INFO`.
#[repr(C)]
struct CatalogFile {
    cb_struct: DWORD,
    file: [u16; 260],
}

/// Principio de `CRYPT_PROVIDER_SGNR`: solo se leen estos campos, a través del puntero que da Windows.
#[repr(C)]
struct ProvSigner {
    cb_struct: DWORD,
    verify_as_of: FILETIME,
    cs_cert_chain: DWORD,
    cert_chain: *const ProvCert,
}

/// Principio de `CRYPT_PROVIDER_CERT`.
#[repr(C)]
struct ProvCert {
    cb_struct: DWORD,
    cert: PCCERT_CONTEXT,
}

// winapi 0.3.9 no las declara. La feature `wintrust` de winapi ya enlaza la biblioteca que las
// exporta (la de mingw no trae las versiones 2).
unsafe extern "system" {
    fn CryptCATAdminAcquireContext2(
        admin: *mut HCatAdmin,
        subsystem: *const GUID,
        hash_algorithm: LPCWSTR,
        strong_hash_policy: *const std::ffi::c_void,
        flags: DWORD,
    ) -> BOOL;
    fn CryptCATAdminReleaseContext(admin: HCatAdmin, flags: DWORD) -> BOOL;
    fn CryptCATAdminCalcHashFromFileHandle2(
        admin: HCatAdmin,
        file: HANDLE,
        cb_hash: *mut DWORD,
        hash: *mut BYTE,
        flags: DWORD,
    ) -> BOOL;
    fn CryptCATAdminEnumCatalogFromHash(
        admin: HCatAdmin,
        hash: *mut BYTE,
        cb_hash: DWORD,
        flags: DWORD,
        prev: *mut HCatInfo,
    ) -> HCatInfo;
    fn CryptCATCatalogInfoFromContext(info: HCatInfo, out: *mut CatalogFile, flags: DWORD) -> BOOL;
    fn CryptCATAdminReleaseCatalogContext(admin: HCatAdmin, info: HCatInfo, flags: DWORD) -> BOOL;
    fn WTHelperProvDataFromStateData(state: HANDLE) -> *mut std::ffi::c_void;
    fn WTHelperGetProvSignerFromChain(
        data: *mut std::ffi::c_void,
        signer: DWORD,
        counter_signer: BOOL,
        counter_idx: DWORD,
    ) -> *const ProvSigner;
}

/// Contexto de catálogos; se suelta al salir.
struct CatAdmin(HCatAdmin);

impl Drop for CatAdmin {
    fn drop(&mut self) {
        // SAFETY: lo creó CryptCATAdminAcquireContext2 y solo se suelta aquí.
        unsafe { CryptCATAdminReleaseContext(self.0, 0) };
    }
}

/// Un catálogo encontrado; se suelta (antes que su contexto) al salir.
struct CatInfo<'a>(&'a CatAdmin, HCatInfo);

impl Drop for CatInfo<'_> {
    fn drop(&mut self) {
        // SAFETY: lo dio CryptCATAdminEnumCatalogFromHash con este contexto.
        unsafe { CryptCATAdminReleaseCatalogContext(self.0.0, self.1, 0) };
    }
}

/// Llama a WinVerifyTrust con `data` ya rellenado (falta la parte común), lee el firmante si
/// la firma es válida y siempre cierra el estado.
fn verify(mut data: WINTRUST_DATA) -> (i32, String) {
    data.cbStruct = size_of::<WINTRUST_DATA>() as DWORD;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` es un WINTRUST_DATA completo cuyos punteros viven durante la llamada.
    let r = unsafe { WinVerifyTrust(INVALID_HANDLE_VALUE.cast(), &mut action, (&raw mut data).cast()) };
    let signer = if r == 0 { signer(data.hWVTStateData) } else { String::new() };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: cierra el estado que abrió la llamada anterior (también si falló).
    unsafe { WinVerifyTrust(INVALID_HANDLE_VALUE.cast(), &mut action, (&raw mut data).cast()) };
    (r, signer)
}

/// Nombre del primer firmante del estado de WinVerifyTrust, o vacío.
fn signer(state: HANDLE) -> String {
    // SAFETY: `state` es el estado vivo de un WinVerifyTrust con éxito.
    let data = unsafe { WTHelperProvDataFromStateData(state) };
    if data.is_null() {
        return String::new();
    }
    // SAFETY: `data` es de Windows y sigue vivo hasta el CLOSE.
    let sgnr = unsafe { WTHelperGetProvSignerFromChain(data, 0, 0, 0) };
    // SAFETY: si no es nulo, apunta a un CRYPT_PROVIDER_SGNR, que empieza como `ProvSigner`.
    let Some(sgnr) = (unsafe { sgnr.as_ref() }) else { return String::new() };
    if sgnr.cs_cert_chain == 0 || sgnr.cert_chain.is_null() {
        return String::new();
    }
    // SAFETY: la cadena tiene al menos un certificado: el del firmante.
    let cert = unsafe { (*sgnr.cert_chain).cert };
    if cert.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 256];
    // SAFETY: certificado válido; `buf` mide lo que se le dice. Devuelve los caracteres con el nulo.
    let n = unsafe {
        CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null_mut(), buf.as_mut_ptr(), buf.len() as DWORD)
    };
    let n = (n as usize).saturating_sub(1).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Resultado de WinVerifyTrust:
///
/// | HRESULT | Firma |
/// |---|---|
/// | 0 (`ERROR_SUCCESS`) | `Signed` |
/// | `TRUST_E_BAD_DIGEST` (0x80096010), `TRUST_E_EXPLICIT_DISTRUST` (0x800B0111), `CERT_E_REVOKED` (0x800B010C), `TRUST_E_SUBJECT_NOT_TRUSTED` (0x800B0004) | `Invalid` |
/// | `TRUST_E_NOSIGNATURE` (0x800B0100) del archivo | se prueba el catálogo; si no está en ninguno, `Unsigned` |
/// | cualquier otro (cadena que no se puede construir sin red, raíz desconocida, E/S…) | `Unknown` |
fn from_result(r: i32, signer: String) -> Signature {
    match r {
        0 => Signature::Signed(signer),
        TRUST_E_BAD_DIGEST | TRUST_E_EXPLICIT_DISTRUST | CERT_E_REVOKED | TRUST_E_SUBJECT_NOT_TRUSTED => Signature::Invalid,
        _ => Signature::Unknown,
    }
}

/// Comprueba la firma de `path`: primero la del propio archivo y, si no tiene, la de los
/// catálogos de Windows (así van firmados casi todos sus ejecutables).
pub fn check(path: &str) -> Signature {
    let w = wide(path);
    // SAFETY: estructura C de enteros y punteros: a ceros es válida.
    let mut file: WINTRUST_FILE_INFO = unsafe { zeroed() };
    file.cbStruct = size_of::<WINTRUST_FILE_INFO>() as DWORD;
    file.pcwszFilePath = w.as_ptr();
    // SAFETY: igual que `file`.
    let mut data: WINTRUST_DATA = unsafe { zeroed() };
    data.dwUnionChoice = WTD_CHOICE_FILE;
    // SAFETY: la unión es un puntero; `file` vive durante `verify`.
    unsafe { *data.u.pFile_mut() = &mut file };
    let (r, signer) = verify(data);
    if r != TRUST_E_NOSIGNATURE {
        return from_result(r, signer);
    }
    catalog(&w)
}

/// Firma por catálogo: `Unsigned` si no está en ninguno; `Unknown` si no se puede saber.
fn catalog(w: &[u16]) -> Signature {
    catalog_inner(w).unwrap_or(Signature::Unknown)
}

/// `None`: algún paso falló antes de saber si está en un catálogo. Los catálogos antiguos solo
/// tienen el hash SHA1: si con SHA256 no está en ninguno, se prueba con SHA1.
fn catalog_inner(w: &[u16]) -> Option<Signature> {
    for alg in ["SHA256", "SHA1"] {
        if let Some(s) = in_catalog(w, alg)? {
            return Some(s);
        }
    }
    // No está en ningún catálogo: de verdad sin firma.
    Some(Signature::Unsigned)
}

/// La firma del catálogo que tiene el hash `alg` del archivo; `Some(None)` si no está en ninguno.
fn in_catalog(w: &[u16], alg: &str) -> Option<Option<Signature>> {
    // SAFETY: ruta terminada en nulo; solo lectura, compartida con todo.
    let file = Handle(unsafe {
        CreateFileW(
            w.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    });
    if file.0 == INVALID_HANDLE_VALUE {
        return None;
    }
    let alg = wide(alg);
    let mut admin: HCatAdmin = null_mut();
    // SAFETY: subsistema y política nulos (los de por defecto); `admin` es nuestro.
    if unsafe { CryptCATAdminAcquireContext2(&mut admin, null(), alg.as_ptr(), null(), 0) } == 0 {
        return None;
    }
    let admin = CatAdmin(admin);
    let mut hash = [0u8; 64];
    let mut n = hash.len() as DWORD;
    // SAFETY: `hash` mide `n` bytes; `file` está abierto para leer.
    if unsafe { CryptCATAdminCalcHashFromFileHandle2(admin.0, file.0, &mut n, hash.as_mut_ptr(), 0) } == 0 {
        return None;
    }
    let n = (n as usize).min(hash.len());
    // SAFETY: hash de `n` bytes; sin catálogo anterior.
    let info = unsafe { CryptCATAdminEnumCatalogFromHash(admin.0, hash.as_mut_ptr(), n as DWORD, 0, null_mut()) };
    if info.is_null() {
        return Some(None);
    }
    let info = CatInfo(&admin, info);
    let mut cat = CatalogFile { cb_struct: size_of::<CatalogFile>() as DWORD, file: [0; 260] };
    // SAFETY: `cat` lleva su tamaño.
    if unsafe { CryptCATCatalogInfoFromContext(info.1, &mut cat, 0) } == 0 {
        return None;
    }
    // La etiqueta del miembro es el hash en hexadecimal.
    let tag = wide(hash[..n].iter().map(|b| format!("{b:02X}")).collect::<String>());
    let mut ci = CatalogInfo {
        cb_struct: size_of::<CatalogInfo>() as DWORD,
        catalog_version: 0,
        catalog_file_path: cat.file.as_ptr(),
        member_tag: tag.as_ptr(),
        member_file_path: w.as_ptr(),
        member_file: file.0,
        calculated_file_hash: hash.as_mut_ptr(),
        cb_calculated_file_hash: n as DWORD,
        catalog_context: null(),
        cat_admin: admin.0,
    };
    // SAFETY: igual que en `check`.
    let mut data: WINTRUST_DATA = unsafe { zeroed() };
    data.dwUnionChoice = WTD_CHOICE_CATALOG;
    // SAFETY: la unión es un puntero (aquí a WINTRUST_CATALOG_INFO); `ci` vive durante `verify`.
    unsafe { *data.u.pFile_mut() = (&raw mut ci).cast() };
    let (r, signer) = verify(data);
    Some(Some(from_result(r, signer)))
}

static CACHE: Mutex<Option<HashMap<String, Signature>>> = Mutex::new(None);

/// `check` con caché por ruta en minúsculas: un ejecutable no cambia de firma mientras corre.
pub fn check_cached(path: &str) -> Signature {
    // ponytail: la caché no se vacía; crece con los ejecutables distintos que se ven (pocos cientos).
    let key = path.to_lowercase();
    if let Some(s) = CACHE.lock().ok().and_then(|c| c.as_ref()?.get(&key).cloned()) {
        return s;
    }
    let s = check(path);
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(HashMap::new).insert(key, s.clone());
    }
    s
}
