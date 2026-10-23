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
        assert_eq!(pixel(&wide, 1100, 600, 700), bg);
        let min = render(app.window(), 880, 600);
        assert_eq!(pixel(&min, 880, 30, 400), s1);
        assert_eq!(pixel(&min, 880, 70, 400), bg);
    }

    /// Un clic en la casilla escribe en el modelo y avisa con su índice; Rust decide.
    #[test]
    fn la_casilla_escribe_en_el_modelo_y_avisa() {
        use crate::ui::{CatState, CategoryRow, CleanData, CleanPhase};
        use slint::platform::PointerEventButton;
        use slint::{LogicalPosition, Model, VecModel};
        use std::cell::Cell;

        init();
        let app = AppWindow::new().unwrap();
        app.set_section(1);
        let data = app.global::<CleanData>();
        let row = CategoryRow { name: "Temporales".into(), state: CatState::Ready, checked: true, ..Default::default() };
        let rows = Rc::new(VecModel::from(vec![row.clone(), row]));
        data.set_rows(rows.clone().into());
        data.set_phase(CleanPhase::Ready);
        let toggled = Rc::new(Cell::new(-1));
        let t = toggled.clone();
        data.on_toggled(move |i| t.set(i));
        render(app.window(), 1100, 720);
        // Casilla de la segunda fila: 216 + 32 + 16 + 9, 96 + 57 + 28.
        let position = LogicalPosition::new(273.0, 181.0);
        let button = PointerEventButton::Left;
        app.window().dispatch_event(WindowEvent::PointerPressed { position, button });
        app.window().dispatch_event(WindowEvent::PointerReleased { position, button });
        assert_eq!(toggled.get(), 1);
        assert!(!rows.row_data(1).unwrap().checked);
        assert!(rows.row_data(0).unwrap().checked);
    }
}
