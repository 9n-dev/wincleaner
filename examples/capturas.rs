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

    // «Acerca de», sobre el Resumen.
    app.set_version(env!("CARGO_PKG_VERSION").into());
    app.set_about_open(true);
    guarda("wcv2-acerca", &headless::render(app.window(), 1100, 720), 1100, 720)?;
    app.set_about_open(false);

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

    // Espacio, con los datos de `espacio*()`, `ROOT` y `VID` de las maquetas.
    use wincleaner::ui::{DriveItem, SpaceData, SpaceRow, StripPart};
    let data = app.global::<SpaceData>();
    data.set_drives(model(vec![
        DriveItem { name: "C:".into(), label: "Sistema".into(), text: "412 GB usados de 476 GB".into(), used: 0.87, low: true, analyzed: false },
        DriveItem { name: "D:".into(), label: "Datos".into(), text: "620 GB usados de 1,8 TB".into(), used: 0.34, low: false, analyzed: false },
    ]));
    data.set_progress("212 GB de 412 GB · 684 210 archivos · 1 min 05 s".into());
    data.set_progress_value(0.51);
    data.set_current("C:\\Users\\ana\\AppData\\Local\\Packages\\…".into());
    // (icono, nombre, ruta, tamaño en GB, %, color, pastilla, se puede borrar); `total` es la carpeta.
    type Fila<'a> = (&'a str, &'a str, &'a str, f32, &'a str, i32, &'a str, bool);
    let filas = |v: &[Fila], total: f32| {
        let rows: Vec<SpaceRow> = v
            .iter()
            .map(|&(icon, name, sub, gb, pct, color, badge, del)| SpaceRow {
                icon: icon.into(),
                name: name.into(),
                sub: sub.into(),
                size: format!("{gb} GB").replace('.', ",").into(),
                pct: pct.into(),
                share: gb / total,
                color,
                badge: badge.into(),
                can_enter: icon == "folder",
                can_delete: del,
                selectable: !name.ends_with("más pequeños"),
            })
            .collect();
        let mut strip: Vec<StripPart> =
            rows.iter().filter(|r| r.color < 5).map(|r| StripPart { label: r.name.clone(), share: r.share, color: r.color }).collect();
        let rest = 1.0 - strip.iter().map(|p| p.share).sum::<f32>();
        strip.push(StripPart { label: "Resto".into(), share: rest, color: 5 });
        (model(rows), model(strip))
    };
    let raiz = filas(
        &[
            ("game", "Steam", "C:\\Program Files (x86)\\Steam", 186.0, "45 %", 0, "Juegos", false),
            ("folder", "Vídeos", "C:\\Users\\ana\\Videos", 64.0, "16 %", 1, "", true),
            ("lock", "Windows", "C:\\Windows", 38.0, "9 %", 2, "Protegida", false),
            ("folder", "Descargas", "C:\\Users\\ana\\Downloads", 29.0, "7 %", 3, "", true),
            ("lock", "Archivos de programa", "C:\\Program Files", 24.0, "6 %", 4, "Protegida", false),
            ("file", "hiberfil.sys", "Archivo de hibernación de Windows", 6.4, "2 %", 5, "Sistema", false),
            ("folder", "AppData", "C:\\Users\\ana\\AppData", 5.9, "1 %", 5, "", true),
        ],
        412.0,
    );
    let videos = filas(
        &[
            ("folder", "OBS", "C:\\Users\\ana\\Videos\\OBS", 41.2, "64 %", 0, "", true),
            ("file", "boda_prima_4K.mov", "14/03/2024", 12.8, "20 %", 1, "", true),
            ("folder", "Capturas", "C:\\Users\\ana\\Videos\\Captures", 6.1, "10 %", 2, "", true),
            ("file", "partida_final.mkv", "02/09/2026", 2.4, "4 %", 3, "", true),
            ("file", "1 284 archivos más pequeños", "Juntos", 1.5, "2 %", 5, "", false),
        ],
        64.0,
    );
    let crumbs = |v: &[&str]| model(v.iter().map(|&c| c.into()).collect::<Vec<slint::SharedString>>());
    app.set_section(2);
    for (nombre, phase, filas, migas, selected, lock) in [
        ("espacio", 0, &raiz, &["Este equipo", "C:"][..], -1, false),
        ("espacio-analizando", 1, &raiz, &["Este equipo", "C:"][..], -1, false),
        ("espacio-raiz", 2, &raiz, &["Este equipo", "C:"][..], 1, false),
        ("espacio-carpeta", 2, &videos, &["Este equipo", "C:", "Usuarios", "ana", "Vídeos"][..], 1, false),
        ("espacio-protegida", 2, &raiz, &["Este equipo", "C:"][..], 2, true),
    ] {
        data.set_subtitle(
            match phase {
                0 => "Mira a dónde va cada GB y libera lo que no necesites.",
                1 => "Analizando C:…",
                _ => "C: · 412 GB usados de 476 GB · analizado hace 2 min",
            }
            .into(),
        );
        data.set_phase(phase);
        data.set_rows(filas.0.clone());
        data.set_strip(filas.1.clone());
        data.set_crumbs(crumbs(migas));
        data.set_selected(selected);
        data.set_lock_title(if lock { "Windows está protegida." } else { "" }.into());
        data.set_lock_text(
            if lock {
                "Puedes ver qué ocupa, pero no borrarla desde aquí: romperías el sistema. Para liberar espacio de Windows usa «Limpiar»."
            } else {
                ""
            }
            .into(),
        );
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wces-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }

    // Borrar desde Carpetas: papelera, borrar del todo, hecho y error, en Vídeos.
    let migas = crumbs(&["Este equipo", "C:", "Usuarios", "ana", "Vídeos"]);
    let sin_boda = filas(
        &[
            ("folder", "OBS", "C:\\Users\\ana\\Videos\\OBS", 41.2, "64 %", 0, "", true),
            ("folder", "Capturas", "C:\\Users\\ana\\Videos\\Captures", 6.1, "10 %", 1, "", true),
            ("file", "partida_final.mkv", "02/09/2026", 2.4, "4 %", 2, "", true),
            ("file", "1 284 archivos más pequeños", "Juntos", 1.5, "2 %", 5, "", false),
        ],
        51.2,
    );
    let boda = || model(vec![wincleaner::ui::ModalLine { label: "boda_prima_4K.mov".into(), value: "12,8 GB".into() }]);
    let purge = app.global::<wincleaner::ui::Purge>();
    data.set_crumbs(migas);
    data.set_lock_title("".into());
    data.set_lock_text("".into());
    for nombre in ["papelera", "borrar-todo", "hecho", "error"] {
        let (filas, selected) = if nombre == "hecho" { (&sin_boda, -1) } else { (&videos, if nombre == "error" { -1 } else { 1 }) };
        data.set_rows(filas.0.clone());
        data.set_strip(filas.1.clone());
        data.set_selected(selected);
        data.set_subtitle(
            match nombre {
                "hecho" => "C: · 399 GB usados de 476 GB",
                "error" => "C: · 412 GB usados de 476 GB · analizado hace 2 min",
                _ => "C: · 412 GB usados de 476 GB",
            }
            .into(),
        );
        data.set_note(match nombre {
            "hecho" => 1,
            "error" => 2,
            _ => 0,
        });
        data.set_note_title(
            if nombre == "hecho" { "Enviado a la papelera: boda_prima_4K.mov (12,8 GB)." } else { "No se ha podido borrar todo «OBS»." }.into(),
        );
        data.set_note_text(
            if nombre == "hecho" {
                "Vacía la papelera en «Limpiar» cuando quieras liberar el espacio."
            } else {
                "3 archivos están en uso por otro programa (38 GB se han enviado a la papelera; quedan 3,2 GB). \
                 Cierra los programas que los usen y vuelve a intentarlo."
            }
            .into(),
        );
        data.set_can_undo(nombre == "hecho");
        data.set_can_retry(nombre == "error");
        if nombre == "papelera" {
            modal.set_title("¿Enviar a la papelera?".into());
            modal.set_text("".into());
            modal.set_lines(boda());
            modal.set_note("Podrás recuperarlo desde la papelera mientras no la vacíes. El espacio no se libera hasta entonces.".into());
            modal.set_confirm_text("Enviar a la papelera".into());
            modal.set_danger(false);
            modal.set_alt_text("Borrar del todo…".into());
            modal.set_open(true);
        }
        if nombre == "borrar-todo" {
            purge.set_lines(boda());
            purge.set_text("No pasa por la papelera: se borra ya y libera 12,8 GB al momento.".into());
            purge.set_open(true);
            // El campo tiene el foco al abrir: se escribe como el usuario.
            headless::render(app.window(), 1100, 720);
            for c in ["B", "O", "R", "R"] {
                headless::press(app.window(), c.into());
            }
        }
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wces-espacio-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
        modal.set_open(false);
        purge.set_open(false);
    }
    data.set_note(0);

    // Juegos, con los datos de `GAMES` de las maquetas: lista, confirmar y vacío.
    use wincleaner::ui::{GameRow, GamesData};
    let games = [
        (0x8fb0c4, "B", "Baldur's Gate 3", "Steam · jugado hace 3 días", "118 GB"),
        (0xc4a7b4, "C", "Call of Duty®", "Battle.net · jugado hace 8 meses", "92 GB"),
        (0xd6b38a, "F", "Fortnite", "Epic Games · jugado hace 1 año", "34 GB"),
        (0xa9c2b8, "H", "Hollow Knight", "GOG · jugado hace 2 años", "9,2 GB"),
        (0x8fc2c0, "F", "Forza Horizon 5", "Xbox · jugado hace 5 meses", "128 GB"),
        (0x8fb0c4, "S", "Stardew Valley", "Steam · nunca jugado", "620 MB"),
    ]
    .map(|(c, l, n, d, s): (u32, _, _, _, _)| GameRow {
        name: n.into(),
        sub: d.into(),
        size: s.into(),
        letter: l.into(),
        tint: slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8),
    });
    let juegos = app.global::<GamesData>();
    juegos.set_phase(1);
    data.set_tab(1);
    modal.set_title("¿Desinstalar Call of Duty®?".into());
    modal.set_text("Se abrirá Battle.net para desinstalarlo: así el lanzador sabe que ya no está. Liberarás unos 92 GB.".into());
    modal.set_lines(model(Vec::new()));
    modal.set_note("".into());
    modal.set_confirm_text("Abrir Battle.net".into());
    modal.set_danger(true);
    modal.set_alt_text("".into());
    for (nombre, rows, selected) in
        [("juegos", games.to_vec(), 1), ("juegos-confirmar", games.to_vec(), 1), ("juegos-vacio", Vec::new(), -1)]
    {
        juegos.set_subtitle(if rows.is_empty() { "Ningún juego encontrado" } else { "6 juegos instalados · 382 GB en total" }.into());
        juegos.set_rows(model(rows));
        juegos.set_selected(selected);
        modal.set_open(nombre == "juegos-confirmar");
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wces-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }
    modal.set_open(false);

    // Duplicados, con los datos de `duplicados*()` y `DUPS` de las maquetas: dónde buscar,
    // buscando, resultados y vacío.
    use wincleaner::ui::{DupFileRow, DupGroupRow, DupPlace, DupStep, DupsData};
    let dups = app.global::<DupsData>();
    data.set_tab(2);
    dups.set_places(model(
        [
            ("Tus carpetas", "Documentos, Escritorio, Descargas, Imágenes, Vídeos y Música", true),
            ("D: Datos", "Toda la unidad", true),
            ("Resto de C:", "Sin Windows, Archivos de programa ni AppData", false),
        ]
        .map(|(n, t, c)| DupPlace { name: n.into(), text: t.into(), checked: c })
        .to_vec(),
    ));
    dups.set_steps(model(
        [("Listando archivos", "184 210 archivos", 2), ("Comparando tamaños", "6 120 posibles duplicados", 2), ("Comparando contenido", "2 340 de 6 120", 1)]
            .map(|(n, t, s)| DupStep { name: n.into(), text: t.into(), state: s })
            .to_vec(),
    ));
    dups.set_progress(0.38);
    let grupos = [
        ("Vídeo", "vacaciones_2023.mp4", "3 copias · sobran 4,2 GB", vec![
            (r"C:\Users\ana\Videos\vacaciones_2023.mp4", "14/08/2023", false),
            (r"C:\Users\ana\Downloads\vacaciones_2023.mp4", "02/09/2023", true),
            (r"D:\Copia\Vídeos\vacaciones_2023.mp4", "10/01/2024", true),
        ]),
        ("PDF", "Contrato_alquiler.pdf", "2 copias · sobran 2,1 MB", vec![
            (r"C:\Users\ana\Documents\Contrato_alquiler.pdf", "03/02/2025", false),
            (r"C:\Users\ana\Downloads\Contrato_alquiler (1).pdf", "03/02/2025", true),
        ]),
        ("Imagen", "IMG_4821.png", "2 copias · sobran 6,8 MB", vec![
            (r"C:\Users\ana\Pictures\Móvil\IMG_4821.png", "21/06/2026", false),
            (r"C:\Users\ana\Desktop\IMG_4821.png", "22/06/2026", true),
        ]),
    ]
    .map(|(k, n, t, fs)| DupGroupRow {
        kind: k.into(),
        name: n.into(),
        text: t.into(),
        files: model(fs.into_iter().map(|(p, d, m)| DupFileRow { path: p.into(), date: d.into(), marked: m, protected: false }).collect()),
    });
    dups.set_send_text("Enviar 412 copias a la papelera · 18,6 GB".into());
    dups.set_can_send(true);
    for (nombre, phase, subtitulo, rows) in [
        ("duplicados", 0, "Archivos idénticos repetidos en tus carpetas y unidades.", Vec::new()),
        ("duplicados-buscando", 1, "Buscando…", Vec::new()),
        ("duplicados-resultados", 2, "214 grupos · sobran 18,6 GB", grupos.to_vec()),
        ("duplicados-vacio", 2, "Búsqueda terminada en 48 s", Vec::new()),
    ] {
        dups.set_phase(phase);
        dups.set_subtitle(subtitulo.into());
        dups.set_groups(model(rows));
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wces-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }
    data.set_tab(0);

    // Programas, con los datos de `PROGS` de las maquetas.
    use wincleaner::ui::{ProgramRow, ProgramsData};
    let progs = [
        (0x8fb0c4, "S", "Steam", "Valve Corporation", "12,4 GB", "03/02/2025", true),
        (0xd68a8a, "A", "Adobe Acrobat (64-bit)", "Adobe", "1,1 GB", "14/09/2026", true),
        (0x8fc2c0, "Z", "Zoom Workplace", "Zoom Communications", "412 MB", "02/10/2026", false),
        (0xc4a7b4, "O", "OBS Studio", "OBS Project", "388 MB", "21/07/2026", true),
        (0x8fb0c4, "V", "Visual Studio Code", "Microsoft Corporation", "372 MB", "11/10/2026", false),
        (0xa9c2b8, "S", "Spotify", "Spotify AB", "310 MB", "19/10/2026", false),
        (0xd6b38a, "V", "VLC media player", "VideoLAN", "168 MB", "05/05/2026", true),
        (0x9ba6ad, "7", "7-Zip 24.08 (x64)", "Igor Pavlov", "5,9 MB", "17/08/2024", true),
    ]
    .map(|(c, l, n, p, s, d, adm): (u32, _, _, _, _, _, _)| ProgramRow {
        name: n.into(),
        publisher: p.into(),
        date: d.into(),
        size: s.into(),
        icon: Default::default(),
        letter: l.into(),
        tint: slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8),
        needs_admin: adm,
        blocked: false,
    });
    let data = app.global::<ProgramsData>();
    data.set_subtitle("86 instalados · 38,2 GB".into());
    data.set_loading(false);
    app.set_section(3);
    for (nombre, query, rows, selected) in
        [("programas", "", progs.to_vec(), 2), ("programas-sin-resultados", "photoshp", Vec::new(), -1)]
    {
        data.set_query(query.into());
        data.set_rows(model(rows));
        data.set_selected(selected);
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wcv2-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }

    // La desinstalación de Zoom Workplace: confirmar, esperar, hecho y error.
    data.set_query("".into());
    modal.set_title("¿Desinstalar Zoom Workplace?".into());
    modal.set_text("Se abrirá el desinstalador del propio programa. Sigue sus pasos; cuando se cierre, WinCleaner comprobará si se ha desinstalado.".into());
    modal.set_lines(model(Vec::new()));
    modal.set_note("".into());
    modal.set_confirm_text("Abrir desinstalador".into());
    let sin_zoom: Vec<_> = progs.iter().filter(|p| p.name != "Zoom Workplace").cloned().collect();
    for (nombre, rows, selected, phase, banner, title, text, path) in [
        ("programas-confirmar", progs.to_vec(), 2, 0, 0, "", "", ""),
        (
            "programas-esperando",
            progs.to_vec(),
            -1,
            1,
            0,
            "Esperando al desinstalador de Zoom Workplace…",
            "Termina en su ventana. La lista se actualizará cuando se cierre.",
            "",
        ),
        ("programas-hecho", sin_zoom, -1, 0, 1, "Zoom Workplace se ha desinstalado.", "Has recuperado 412 MB.", ""),
        (
            "programas-error",
            progs.to_vec(),
            2,
            0,
            2,
            "No se pudo abrir el desinstalador de Zoom Workplace.",
            "El archivo no existe: C:\\Users\\ana\\AppData\\Roaming\\Zoom\\uninstall\\Installer.exe. Puede que se borrara a mano.",
            "C:\\Users\\ana\\AppData\\Roaming\\Zoom",
        ),
    ] {
        data.set_subtitle(if banner == 1 { "85 instalados · 37,8 GB" } else { "86 instalados · 38,2 GB" }.into());
        data.set_rows(model(rows));
        data.set_selected(selected);
        data.set_phase(phase);
        data.set_banner(banner);
        data.set_banner_title(title.into());
        data.set_banner_text(text.into());
        data.set_banner_path(path.into());
        modal.set_open(nombre == "programas-confirmar");
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wcv2-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }
    modal.set_open(false);

    // Procesos, con los datos de `PROCS` de las maquetas: lista, confirmar y error.
    use wincleaner::ui::{ProcRow, ProcsData};
    let procs = [
        (0x8fb0c4, "C", "Google Chrome", "Google LLC", 14, "6 %", "1,8 GB", 0),
        (0xc4a7b4, "D", "Discord", "Discord Inc.", 6, "1 %", "612 MB", 0),
        (0x9ba6ad, "E", "Explorador de Windows", "Microsoft Windows", 1, "0 %", "148 MB", 1),
        (0xa9c2b8, "S", "Spotify", "Spotify AB", 5, "0 %", "402 MB", 0),
        (0x9ba6ad, "M", "Antimalware Service Executable", "Microsoft Windows", 1, "2 %", "220 MB", 1),
        (0xd6b38a, "u", "updater.exe", "Sin firma · se ejecuta desde Temp", 1, "0 %", "18 MB", 2),
        (0x8fb0c4, "S", "Steam", "Valve Corporation", 3, "0 %", "260 MB", 0),
    ]
    .map(|(c, l, n, s, k, cpu, mem, f): (u32, _, _, _, _, _, _, _)| ProcRow {
        name: n.into(),
        count: k,
        sub: s.into(),
        cpu: cpu.into(),
        mem: mem.into(),
        icon: Default::default(),
        letter: l.into(),
        tint: slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8),
        blocked: f == 1,
        review: f == 2,
    });
    let data = app.global::<ProcsData>();
    data.set_subtitle("128 procesos · CPU 14 % · Memoria 5,4 GB de 16 GB".into());
    data.set_rows(model(procs.to_vec()));
    data.set_denied_name("AsusCertService".into());
    modal.set_title("¿Terminar Discord?".into());
    modal.set_text("Se cerrarán sus 6 procesos. Lo que no hayas guardado en Discord se perderá.".into());
    modal.set_confirm_text("Terminar".into());
    app.set_section(4);
    for (nombre, selected, denied) in
        [("procesos", 1, false), ("procesos-confirmar", 1, false), ("procesos-error", -1, true)]
    {
        data.set_selected(selected);
        data.set_denied(denied);
        modal.set_open(nombre == "procesos-confirmar");
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wcv2-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }
    modal.set_open(false);

    // Arrancan con Windows, con los datos de `STARTS` de las maquetas.
    use wincleaner::ui::StartRow;
    let starts = [
        (0xc4a7b4, "D", "Discord", "Registro del usuario", true, false, false),
        (0xa9c2b8, "S", "Spotify", "Registro del usuario", true, false, false),
        (0x8fb0c4, "O", "Microsoft OneDrive", "Registro del usuario", true, false, false),
        (0x8fb0c4, "S", "Steam", "Registro del usuario", false, false, false),
        (0xd6b38a, "u", "updater.exe", "Registro del usuario · %TEMP%\\x7f2 · sin firma", true, false, true),
        (0xd68a8a, "A", "Adobe Acrobat Update", "Carpeta Inicio · todos los usuarios", true, true, false),
        (0x9ba6ad, "N", "NVIDIA App", "Registro del equipo", true, true, false),
    ]
    .map(|(c, l, n, w, on, adm, f): (u32, _, _, _, _, _, _)| StartRow {
        name: n.into(),
        r#where: w.into(),
        on,
        needs_admin: adm,
        review: f,
        icon: Default::default(),
        letter: l.into(),
        tint: slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8),
    });
    data.set_selected(-1);
    data.set_denied(false);
    data.set_starts(model(starts.to_vec()));
    data.set_tab(1);
    for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
        guarda(&format!("wcv2-procesos-inicio{suffix}"), &headless::render(app.window(), w, h), w, h)?;
    }
    data.set_tab(0);

    // Seguridad, con los datos de `CHECKS` y `seguridad*()` de las maquetas.
    use wincleaner::ui::{CheckRow, FindingRow, Nav, SecPhase, SecurityData};
    let data = app.global::<SecurityData>();
    let checks = |done: i32| {
        let names = [
            "Programas que arrancan con Windows",
            "Procesos en ejecución",
            "Servicios que no son de Microsoft",
            "Archivo hosts",
            "Proxy del sistema",
            "Copias falsas de procesos de Windows",
        ];
        let rows = names.iter().enumerate().map(|(i, n)| CheckRow { name: (*n).into(), state: if (i as i32) < done { 2 } else { 1 } });
        model(rows.collect::<Vec<_>>())
    };
    let finding = |level, title: &str, path: &str, why: &str, actions: &[&str]| FindingRow {
        level,
        title: title.into(),
        path: path.into(),
        why: why.into(),
        actions: model(actions.iter().map(|a| slint::SharedString::from(*a)).collect()),
    };
    data.set_findings(model(vec![
        finding(
            0,
            "Defender ha encontrado Trojan:Win32/Wacatac.B!ml",
            "C:\\Users\\ana\\Downloads\\juego_crack.exe",
            "Defender ya la ha puesto en cuarentena. Desde Seguridad de Windows puedes eliminarla o restaurarla.",
            &["Abrir Seguridad de Windows"],
        ),
        finding(
            1,
            "updater.exe arranca con Windows desde una carpeta temporal",
            "C:\\Users\\ana\\AppData\\Local\\Temp\\x7f2\\updater.exe · sin firma",
            "Los programas legítimos casi nunca arrancan desde Temp.",
            &["Desactivar inicio", "Terminar proceso", "Abrir ubicación", "Buscar en internet"],
        ),
        finding(
            2,
            "El archivo hosts tiene 2 entradas añadidas",
            "0.0.0.0 ads.ejemplo.com · 0.0.0.0 tracker.ejemplo.net",
            "Puede ser un bloqueador de anuncios que instalaste tú.",
            &["Ver el archivo"],
        ),
    ]));
    data.set_av_text("Firmas de hace 2 días".into());
    data.set_clean_text("Defender no ha detectado amenazas y las 6 comprobaciones están bien.".into());
    data.set_banner_text(
        "Una la ha encontrado Defender. Las otras son avisos de WinCleaner: puede que sean cosas que instalaste tú.".into(),
    );
    data.set_banner_bad(true);
    app.set_section(5);
    for (nombre, phase, av, header, done, badge) in [
        ("seguridad", SecPhase::Idle, 0, "", 0, 0),
        ("seguridad-analizando", SecPhase::Scanning, 0, "Análisis rápido en curso · 2 min 5 s", 4, 0),
        ("seguridad-limpio", SecPhase::Clean, 0, "Último análisis: hoy, 18:42 · rápido", 6, 0),
        ("seguridad-hallazgos", SecPhase::Findings, 0, "Último análisis: hoy, 18:42 · rápido", 6, 3),
        ("seguridad-sin-defender", SecPhase::Idle, 1, "", 0, 0),
        // WMI no responde: aviso neutro y, con MpCmdRun, los análisis siguen.
        ("seguridad-desconocido", SecPhase::Idle, 3, "", 0, 0),
        // Una comprobación no se pudo hacer: escudo ámbar, no verde.
        ("seguridad-limpio-incompleto", SecPhase::Clean, 0, "Último análisis: hoy, 18:42 · rápido", 6, 0),
    ] {
        let incompleto = nombre.ends_with("incompleto");
        data.set_incomplete(incompleto);
        if incompleto {
            data.set_clean_text(
                "Defender no ha detectado amenazas. 5 de 6 comprobaciones bien; no se pudo revisar: Archivo hosts.".into(),
            );
        }
        data.set_phase(phase);
        data.set_av(av);
        data.set_scan_anyway(av == 3);
        if av == 1 {
            data.set_av_text("Avast Free Antivirus".into());
        }
        data.set_header(header.into());
        data.set_checks(checks(done));
        data.set_defender_running(phase == SecPhase::Scanning);
        // En «limpio», el aviso solo sale si Defender ha fallado.
        data.set_banner_title(if badge > 0 { "Hay 3 cosas que revisar." } else { "" }.into());
        app.global::<Nav>().set_security_badge(badge);
        for (suffix, w, h) in [("", 1100, 720), ("-min", 880, 600)] {
            guarda(&format!("wcv2-{nombre}{suffix}"), &headless::render(app.window(), w, h), w, h)?;
        }
    }
    app.global::<Nav>().set_security_badge(0);
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
