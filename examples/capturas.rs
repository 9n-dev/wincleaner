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

    // Página de muestra: la genera `build.rs` (ver `ui/muestra.slint`).
    mod muestra {
        include!(concat!(env!("OUT_DIR"), "/muestra.rs"));
    }

    headless::init();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/capturas");
    std::fs::create_dir_all(&dir)?;

    let guarda = |nombre: &str, rgba: &[u8], w, h| -> Result<(), Box<dyn std::error::Error>> {
        let file = std::fs::File::create(dir.join(format!("{nombre}.png")))?;
        let mut png = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?.write_image_data(rgba)?;
        Ok(())
    };

    let app = AppWindow::new()?;
    for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
        guarda(&format!("wcv2-inicio{suffix}"), &headless::render(app.window(), w, h), w, h)?;
    }

    // Limpiar, con los datos de `CATS` de las maquetas.
    use wincleaner::ui::{CatState, CategoryRow, CleanData, CleanPhase, ReportRow};
    fn model<T: Clone + 'static>(v: Vec<T>) -> slint::ModelRc<T> {
        std::rc::Rc::new(slint::VecModel::from(v)).into()
    }
    let cats = |calculando: bool| {
        use CatState::*;
        let cats = [
            (true, "Temporales del usuario", "Archivos de %TEMP% sin tocar en más de 24 h", "1,4 GB", "8 312 archivos", Ready),
            (false, "Temporales de Windows", "%SystemRoot%\\Temp · más de 24 h", "", "", Admin),
            (false, "Papelera de reciclaje", "Se vacía entera. Empieza desmarcada", "640 MB", "37 elementos", Ready),
            (true, "Caché de miniaturas", "thumbcache del Explorador", "212 MB", "14 archivos", Ready),
            (true, "Caché de Chrome", "Caché de todos los perfiles", "980 MB", "5 120 archivos", Ready),
            (true, "Caché de Edge", "Caché de todos los perfiles", "310 MB", "2 044 archivos", Ready),
            (false, "Caché de Firefox", "cache2 de cada perfil", "", "", Missing),
            (true, "Volcados y logs antiguos", "Informes de errores de más de 30 días", "96 MB", "61 archivos", Ready),
            (false, "Caché de Windows Update", "%SystemRoot%\\SoftwareDistribution\\Download", "", "", Admin),
        ];
        let rows = cats.map(|(checked, name, desc, size, files, state)| CategoryRow {
            name: name.into(),
            desc: desc.into(),
            size: size.into(),
            files: files.into(),
            state: if calculando && state == Ready { Calculating } else { state },
            checked,
        });
        model(rows.to_vec())
    };
    app.set_section(1);
    let data = app.global::<CleanData>();
    data.set_total("3,0 GB".into());
    data.set_progress_name("Caché de Chrome".into());
    data.set_progress_text("1,7 GB de 3,0 GB".into());
    data.set_progress_detail("9 840 archivos borrados · 132 en uso, se saltan".into());
    data.set_progress(0.57);
    data.set_report_title("Has liberado 2,6 GB.".into());
    data.set_report_text("154 archivos estaban en uso y se han saltado. Es normal.".into());
    data.set_report(model(
        [
            ("Temporales del usuario", "1,3 GB", "118 en uso"),
            ("Caché de miniaturas", "0 B", "14 en uso"),
            ("Caché de Chrome", "962 MB", "—"),
            ("Caché de Edge", "288 MB", "22 en uso"),
            ("Volcados y logs antiguos", "96 MB", "—"),
        ]
        .map(|(n, f, s)| ReportRow { name: n.into(), freed: f.into(), skipped: s.into() })
        .to_vec(),
    ));
    let lineas = model(
        [
            ("Temporales del usuario", "1,4 GB"),
            ("Caché de miniaturas", "212 MB"),
            ("Caché de Chrome", "980 MB"),
            ("Caché de Edge", "310 MB"),
            ("Volcados y logs antiguos", "96 MB"),
        ]
        .map(|(l, v)| wincleaner::ui::ModalLine { label: l.into(), value: v.into() })
        .to_vec(),
    );
    let modal = app.global::<wincleaner::ui::Modal>();
    modal.set_title("¿Borrar 3,0 GB?".into());
    modal.set_text("Se va a borrar el contenido de estas categorías:".into());
    modal.set_lines(lineas);
    modal.set_note("Los archivos en uso se saltarán. Esto no se puede deshacer.".into());
    modal.set_confirm_text("Borrar".into());
    modal.set_danger(true);
    for (nombre, phase, calculando, abierto) in [
        ("limpiar-calculando", CleanPhase::Calculating, true, false),
        ("limpiar", CleanPhase::Ready, false, false),
        ("limpiar-confirmar", CleanPhase::Ready, false, true),
        ("limpiar-progreso", CleanPhase::Cleaning, false, false),
        ("limpiar-informe", CleanPhase::Report, false, false),
    ] {
        data.set_rows(cats(calculando));
        data.set_phase(phase);
        data.set_can_clean(phase == CleanPhase::Ready);
        app.set_locked(phase == CleanPhase::Cleaning);
        modal.set_open(abierto);
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wcv2-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }
    app.set_locked(false);
    app.set_section(0);
    modal.set_open(false);

    // Todos los componentes, con el modal abierto encima.
    use muestra::{Modal, ModalLine, Muestra};
    let m = Muestra::new()?;
    let modal = m.global::<Modal>();
    modal.set_title("¿Borrar 3,0 GB?".into());
    modal.set_text("Se va a borrar el contenido de estas categorías:".into());
    modal.set_lines(
        std::rc::Rc::new(slint::VecModel::from(vec![
            ModalLine { label: "Temporales".into(), value: "1,8 GB".into() },
            ModalLine { label: "Caché de miniaturas".into(), value: "1,2 GB".into() },
        ]))
        .into(),
    );
    modal.set_confirm_text("Borrar".into());
    modal.set_danger(true);
    modal.set_open(true);
    guarda("wcv2-componentes", &headless::render(m.window(), 1100, 720), 1100, 720)?;
    Ok(())
}
