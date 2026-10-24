//! Consultas WQL por WMI (COM), más los BSTR y VARIANT propios que usan también las tareas
//! programadas. Todo error de COM o de WMI llega como `Err(HRESULT)`.

use std::mem::zeroed;
use std::ptr::null_mut;
use std::sync::Once;

use winapi::ctypes::c_void;
use winapi::shared::rpcdce::{
    RPC_C_AUTHN_LEVEL_CALL, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE,
    RPC_C_IMP_LEVEL_IMPERSONATE,
};
use winapi::shared::wtypes::{
    BSTR, VT_ARRAY, VT_BOOL, VT_BSTR, VT_I1, VT_I2, VT_I4, VT_I8, VT_INT, VT_UI1, VT_UI2, VT_UI4,
    VT_UI8, VT_UINT,
};
use winapi::shared::wtypesbase::CLSCTX_INPROC_SERVER;
use winapi::um::combaseapi::{CoCreateInstance, CoInitializeSecurity, CoSetProxyBlanket};
use winapi::um::oaidl::VARIANT;
use winapi::um::objidlbase::EOAC_NONE;
use winapi::um::oleauto::{
    SafeArrayAccessData, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData,
    SysAllocString, SysFreeString, SysStringLen, VariantClear,
};
use winapi::um::unknwnbase::IUnknown;
use winapi::um::wbemcli::{
    IEnumWbemClassObject, IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY,
    WBEM_E_TIMED_OUT, WBEM_FLAG_CONNECT_USE_MAX_WAIT, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_S_TIMEDOUT,
    WbemLocator,
};
use winapi::{Class, Interface};

use super::{Com, wide, with_com};

/// Lo más que se espera a cada objeto: si WMI no contesta, la consulta falla en vez de colgarse.
const NEXT_TIMEOUT_MS: i32 = 30_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    StrList(Vec<String>),
    /// Nulo, vacío o de un tipo que no se lee (p. ej. reales). WMI da `uint64`, `sint64` y
    /// `datetime` como texto (`Str`).
    Null,
}

/// BSTR propio (o nulo); se libera al soltarlo.
pub(super) struct Bstr(pub(super) BSTR);

impl Bstr {
    /// Copia de `s`; nula si no hay memoria (COM la rechaza con un error).
    pub(super) fn new(s: &str) -> Self {
        let w = wide(s);
        // SAFETY: `w` es UTF-16 terminada en nulo; la copia es nuestra.
        Bstr(unsafe { SysAllocString(w.as_ptr()) })
    }

    pub(super) fn null() -> Self {
        Bstr(null_mut())
    }

    pub(super) fn text(&self) -> String {
        lossy(self.0)
    }
}

impl Drop for Bstr {
    fn drop(&mut self) {
        // SAFETY: es nuestro (o nulo, que SysFreeString acepta) y solo se libera aquí.
        unsafe { SysFreeString(self.0) };
    }
}

/// Texto de un BSTR que no es nuestro; vacío si es nulo.
fn lossy(b: BSTR) -> String {
    if b.is_null() {
        return String::new();
    }
    // SAFETY: un BSTR no nulo lleva delante su longitud en u16, que da SysStringLen.
    let s = unsafe { std::slice::from_raw_parts(b, SysStringLen(b) as usize) };
    String::from_utf16_lossy(s)
}

/// VARIANT propio; se vacía con `VariantClear` al soltarlo.
pub(super) struct Variant(pub(super) VARIANT);

impl Variant {
    /// `VT_EMPTY` (todo ceros, como `VariantInit`).
    pub(super) fn empty() -> Self {
        // SAFETY: un VARIANT a ceros es VT_EMPTY, válido.
        Variant(unsafe { zeroed() })
    }

    /// `VT_I4`, para los índices de las colecciones de COM. No hay nada que liberar.
    pub(super) fn int(i: i32) -> VARIANT {
        let mut v = Self::empty();
        // SAFETY: se escribe el tipo y el valor que le corresponde.
        unsafe {
            let n2 = v.0.n1.n2_mut();
            n2.vt = VT_I4 as u16;
            *n2.n3.lVal_mut() = i;
        }
        v.0
    }
}

impl Drop for Variant {
    fn drop(&mut self) {
        // SAFETY: es un VARIANT válido nuestro; VariantClear libera lo que lleve (BSTR, SAFEARRAY…).
        unsafe { VariantClear(&mut self.0) };
    }
}

/// Lee un VARIANT que ha rellenado WMI.
fn value(v: &Variant) -> Value {
    // SAFETY: `vt` dice qué campo de la unión es válido y solo se lee ese.
    unsafe {
        let n2 = v.0.n1.n2();
        let n3 = &n2.n3;
        match u32::from(n2.vt) {
            VT_BSTR => Value::Str(lossy(*n3.bstrVal())),
            VT_BOOL => Value::Bool(*n3.boolVal() != 0),
            VT_I1 => Value::Int(i64::from(*n3.cVal())),
            VT_UI1 => Value::Int(i64::from(*n3.bVal())),
            VT_I2 => Value::Int(i64::from(*n3.iVal())),
            VT_UI2 => Value::Int(i64::from(*n3.uiVal())),
            VT_I4 | VT_INT => Value::Int(i64::from(*n3.lVal())),
            VT_UI4 | VT_UINT => Value::Int(i64::from(*n3.ulVal())),
            VT_I8 => Value::Int(*n3.llVal()),
            VT_UI8 => i64::try_from(*n3.ullVal()).map_or(Value::Null, Value::Int),
            t if t == VT_ARRAY | VT_BSTR => str_list(*n3.parray()).map_or(Value::Null, Value::StrList),
            _ => Value::Null,
        }
    }
}

/// Las cadenas de un SAFEARRAY de BSTR de una dimensión.
///
/// # Safety
/// `sa` es nulo o un SAFEARRAY válido de BSTR.
unsafe fn str_list(sa: *mut winapi::um::oaidl::SAFEARRAY) -> Option<Vec<String>> {
    if sa.is_null() {
        return Some(Vec::new());
    }
    let (mut lo, mut hi) = (0, 0);
    let mut data: *mut c_void = null_mut();
    // SAFETY: `sa` es válido; se piden los límites de la dimensión 1 y se bloquean los datos.
    unsafe {
        if SafeArrayGetLBound(sa, 1, &mut lo) < 0
            || SafeArrayGetUBound(sa, 1, &mut hi) < 0
            || SafeArrayAccessData(sa, &mut data) < 0
        {
            return None;
        }
        // Vacío: hi = lo - 1.
        let n = usize::try_from(i64::from(hi) - i64::from(lo) + 1).unwrap_or(0);
        let out = std::slice::from_raw_parts(data.cast::<BSTR>(), n).iter().map(|&b| lossy(b)).collect();
        SafeArrayUnaccessData(sa);
        Some(out)
    }
}

static SECURITY: Once = Once::new();

/// Fija la seguridad de COM del proceso una vez. Si alguien (p. ej. la accesibilidad) ya la fijó,
/// devuelve `RPC_E_TOO_LATE`, que no importa: `CoSetProxyBlanket` pone la de cada proxy.
fn init_security() {
    SECURITY.call_once(|| {
        // SAFETY: sin descriptor ni lista de servicios (los de por defecto); COM ya está iniciado.
        unsafe {
            CoInitializeSecurity(
                null_mut(),
                -1,
                null_mut(),
                null_mut(),
                RPC_C_AUTHN_LEVEL_DEFAULT,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                null_mut(),
                EOAC_NONE,
                null_mut(),
            )
        };
    });
}

/// Suplantación en las llamadas de `proxy` (WMI la necesita para responder).
fn blanket(proxy: *mut IUnknown) -> i32 {
    // SAFETY: `proxy` es una interfaz válida nuestra; sin nombre ni credenciales: las del proceso.
    unsafe {
        CoSetProxyBlanket(
            proxy,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            null_mut(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            null_mut(),
            EOAC_NONE,
        )
    }
}

/// Ejecuta `wql` en el espacio de nombres local `namespace` (p. ej. `root\SecurityCenter2`) y
/// devuelve, por cada objeto, los valores de `fields` en ese orden. Inicia COM en el hilo.
pub fn query(namespace: &str, wql: &str, fields: &[&str]) -> Result<Vec<Vec<Value>>, i32> {
    with_com(|| run(namespace, wql, fields))
}

fn run(namespace: &str, wql: &str, fields: &[&str]) -> Result<Vec<Vec<Value>>, i32> {
    init_security();
    let clsid = WbemLocator::uuidof();
    // SAFETY: CLSID e IID válidos; si tiene éxito deja un IWbemLocator nuestro.
    let loc = unsafe {
        Com::<IWbemLocator>::out(|p| {
            CoCreateInstance(&clsid, null_mut(), CLSCTX_INPROC_SERVER, &IWbemLocator::uuidof(), p.cast())
        })
    }?;
    let ns = Bstr::new(namespace);
    // Con MAX_WAIT, la conexión se rinde a los 2 minutos en vez de esperar sin fin.
    let connect = WBEM_FLAG_CONNECT_USE_MAX_WAIT as i32;
    // SAFETY: `ns` vive durante la llamada; sin usuario, contraseña ni contexto (el usuario actual).
    let svc = unsafe {
        Com::<IWbemServices>::out(|p| {
            loc.ConnectServer(ns.0, null_mut(), null_mut(), null_mut(), connect, null_mut(), null_mut(), p)
        })
    }?;
    let hr = blanket(svc.0.cast());
    if hr < 0 {
        return Err(hr);
    }
    let (lang, q) = (Bstr::new("WQL"), Bstr::new(wql));
    let flags = (WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY) as i32;
    // SAFETY: las cadenas viven durante la llamada; si tiene éxito deja un enumerador nuestro.
    let en = unsafe {
        Com::<IEnumWbemClassObject>::out(|p| svc.ExecQuery(lang.0, q.0, flags, null_mut(), p))
    }?;
    // El enumerador también es un proxy; si no lo es, no hace falta y el error no importa.
    blanket(en.0.cast());
    let names: Vec<Vec<u16>> = fields.iter().map(wide).collect();
    let mut rows = Vec::new();
    loop {
        let mut obj: *mut IWbemClassObject = null_mut();
        let mut n = 0u32;
        // SAFETY: se pide un objeto; si llega (n == 1), `obj` es nuestro.
        let hr = unsafe { en.Next(NEXT_TIMEOUT_MS, 1, &mut obj, &mut n) };
        let obj = (!obj.is_null()).then(|| Com(obj));
        if hr < 0 {
            return Err(hr);
        }
        // WMI no contesta: se da como error (negativo), igual que los demás.
        if hr == WBEM_S_TIMEDOUT as i32 {
            return Err(WBEM_E_TIMED_OUT as i32);
        }
        // WBEM_S_FALSE (1) sin objeto: no hay más.
        let Some(obj) = obj.filter(|_| n == 1) else { break };
        let row = names
            .iter()
            .map(|f| {
                let mut v = Variant::empty();
                // SAFETY: nombre UTF-16 terminado en nulo; `v` está vacío y lo rellena Get.
                let hr = unsafe { obj.Get(f.as_ptr(), 0, &mut v.0, null_mut(), null_mut()) };
                if hr < 0 { Err(hr) } else { Ok(value(&v)) }
            })
            .collect::<Result<Vec<_>, _>>()?;
        rows.push(row);
    }
    Ok(rows)
}
