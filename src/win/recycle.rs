//! Papelera de Espacio: `send` envía con `IFileOperation` y un *sink* que frena lo que no cabe
//! en la papelera o cae en una zona protegida; `undo` lo devuelve a su sitio.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::null_mut;

use winapi::Interface;
use winapi::Class;
use winapi::shared::guiddef::{IsEqualIID, REFIID};
use winapi::shared::minwindef::{DWORD, UINT, ULONG};
use winapi::shared::ntdef::LPCWSTR;
use winapi::shared::winerror::{E_ABORT, E_FAIL, E_NOINTERFACE, HRESULT, S_OK};
use winapi::shared::wtypesbase::CLSCTX_INPROC_SERVER;
use winapi::ctypes::c_void;
use winapi::um::combaseapi::CoCreateInstance;
use winapi::um::shobjidl_core::{IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH};
use winapi::um::unknwnbase::{IUnknown, IUnknownVtbl};
use winapi::um::winbase::MoveFileExW;

use wincleaner::categories::is_link;
use wincleaner::clean::final_path_of;
use wincleaner::paths::same;
use wincleaner::space::recycle::{Reason, info_path, may_recycle, reason};
use wincleaner::space::zones::Zones;

use super::{Com, take_co_str, wide, with_com};
use ffi::{FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSinkVtbl};

/// winapi 0.3.9 no declara `IFileOperation` ni su *sink*. GUID y orden de la vtable según
/// `shobjidl_core.h`; los punteros a interfaces que no se usan van como `IUnknown`.
#[allow(dead_code, non_snake_case, clippy::too_many_arguments)] // la declaración trae todos los métodos, tal cual
mod ffi {
    use winapi::RIDL;
    use winapi::shared::minwindef::{BOOL, DWORD, UINT};
    use winapi::shared::ntdef::LPCWSTR;
    use winapi::shared::windef::HWND;
    use winapi::shared::winerror::HRESULT;
    use winapi::um::shobjidl_core::IShellItem;
    use winapi::um::unknwnbase::{IUnknown, IUnknownVtbl};

    RIDL! {#[uuid(0x3ad05575, 0x8857, 0x4850, 0x92, 0x77, 0x11, 0xb8, 0x5b, 0xdb, 0x8e, 0x09)]
    class FileOperation;}

    RIDL! {#[uuid(0x04b0f1a7, 0x9490, 0x44bc, 0x96, 0xe1, 0x42, 0x96, 0xa3, 0x12, 0x52, 0xe2)]
    interface IFileOperationProgressSink(IFileOperationProgressSinkVtbl): IUnknown(IUnknownVtbl) {
        fn StartOperations() -> HRESULT,
        fn FinishOperations(hrResult: HRESULT,) -> HRESULT,
        fn PreRenameItem(dwFlags: DWORD, psiItem: *mut IShellItem, pszNewName: LPCWSTR,) -> HRESULT,
        fn PostRenameItem(dwFlags: DWORD, psiItem: *mut IShellItem, pszNewName: LPCWSTR, hrRename: HRESULT,
            psiNewlyCreated: *mut IShellItem,) -> HRESULT,
        fn PreMoveItem(dwFlags: DWORD, psiItem: *mut IShellItem, psiDestinationFolder: *mut IShellItem,
            pszNewName: LPCWSTR,) -> HRESULT,
        fn PostMoveItem(dwFlags: DWORD, psiItem: *mut IShellItem, psiDestinationFolder: *mut IShellItem,
            pszNewName: LPCWSTR, hrMove: HRESULT, psiNewlyCreated: *mut IShellItem,) -> HRESULT,
        fn PreCopyItem(dwFlags: DWORD, psiItem: *mut IShellItem, psiDestinationFolder: *mut IShellItem,
            pszNewName: LPCWSTR,) -> HRESULT,
        fn PostCopyItem(dwFlags: DWORD, psiItem: *mut IShellItem, psiDestinationFolder: *mut IShellItem,
            pszNewName: LPCWSTR, hrCopy: HRESULT, psiNewlyCreated: *mut IShellItem,) -> HRESULT,
        fn PreDeleteItem(dwFlags: DWORD, psiItem: *mut IShellItem,) -> HRESULT,
        fn PostDeleteItem(dwFlags: DWORD, psiItem: *mut IShellItem, hrDelete: HRESULT,
            psiNewlyCreated: *mut IShellItem,) -> HRESULT,
        fn PreNewItem(dwFlags: DWORD, psiDestinationFolder: *mut IShellItem, pszNewName: LPCWSTR,) -> HRESULT,
        fn PostNewItem(dwFlags: DWORD, psiDestinationFolder: *mut IShellItem, pszNewName: LPCWSTR,
            pszTemplateName: LPCWSTR, dwFileAttributes: DWORD, hrNew: HRESULT, psiNewItem: *mut IShellItem,) -> HRESULT,
        fn UpdateProgress(iWorkTotal: UINT, iWorkSoFar: UINT,) -> HRESULT,
        fn ResetTimer() -> HRESULT,
        fn PauseTimer() -> HRESULT,
        fn ResumeTimer() -> HRESULT,
    }}

    RIDL! {#[uuid(0x947aab5f, 0x0a5c, 0x4c13, 0xb4, 0xd6, 0x4b, 0xf7, 0x83, 0x6f, 0xc9, 0xf8)]
    interface IFileOperation(IFileOperationVtbl): IUnknown(IUnknownVtbl) {
        fn Advise(pfops: *mut IFileOperationProgressSink, pdwCookie: *mut DWORD,) -> HRESULT,
        fn Unadvise(dwCookie: DWORD,) -> HRESULT,
        fn SetOperationFlags(dwOperationFlags: DWORD,) -> HRESULT,
        fn SetProgressMessage(pszMessage: LPCWSTR,) -> HRESULT,
        fn SetProgressDialog(popd: *mut IUnknown,) -> HRESULT,
        fn SetProperties(pproparray: *mut IUnknown,) -> HRESULT,
        fn SetOwnerWindow(hwndOwner: HWND,) -> HRESULT,
        fn ApplyPropertiesToItem(psiItem: *mut IShellItem,) -> HRESULT,
        fn ApplyPropertiesToItems(punkItems: *mut IUnknown,) -> HRESULT,
        fn RenameItem(psiItem: *mut IShellItem, pszNewName: LPCWSTR, pfopsItem: *mut IFileOperationProgressSink,) -> HRESULT,
        fn RenameItems(pUnkItems: *mut IUnknown, pszNewName: LPCWSTR,) -> HRESULT,
        fn MoveItem(psiItem: *mut IShellItem, psiDestinationFolder: *mut IShellItem, pszNewName: LPCWSTR,
            pfopsItem: *mut IFileOperationProgressSink,) -> HRESULT,
        fn MoveItems(punkItems: *mut IUnknown, psiDestinationFolder: *mut IShellItem,) -> HRESULT,
        fn CopyItem(psiItem: *mut IShellItem, psiDestinationFolder: *mut IShellItem, pszCopyName: LPCWSTR,
            pfopsItem: *mut IFileOperationProgressSink,) -> HRESULT,
        fn CopyItems(punkItems: *mut IUnknown, psiDestinationFolder: *mut IShellItem,) -> HRESULT,
        fn DeleteItem(psiItem: *mut IShellItem, pfopsItem: *mut IFileOperationProgressSink,) -> HRESULT,
        fn DeleteItems(punkItems: *mut IUnknown,) -> HRESULT,
        fn NewItem(psiDestinationFolder: *mut IShellItem, dwFileAttributes: DWORD, pszName: LPCWSTR,
            pszTemplateName: LPCWSTR, pfopsItem: *mut IFileOperationProgressSink,) -> HRESULT,
        fn PerformOperations() -> HRESULT,
        fn GetAnyOperationsAborted(pfAnyOperationsAborted: *mut BOOL,) -> HRESULT,
    }}
}

/// FOF_SILENT | FOF_NOCONFIRMATION | FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR | FOF_NOERRORUI |
/// FOF_NO_CONNECTED_ELEMENTS | FOFX_RECYCLEONDELETE. Sin FOF_NO_CONNECTED_ELEMENTS, borrar
/// `pagina.htm` se llevaría también `pagina_files\`, que nadie ha elegido ni comprobado.
/// Nunca FOFX_EARLYFAILURE: un fallo no para el resto.
const FLAGS: DWORD = 0x4 | 0x10 | 0x40 | 0x200 | 0x400 | 0x2000 | 0x8_0000;

#[derive(Debug, Default)]
pub struct Report {
    /// `(ruta original, ruta en la papelera)`.
    pub binned: Vec<(PathBuf, PathBuf)>,
    pub failed: Vec<(PathBuf, Reason)>,
}

/// Envía `items` a la papelera. Se comprueban las zonas sobre la ruta escrita y sobre la
/// final (sin seguir un enlace en el último componente); lo que no pasa sale como `Protected`.
/// Lo que no cabe en la papelera no se borra. Bloquea: va en un hilo de trabajo.
pub fn send(items: &[PathBuf], zones: &Zones) -> Report {
    let (ok, bad): (Vec<&PathBuf>, Vec<&PathBuf>) = items.iter().partition(|p| vetted(p, zones));
    let mut rep = with_com(|| perform(&ok, zones));
    rep.failed.extend(bad.into_iter().map(|p| (p.clone(), Reason::Protected)));
    rep
}

/// Se puede tocar: las zonas no la bloquean, no es un enlace y su ruta final es la escrita
/// (sin un junction por medio) y tampoco está bloqueada.
pub fn vetted(p: &Path, zones: &Zones) -> bool {
    let Some(s) = p.to_str() else { return false };
    zones.check(s).is_none()
        && std::fs::symlink_metadata(p).is_ok_and(|md| !is_link(&md))
        && final_path_of(p).is_some_and(|f| same(&f, s) && zones.check(&f).is_none())
}

fn perform(items: &[&PathBuf], zones: &Zones) -> Report {
    let mut rep = Report::default();
    if items.is_empty() {
        return rep;
    }
    let fail_all = |rep: &mut Report, list: &[&PathBuf], hr| rep.failed.extend(list.iter().map(|p| ((*p).clone(), reason(hr))));
    // El sink se declara antes que la operación para que viva más que ella, y en el montón: si
    // al final alguien lo sigue teniendo, se suelta sin liberarlo.
    let sink = Box::new(Sink { vtbl: &SINK_VTBL, refs: Cell::new(1), zones, out: RefCell::default() });
    // SAFETY: CLSID e IID válidos; si tiene éxito deja un IFileOperation nuestro.
    let op = unsafe {
        Com::<IFileOperation>::out(|p| {
            CoCreateInstance(&FileOperation::uuidof(), null_mut(), CLSCTX_INPROC_SERVER, &IFileOperation::uuidof(), p.cast())
        })
    };
    let op = match op {
        Ok(op) => op,
        Err(hr) => {
            fail_all(&mut rep, items, hr);
            return rep;
        }
    };
    // Sin flags o sin sink no hay quien frene un borrado definitivo: fallo cerrado.
    // SAFETY: `op` es válido; un DWORD.
    let hr = unsafe { op.SetOperationFlags(FLAGS) };
    if hr < 0 {
        fail_all(&mut rep, items, hr);
        return rep;
    }
    let mut queued = Vec::new();
    for p in items {
        // Sin `\\?\`, que IFileOperation no admite (y que `Zones::check` ya rechaza).
        let w = wide(p);
        // SAFETY: `w` es UTF-16 terminada en nulo; sin contexto; si tiene éxito deja un IShellItem nuestro.
        let item = unsafe {
            Com::<IShellItem>::out(|pp| SHCreateItemFromParsingName(w.as_ptr(), null_mut(), &IShellItem::uuidof(), pp.cast()))
        };
        // SAFETY: `op` e `item` son válidos; DeleteItem toma su propia referencia del item.
        let hr = item.map_or_else(|hr| hr, |item| unsafe { op.DeleteItem(item.0, null_mut()) });
        if hr < 0 {
            rep.failed.push(((*p).clone(), reason(hr)));
        } else {
            queued.push(*p);
        }
    }
    if queued.is_empty() {
        return rep;
    }
    let mut cookie = 0;
    // SAFETY: `sink` empieza por su vtable (repr C), no se mueve (está en una caja) y vive más que `op`.
    let hr = unsafe { op.Advise((&raw const *sink).cast_mut().cast(), &mut cookie) };
    if hr < 0 {
        fail_all(&mut rep, &queued, hr);
        return rep;
    }
    // SAFETY: `op` es válido y tiene el sink enganchado.
    let hr = unsafe { op.PerformOperations() };
    // SAFETY: la cookie es la que dio Advise.
    unsafe { op.Unadvise(cookie) };
    drop(op);
    // Cada elemento enviado se cuenta una vez: en la papelera, con el motivo que apuntó el sink
    // o, si el sink no llegó a verlo (operación cancelada), con el resultado de la operación.
    let raw = sink.out.take();
    // Si alguien se ha quedado una referencia, liberarlo sería un uso tras liberar: se pierde.
    // ponytail: fuga de unos bytes por envío en un caso que no debería darse.
    if sink.refs.get() > 1 {
        Box::leak(sink);
    }
    let is = |a: &Path, b: &Path| same(&a.to_string_lossy(), &b.to_string_lossy());
    for p in queued {
        if let Some((_, bin)) = raw.binned.iter().find(|(o, _)| is(o, p)) {
            rep.binned.push((p.clone(), bin.clone()));
        } else {
            let r = raw.failed.iter().find(|(o, _)| is(o, p)).map_or(reason(if hr < 0 { hr } else { E_FAIL }), |f| f.1);
            rep.failed.push((p.clone(), r));
        }
    }
    rep
}

/// Ruta de un IShellItem (`SIGDN_FILESYSPATH`); `None` si es nulo o no es del sistema de archivos.
///
/// # Safety
/// `item` es nulo o un IShellItem válido.
unsafe fn item_path(item: *mut IShellItem) -> Option<PathBuf> {
    if item.is_null() {
        return None;
    }
    let mut p = null_mut();
    // SAFETY: `item` es válido; `p` recibe una cadena de CoTaskMemAlloc (o nulo).
    let hr = unsafe { (*item).GetDisplayName(SIGDN_FILESYSPATH, &mut p) };
    // SAFETY: lo que deja GetDisplayName es nuestro y se libera dentro.
    let path = unsafe { take_co_str(p) };
    path.filter(|_| hr >= 0)
}

/// IFileOperationProgressSink en la pila: `AddRef`/`Release` cuentan pero no liberan nada.
#[repr(C)]
struct Sink<'a> {
    vtbl: *const IFileOperationProgressSinkVtbl,
    refs: Cell<u32>,
    zones: &'a Zones,
    out: RefCell<Report>,
}

/// # Safety
/// `this` es el puntero a un `Sink` vivo que se dio en `Advise`.
unsafe fn sink<'a>(this: *mut IFileOperationProgressSink) -> &'a Sink<'a> {
    // SAFETY: lo garantiza quien llama.
    unsafe { &*this.cast::<Sink>() }
}

unsafe extern "system" fn query_interface(this: *mut IUnknown, riid: REFIID, ppv: *mut *mut c_void) -> HRESULT {
    if ppv.is_null() {
        return 0x8000_4003_u32 as i32; // E_POINTER
    }
    // SAFETY: COM pasa un IID válido y un puntero de salida no nulo.
    unsafe {
        if IsEqualIID(&*riid, &IUnknown::uuidof()) || IsEqualIID(&*riid, &IFileOperationProgressSink::uuidof()) {
            *ppv = this.cast();
            add_ref(this);
            S_OK
        } else {
            *ppv = null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn add_ref(this: *mut IUnknown) -> ULONG {
    // SAFETY: `this` es nuestro sink, vivo mientras la operación lo tenga.
    let s = unsafe { sink(this.cast()) };
    s.refs.set(s.refs.get().saturating_add(1));
    s.refs.get()
}

unsafe extern "system" fn release(this: *mut IUnknown) -> ULONG {
    // SAFETY: como en `add_ref`.
    let s = unsafe { sink(this.cast()) };
    s.refs.set(s.refs.get().saturating_sub(1));
    s.refs.get()
}

unsafe extern "system" fn pre_delete(this: *mut IFileOperationProgressSink, flags: DWORD, item: *mut IShellItem) -> HRESULT {
    // SAFETY: COM llama con nuestro sink y un IShellItem válido (o nulo).
    let (s, path) = unsafe { (sink(this), item_path(item)) };
    let block = if !may_recycle(flags) {
        Some(Reason::NoBin)
    } else if path.as_deref().and_then(Path::to_str).is_none_or(|p| s.zones.check(p).is_some()) {
        Some(Reason::Protected)
    } else {
        None
    };
    let Some(r) = block else { return S_OK };
    if let Some(p) = path {
        s.out.borrow_mut().failed.push((p, r));
    }
    E_ABORT
}

unsafe extern "system" fn post_delete(this: *mut IFileOperationProgressSink, _flags: DWORD, item: *mut IShellItem,
    hr: HRESULT, new_item: *mut IShellItem) -> HRESULT {
    // SAFETY: COM llama con nuestro sink y IShellItem válidos (o nulos).
    let (s, path, bin) = unsafe { (sink(this), item_path(item), item_path(new_item)) };
    let Some(p) = path else { return S_OK };
    let mut out = s.out.borrow_mut();
    if hr < 0 {
        out.failed.push((p, reason(hr)));
    } else if !new_item.is_null() {
        // Sin ruta de la papelera no se podrá deshacer, pero está en ella.
        out.binned.push((p, bin.unwrap_or_default()));
    } else {
        // Borrado sin pasar por la papelera: no debería ocurrir, pero que se vea (y no se puede deshacer).
        out.failed.push((p, Reason::Purged));
    }
    S_OK
}

// Los demás métodos no hacen nada.
unsafe extern "system" fn nop(_: *mut IFileOperationProgressSink) -> HRESULT { S_OK }
unsafe extern "system" fn finish(_: *mut IFileOperationProgressSink, _: HRESULT) -> HRESULT { S_OK }
unsafe extern "system" fn pre_name(_: *mut IFileOperationProgressSink, _: DWORD, _: *mut IShellItem, _: LPCWSTR) -> HRESULT { S_OK }
unsafe extern "system" fn post_rename(_: *mut IFileOperationProgressSink, _: DWORD, _: *mut IShellItem, _: LPCWSTR,
    _: HRESULT, _: *mut IShellItem) -> HRESULT { S_OK }
unsafe extern "system" fn pre_move(_: *mut IFileOperationProgressSink, _: DWORD, _: *mut IShellItem, _: *mut IShellItem,
    _: LPCWSTR) -> HRESULT { S_OK }
unsafe extern "system" fn post_move(_: *mut IFileOperationProgressSink, _: DWORD, _: *mut IShellItem, _: *mut IShellItem,
    _: LPCWSTR, _: HRESULT, _: *mut IShellItem) -> HRESULT { S_OK }
unsafe extern "system" fn post_new(_: *mut IFileOperationProgressSink, _: DWORD, _: *mut IShellItem, _: LPCWSTR,
    _: LPCWSTR, _: DWORD, _: HRESULT, _: *mut IShellItem) -> HRESULT { S_OK }
unsafe extern "system" fn progress(_: *mut IFileOperationProgressSink, _: UINT, _: UINT) -> HRESULT { S_OK }

static SINK_VTBL: IFileOperationProgressSinkVtbl = IFileOperationProgressSinkVtbl {
    parent: IUnknownVtbl { QueryInterface: query_interface, AddRef: add_ref, Release: release },
    StartOperations: nop,
    FinishOperations: finish,
    PreRenameItem: pre_name,
    PostRenameItem: post_rename,
    PreMoveItem: pre_move,
    PostMoveItem: post_move,
    PreCopyItem: pre_move,
    PostCopyItem: post_move,
    PreDeleteItem: pre_delete,
    PostDeleteItem: post_delete,
    PreNewItem: pre_name,
    PostNewItem: post_new,
    UpdateProgress: progress,
    ResetTimer: nop,
    PauseTimer: nop,
    ResumeTimer: nop,
};

/// Devuelve cada elemento a su sitio sin reemplazar nada; da los que no se han podido restaurar.
pub fn undo(binned: &[(PathBuf, PathBuf)]) -> Vec<PathBuf> {
    binned.iter().filter(|(orig, bin)| !restore(orig, bin)).map(|(orig, _)| orig.clone()).collect()
}

fn restore(orig: &Path, bin: &Path) -> bool {
    if let Some(parent) = orig.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let (from, to) = (wide(verbatim(bin)), wide(verbatim(orig)));
    // SAFETY: rutas UTF-16 terminadas en nulo; flags 0: sin reemplazar ni copiar entre unidades.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return false;
    }
    // Sin su `$I`, el elemento deja de salir en la papelera.
    if let Some(info) = info_path(&bin.to_string_lossy()) {
        let _ = std::fs::remove_file(info);
    }
    true
}

/// `\\?\` delante de una ruta de unidad, para pasar de 260 caracteres en MoveFileExW.
fn verbatim(p: &Path) -> PathBuf {
    let Some(Component::Prefix(pre)) = p.components().next() else { return p.to_path_buf() };
    if !matches!(pre.kind(), Prefix::Disk(_)) {
        return p.to_path_buf();
    }
    let mut s = OsString::from(r"\\?\");
    s.push(p);
    s.into()
}
