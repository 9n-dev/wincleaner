//! Plataforma de Slint sin ventana, con el renderizador por software: capturas y
//! pruebas de teclado en Linux.

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter, WindowEvent};
use slint::{PhysicalSize, SharedString};

struct Headless;

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// Instala la plataforma en este hilo. Se puede llamar más de una vez.
pub fn init() {
    let _ = slint::platform::set_platform(Box::new(Headless));
}

/// Dibuja la ventana a `w`×`h` y devuelve los píxeles en RGBA8.
pub fn render(window: &slint::Window, w: u32, h: u32) -> Vec<u8> {
    window.set_size(PhysicalSize::new(w, h));
    slint::platform::update_timers_and_animations();
    let shot = window.take_snapshot().expect("captura de la ventana");
    shot.as_bytes().to_vec()
}

/// Pulsa y suelta una tecla (`"\n"`, `"\u{1b}"`, `" "`…).
pub fn press(window: &slint::Window, key: SharedString) {
    window.dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text: key });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::AppWindow;
    use slint::ComponentHandle;

    fn pixel(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn la_barra_lateral_se_estrecha_por_debajo_de_1000_px() {
        init();
        let app = AppWindow::new().unwrap();
        let (bg, s1) = ([0x17, 0x1e, 0x23, 255], [0x1c, 0x24, 0x2a, 255]);
        let wide = render(app.window(), 1100, 720);
        assert_eq!(wide.len(), 1100 * 720 * 4);
        assert_eq!(pixel(&wide, 1100, 150, 400), s1);
        assert_eq!(pixel(&wide, 1100, 600, 400), bg);
        let min = render(app.window(), 880, 600);
        assert_eq!(pixel(&min, 880, 30, 400), s1);
        assert_eq!(pixel(&min, 880, 150, 400), bg);
    }
}
