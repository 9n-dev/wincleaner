//! Icono de un programa en RGBA, para la lista de Programas. Se llama desde un hilo de
//! trabajo: el resultado es `Send` y la interfaz lo convierte en `Image`.

use std::mem::zeroed;
use std::ptr::null_mut;

use slint::{Rgba8Pixel, SharedPixelBuffer};
use winapi::shared::windef::{HBITMAP, HDC, HICON};
use winapi::um::shellapi::{ExtractIconExW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
use winapi::um::wingdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits,
    GetObjectW,
};
use winapi::um::winuser::{DestroyIcon, GetIconInfo, ICONINFO};

/// Icono grande (32×32 a 96 ppp) de `path`, el número `index` (negativo: id de recurso).
/// Si el archivo no tiene, el que le da el shell. Usa COM: el hilo lo tiene que iniciar.
/// Solo rutas locales con letra: un icono en `\\host\…` conectaría con la red.
pub fn load(path: &str, index: i32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    if !wincleaner::cmdline::local_path(path) {
        return None;
    }
    let w = super::wide(path);
    let mut big: HICON = null_mut();
    // SAFETY: ruta UTF-16 terminada en nulo; pide un solo icono grande, que queda en `big`.
    unsafe { ExtractIconExW(w.as_ptr(), index, &mut big, null_mut(), 1) };
    let icon = if !big.is_null() {
        big
    } else {
        // SAFETY: SHFILEINFOW son enteros, punteros y arrays: a ceros es válida.
        let mut info: SHFILEINFOW = unsafe { zeroed() };
        // SAFETY: `w` termina en nulo e `info` mide lo que se le dice.
        let r = unsafe {
            SHGetFileInfoW(w.as_ptr(), 0, &mut info, size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON)
        };
        if r == 0 || info.hIcon.is_null() {
            return None;
        }
        info.hIcon
    };
    let out = pixels(icon);
    // SAFETY: el icono es nuestro (de ExtractIconExW o SHGFI_ICON) y ya no se usa.
    unsafe { DestroyIcon(icon) };
    out
}

fn pixels(icon: HICON) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    // SAFETY: ICONINFO son enteros y handles: a ceros es válida.
    let mut ii: ICONINFO = unsafe { zeroed() };
    // SAFETY: `icon` es válido; si tiene éxito, los dos bitmaps son nuestros.
    if unsafe { GetIconInfo(icon, &mut ii) } == 0 {
        return None;
    }
    // Sin bitmap de color es un icono monocromo: se queda en la inicial.
    let out = if ii.hbmColor.is_null() { None } else { read(ii.hbmColor, ii.hbmMask) };
    for b in [ii.hbmColor, ii.hbmMask] {
        if !b.is_null() {
            // SAFETY: los creó GetIconInfo para nosotros y no se usan más.
            unsafe { DeleteObject(b.cast()) };
        }
    }
    out
}

/// Color BGRA → RGBA. Si el alfa vale 0 en todo el icono (iconos antiguos), sale de la máscara.
fn read(color: HBITMAP, mask: HBITMAP) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    // SAFETY: BITMAP son enteros y un puntero: a ceros es válida.
    let mut bm: BITMAP = unsafe { zeroed() };
    // SAFETY: `color` es un bitmap y `bm` mide lo que se le dice.
    if unsafe { GetObjectW(color.cast(), size_of::<BITMAP>() as i32, (&raw mut bm).cast()) } == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight);
    if !(1..=256).contains(&w) || !(1..=256).contains(&h) {
        return None;
    }
    // SAFETY: DC de memoria compatible con la pantalla; se borra abajo.
    let dc = unsafe { CreateCompatibleDC(null_mut()) };
    if dc.is_null() {
        return None;
    }
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
    let px = buf.make_mut_bytes();
    let ok = dib(dc, color, w, h, px);
    if ok && px.chunks_exact(4).all(|p| p[3] == 0) {
        let mut m = vec![0u8; px.len()];
        // Sin máscara legible, opaco.
        let hay = !mask.is_null() && dib(dc, mask, w, h, &mut m);
        for (p, m) in px.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
            // En la máscara, negro es opaco y blanco transparente.
            p[3] = if !hay || m[0] == 0 { 255 } else { 0 };
        }
    }
    // SAFETY: lo creó CreateCompatibleDC y no tiene nada seleccionado.
    unsafe { DeleteDC(dc) };
    if !ok {
        return None;
    }
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    Some(buf)
}

/// Copia `bmp` en `out` como 32 bits de arriba abajo; `out` mide `w * h * 4`.
fn dib(dc: HDC, bmp: HBITMAP, w: i32, h: i32, out: &mut [u8]) -> bool {
    if out.len() != w as usize * h as usize * 4 {
        return false;
    }
    // SAFETY: BITMAPINFO son enteros: a ceros es válida.
    let mut bi: BITMAPINFO = unsafe { zeroed() };
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        // Negativo: de arriba abajo.
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..bi.bmiHeader
    };
    // SAFETY: `out` tiene sitio para `h` filas de `w` píxeles de 32 bits (sin relleno) y
    // `bmp` no está seleccionado en ningún DC.
    let n = unsafe { GetDIBits(dc, bmp, 0, h as u32, out.as_mut_ptr().cast(), &mut bi, DIB_RGB_COLORS) };
    n == h
}
