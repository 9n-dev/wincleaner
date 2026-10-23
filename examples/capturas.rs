//! Dibuja las pantallas de las maquetas en `target/capturas/`, con el renderizador por
//! software y sin ventana, para compararlas con los PNG del diseño.

#[cfg(windows)]
fn main() {
    eprintln!("Las capturas se hacen en Linux.");
}

#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use slint::ComponentHandle;
    use wincleaner::ui::{AppWindow, headless};

    headless::init();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/capturas");
    std::fs::create_dir_all(&dir)?;

    let app = AppWindow::new()?;
    for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
        let rgba = headless::render(app.window(), w, h);
        let file = std::fs::File::create(dir.join(format!("wcv2-inicio{suffix}.png")))?;
        let mut png = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?.write_image_data(&rgba)?;
    }
    Ok(())
}
