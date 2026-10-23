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
    // Resumen, con los datos de `inicio()` de las maquetas.
    use wincleaner::ui::{DriveRow, SummaryData};
    let data = app.global::<SummaryData>();
    data.set_drives(
        std::rc::Rc::new(slint::VecModel::from(vec![
            DriveRow { name: "C:".into(), label: "Sistema".into(), text: "64 GB libres de 476 GB".into(), used: 0.87, low: true },
            DriveRow { name: "D:".into(), label: "Datos".into(), text: "1,2 TB libres de 1,8 TB".into(), used: 0.34, low: false },
        ]))
        .into(),
    );
    data.set_clean_total("3,0 GB".into());
    data.set_clean_text("se pueden liberar en 6 categorías".into());
    data.set_security_title("Sin revisar todavía".into());
    data.set_security_text("Defender activo · firmas de hace 2 días".into());
    data.set_procs_title("128 procesos · 5,4 GB".into());
    data.set_procs_text("11 programas arrancan con Windows".into());
    data.set_programs_title("86 instalados · 38,2 GB".into());
    data.set_programs_text("El más grande: Steam (12,4 GB)".into());
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
    modal.set_open(false);

    // Analizar, con los datos de las maquetas.
    use wincleaner::ui::{AnalyzeData, DirRow};
    let data = app.global::<AnalyzeData>();
    data.set_drives(model(["C: · 64 GB libres", "D: · 1,2 TB libres"].map(Into::into).to_vec()));
    data.set_title("Analizando C:…".into());
    data.set_progress("412 803 archivos · 1 min 12 s".into());
    data.set_current("C:\\Users\\ana\\AppData\\Local\\Packages\\…".into());
    data.set_summary("Analizados 1 284 112 archivos en 2 min 41 s.".into());
    data.set_rows(model(
        [
            ("C:\\Users\\ana\\AppData\\Local\\Steam\\steamapps\\common\\Baldur's Gate 3", "118 GB", 1.0),
            ("C:\\Users\\ana\\Videos\\OBS", "41,2 GB", 0.35),
            ("C:\\Windows\\WinSxS", "9,8 GB", 0.08),
            ("C:\\Users\\ana\\AppData\\Local\\Docker\\wsl", "8,6 GB", 0.07),
            ("C:\\hiberfil.sys", "6,4 GB", 0.05),
            ("C:\\Users\\ana\\AppData\\Local\\Google\\Chrome\\User Data", "3,9 GB", 0.03),
            ("C:\\ProgramData\\Package Cache", "2,7 GB", 0.02),
            ("C:\\Users\\ana\\Downloads\\isos", "2,1 GB", 0.02),
        ]
        .map(|(p, s, share)| DirRow { path: p.into(), size: s.into(), share })
        .to_vec(),
    ));
    app.set_section(2);
    for (nombre, phase, selected) in [("analizar", 0, -1), ("analizar-progreso", 1, -1), ("analizar-resultados", 2, 1)] {
        data.set_phase(phase);
        data.set_selected(selected);
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wcv2-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }
    app.set_section(0);

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
