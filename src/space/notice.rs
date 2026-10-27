//! Textos de los avisos de Espacio: por qué no se borra una fila y qué ha fallado al borrar.
//! Solo construye texto: no hace E/S ni toca Slint.

use super::Why;
use super::recycle::{Fail, Outcome, Reason};
use super::zones::Lock;
use crate::format::bytes;

/// Lo que hace un botón del aviso; la interfaz decide cómo.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Retry,
    Purge,
    Search,
    Games(Option<String>),
    Programs(Option<String>),
    Procs(Option<u32>),
    Enter,
    Explorer,
    Relaunch,
    Analyze(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub text: String,
    pub icon: &'static str,
    pub act: Act,
}

/// Fallos que comparten motivo.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub icon: &'static str,
    pub title: String,
    pub meta: String,
    pub names: String,
    pub button: Option<Button>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub title: String,
    pub text: String,
    /// `(etiqueta, valor)`: normalmente la ruta y el tamaño.
    pub lines: Vec<(String, String)>,
    /// Recuadro verde de lo que sí se ha borrado (vacío si nada).
    pub done: String,
    /// Encabezado de la lista de grupos (vacío si no hay).
    pub head: String,
    pub groups: Vec<Group>,
    pub left: Option<Button>,
    pub close: &'static str,
    pub main: Option<Button>,
}

/// Lo que `locked` sabe de la fila.
pub struct Ctx<'a> {
    /// Nombre que enseña la fila.
    pub name: &'a str,
    pub path: &'a str,
    pub size: Option<u64>,
    pub admin: bool,
    /// Se puede entrar en la fila (tiene contenido que enseñar).
    pub can_enter: bool,
    /// `(juego, lanzador)` si se conoce.
    pub game: Option<(&'a str, &'a str)>,
    pub xbox: bool,
    pub link_to: Option<&'a str>,
    /// Unidad del destino del enlace, si es otra unidad fija (`D:`).
    pub link_drive: Option<&'a str>,
}

impl Notice {
    fn new(title: impl Into<String>, text: impl Into<String>) -> Self {
        Notice {
            title: title.into(),
            text: text.into(),
            lines: vec![],
            done: String::new(),
            head: String::new(),
            groups: vec![],
            left: None,
            close: "Cerrar",
            main: None,
        }
    }

    fn main(mut self, text: impl Into<String>, icon: &'static str, act: Act) -> Self {
        self.main = Some(btn(text, icon, act));
        self
    }
}

fn btn(text: impl Into<String>, icon: &'static str, act: Act) -> Button {
    Button { text: text.into(), icon, act }
}

/// Raíz de «Archivos de programa» (`X:\Program Files` o `(x86)`), no una carpeta de dentro.
fn is_programs_root(path: &str) -> bool {
    let Some((parent, last)) = path.trim_end_matches('\\').rsplit_once('\\') else { return false };
    parent.ends_with(':') && (last.eq_ignore_ascii_case("Program Files") || last.eq_ignore_ascii_case("Program Files (x86)"))
}

/// Por qué una fila no se puede borrar desde Carpetas.
pub fn locked(why: Why, c: &Ctx) -> Notice {
    let (n, p) = (c.name, c.path);
    let enter = |title: String, text: &str, label: &str| {
        let nt = Notice::new(title, text);
        if c.can_enter { nt.main(format!("Entrar en {label}"), "folder", Act::Enter) } else { nt }
    };
    let mut nt = match why {
        Why::Zone(Lock::Game) => match c.game {
            Some((game, launcher)) => {
                let end = if c.xbox { "la Configuración de Windows".to_string() } else { launcher.to_string() };
                Notice::new(
                    format!("{game} es un juego de {launcher}"),
                    format!(
                        "Si lo borras desde aquí, {launcher} seguirá creyendo que está instalado y lo volverá a descargar \
                         o dará error al abrirlo. Para quitarlo, desinstálalo: en Juegos se abre {end} con el juego ya elegido."
                    ),
                )
            }
            None => Notice::new(
                format!("«{n}» es de un juego"),
                "Si la borras, el lanzador seguirá creyendo que está instalado. Desinstálalo desde Juegos.",
            ),
        }
        .main("Ir a Juegos", "game", Act::Games(Some(p.into()))),
        Why::Zone(Lock::HoldsGames) => Notice::new(
            format!("«{n}» tiene juegos dentro"),
            "Si la borras, los lanzadores seguirán creyendo que los juegos están instalados. \
             Desinstálalos desde Juegos y entra en la carpeta para borrar lo demás.",
        )
        .main("Ir a Juegos", "game", Act::Games(None)),
        Why::Zone(Lock::Programs) if is_programs_root(p) => Notice::new(
            format!("{n} no se puede borrar"),
            "Aquí se instalan los programas: borrarla los dejaría todos rotos. Para quitar uno, desinstálalo desde Programas.",
        )
        .main("Ir a Programas", "apps", Act::Programs(None)),
        Why::Zone(Lock::Programs) => Notice::new(
            format!("«{n}» es de un programa instalado"),
            "Borrar la carpeta de un programa lo deja roto y sin desinstalador. Para quitarlo, desinstálalo desde Programas.",
        )
        .main("Ir a Programas", "apps", Act::Programs(Some(p.into()))),
        Why::Zone(Lock::Known(l)) => enter(
            format!("{l} no se puede borrar"),
            "Es una carpeta de Windows y la usan el navegador y otros programas. Lo que hay dentro sí lo puedes borrar.",
            l,
        ),
        Why::Zone(Lock::Profile) => enter(
            format!("«{n}» no se puede borrar"),
            "Es tu carpeta de usuario y Windows la necesita para iniciar sesión. \
             Lo que hay dentro sí lo puedes borrar, salvo tus carpetas de Windows.",
            n,
        ),
        Why::Zone(Lock::HoldsKnown(l)) => enter(
            format!("«{n}» no se puede borrar"),
            &format!("Dentro está tu carpeta {l}, que Windows necesita. Entra y borra lo que no quieras."),
            n,
        ),
        Why::Zone(Lock::Windows) => Notice::new(
            format!("{n} no se puede borrar"),
            "Es parte del sistema: borrar algo de aquí puede dejar el equipo sin arrancar. \
             Para liberar espacio de Windows (actualizaciones viejas, temporales, volcados) usa «Limpiar».",
        ),
        Why::Zone(Lock::Users) => enter(
            format!("{n} no se puede borrar"),
            "Tiene las carpetas de todos los usuarios de este equipo. Entra en la tuya para borrar lo que no necesites.",
            n,
        ),
        Why::Zone(Lock::OtherUser) => Notice::new(
            "Es de otro usuario de este equipo",
            "WinCleaner no borra nada de las carpetas de otros usuarios. \
             Si es tuyo, inicia sesión con esa cuenta y bórralo desde allí.",
        ),
        Why::Zone(Lock::Bin) => Notice::new("Es la papelera", "Lo que hay aquí ya está en la papelera. Se vacía desde «Limpiar»."),
        Why::Zone(Lock::SystemFile) => {
            let low = n.to_lowercase();
            let text = if low == "pagefile.sys" || low == "swapfile.sys" {
                "Windows lo usa como memoria virtual y lo vuelve a crear. \
                 Para que ocupe menos, cambia la memoria virtual en la configuración de Windows."
            } else if low == "hiberfil.sys" {
                "Windows lo usa para hibernar y lo vuelve a crear. \
                 Si no hibernas el equipo, puedes desactivar la hibernación en la configuración de Windows."
            } else {
                "Windows lo necesita para arrancar o para guardar datos de errores."
            };
            Notice::new(format!("{n} es un archivo del sistema"), text)
        }
        Why::Zone(Lock::Me) => Notice::new("Es WinCleaner", "No se puede borrar mientras está abierto. Ciérralo y bórralo desde el Explorador."),
        Why::Zone(Lock::Drive | Lock::Unclear) | Why::Rest => Notice::new(
            "WinCleaner no puede comprobar esta ruta",
            "Windows puede leerla de varias maneras, así que, por seguridad, no se borra desde aquí. \
             Si quieres, bórrala desde el Explorador.",
        )
        .main("Abrir en el Explorador", "ext", Act::Explorer),
        Why::Link => {
            let mut text = match c.link_to {
                Some(to) => format!("«{n}» apunta a {to}. "),
                None => format!("«{n}» apunta a otra carpeta. "),
            };
            text.push_str("WinCleaner no borra a través de enlaces, para no borrar nada fuera de aquí.");
            let nt = match (c.link_to, c.link_drive) {
                (Some(_), Some(d)) => {
                    text.push_str(&format!(" Lo que hay en esa carpeta lo puedes ver y borrar analizando la unidad {d}."));
                    Notice::new("Es un enlace a otra carpeta", text).main(format!("Analizar {d}"), "drive", Act::Analyze(format!("{d}\\")))
                }
                _ => Notice::new("Es un enlace a otra carpeta", text),
            };
            return nt;
        }
        Why::Odd => {
            let mut nt = Notice::new(
                "El nombre tiene caracteres que WinCleaner no puede comprobar",
                "Para no equivocarse de archivo, no lo borra desde aquí. Puedes borrarlo desde el Explorador.",
            )
            .main("Abrir en el Explorador", "ext", Act::Explorer);
            nt.lines = vec![(n.into(), c.size.map(bytes).unwrap_or_default())];
            return nt;
        }
        Why::Denied if !c.admin => Notice::new(
            "Hacen falta permisos de administrador",
            "Windows no deja abrir esta carpeta sin permisos de administrador. \
             Reinicia WinCleaner como administrador y vuelve a intentarlo: tendrás que volver a analizar la unidad.",
        )
        .main("Reiniciar como administrador", "shield", Act::Relaunch),
        Why::Denied => Notice::new("Windows no deja abrirla", "Es de otro propietario o está protegida por el sistema."),
        Why::NoZones => Notice::new(
            "No se puede borrar nada desde aquí",
            "WinCleaner no ha podido saber qué carpetas de Windows tiene que proteger, así que, por seguridad, todo es solo para ver.",
        ),
    };
    nt.lines = vec![(p.into(), c.size.map(bytes).unwrap_or_default())];
    nt
}

/// Aviso de lo que no se ha podido borrar. Solo se llama si `!o.failed.is_empty() || o.missing`.
/// `alt`: se ofrece «Borrar del todo…»; `folder`: lo elegido es una carpeta.
pub fn failed(o: &Outcome, purge: bool, alt: bool, admin: bool, folder: bool) -> Notice {
    let name = o.names.first().map_or("", String::as_str);
    if o.failed.is_empty() {
        let title = match o.names.as_slice() {
            [one] => format!("«{one}» ya no está"),
            many => format!("Ya no estaban: {} elementos", many.len()),
        };
        let mut nt = Notice::new(title, "Se ha borrado o movido desde fuera de WinCleaner después del análisis. La lista ya está actualizada.");
        nt.close = "Aceptar";
        return nt;
    }
    let pid = o.users.first().map(|u| u.0);
    if o.failed.len() == 1 && !(purge && o.sent > 0) {
        let f = &o.failed[0];
        // Con varios elegidos, «el elemento» es el que ha fallado.
        let n = if o.names.len() == 1 { name } else { f.name.as_str() };
        let nt = match f.reason {
            Reason::InUse => {
                let who = o.users.first().map_or("Otro programa", |u| u.1.as_str());
                let what = if folder { "algo de esta carpeta" } else { "este archivo" };
                let mut nt = Notice::new(
                    "No se ha podido borrar: está en uso",
                    format!("{who} tiene abierto {what}. Ciérralo y vuelve a intentarlo. En Procesos puedes cerrarlo si no responde."),
                );
                nt.left = Some(btn("Ir a Procesos", "pulse", Act::Procs(pid)));
                nt.main = Some(btn("Reintentar", "refresh", Act::Retry));
                return nt;
            }
            Reason::Denied if !admin => Notice::new(
                "Hacen falta permisos de administrador",
                "Windows no deja borrarlo sin permisos de administrador. Reinicia WinCleaner como administrador y vuelve a intentarlo.",
            )
            .main("Reiniciar como administrador", "shield", Act::Relaunch),
            Reason::Denied => Notice::new("Windows no deja borrarlo", "Es de otro propietario o está protegido por el sistema."),
            Reason::NoBin => {
                let mut text = format!("«{n}» no cabe en la papelera o su unidad no tiene. No se ha borrado nada.");
                if alt {
                    Notice::new("No cabe en la papelera", text).main("Borrar del todo…", "trash", Act::Purge)
                } else {
                    text.push_str(" Si quieres liberar el espacio igualmente, búscalo en Carpetas y usa «Borrar del todo…».");
                    Notice::new("No cabe en la papelera", text)
                }
            }
            Reason::Protected => Notice::new(
                "No se ha borrado: está protegido",
                "Desde el análisis, la ruta ha cambiado (por ejemplo, por un enlace) y ahora cae en una carpeta protegida. \
                 Vuelve a analizar la unidad.",
            ),
            Reason::Link => Notice::new("No se ha borrado: es un enlace", "WinCleaner no borra a través de enlaces."),
            Reason::Changed => Notice::new(
                "Ha cambiado desde la búsqueda",
                "La copia ya no es igual a la que se conserva. Vuelve a buscar duplicados.",
            )
            .main("Buscar otra vez", "refresh", Act::Search),
            Reason::Purged => {
                let mut nt = Notice::new(
                    format!("Se ha borrado del todo «{n}»"),
                    "Windows lo ha borrado sin pasar por la papelera: no se puede deshacer.",
                );
                nt.close = "Aceptar";
                return nt;
            }
            Reason::Other(c) => Notice::new(
                format!("No se ha podido borrar «{n}»"),
                format!("Windows ha devuelto el error 0x{c:08X}. Vuelve a intentarlo."),
            )
            .main("Reintentar", "refresh", Act::Retry),
        };
        return nt;
    }

    // Varios fallos o borrado a medias.
    let count = match o.names.as_slice() {
        [one] => format!("«{one}»"),
        many => format!("{} elementos", many.len()),
    };
    let title = match (o.names.len() == 1, o.sent > 0) {
        (_, true) => format!("Se ha borrado parte de {count}"),
        (true, false) => format!("No se ha podido borrar {count}"),
        (false, false) => format!("No se han podido borrar {count}"),
    };
    let mut nt = Notice::new(title, "");
    if o.sent > 0 {
        nt.done = if purge { format!("Se han liberado {}", bytes(o.sent)) } else { format!("{} enviados a la papelera", bytes(o.sent)) };
    }
    let total: u64 = o.failed.iter().map(|f| f.size).sum();
    nt.head = match o.failed.len() {
        1 => format!("No se ha podido borrar 1 elemento ({}):", bytes(total)),
        k => format!("No se han podido borrar {k} elementos ({}):", bytes(total)),
    };
    // Un grupo por motivo, en este orden (el de `rank`) y con estos iconos.
    const ICONS: [&str; 8] = ["pulse", "shield", "trash", "link", "lock", "copy", "warn", "warn"];
    let rank = |r: &Reason| match r {
        Reason::InUse => 0,
        Reason::Denied => 1,
        Reason::NoBin => 2,
        Reason::Link => 3,
        Reason::Protected => 4,
        Reason::Changed => 5,
        Reason::Other(_) => 6,
        Reason::Purged => 7,
    };
    for (i, icon) in ICONS.into_iter().enumerate() {
        let fs: Vec<&Fail> = o.failed.iter().filter(|f| rank(&f.reason) == i).collect();
        if fs.is_empty() {
            continue;
        }
        let (title, button) = match i {
            0 => (
                match o.users.as_slice() {
                    [] => "En uso por otro programa".to_string(),
                    [a] => format!("En uso por {}", a.1),
                    [a, b] => format!("En uso por {} y {}", a.1, b.1),
                    [a, rest @ ..] => format!("En uso por {} y {} más", a.1, rest.len()),
                },
                Some(btn("Ir a Procesos", "pulse", Act::Procs(pid))),
            ),
            1 => ("Sin permisos".into(), (!admin).then(|| btn("Reiniciar como admin.", "shield", Act::Relaunch))),
            2 => ("No caben en la papelera".into(), None),
            3 => ("Enlaces".into(), None),
            4 => ("Protegidos".into(), None),
            5 => ("Han cambiado desde la búsqueda".into(), Some(btn("Buscar otra vez", "refresh", Act::Search))),
            6 => ("Otros errores".into(), None),
            _ => ("Borrados sin pasar por la papelera".into(), None),
        };
        let size: u64 = fs.iter().map(|f| f.size).sum();
        nt.groups.push(Group {
            icon,
            title,
            meta: format!("{} · {}", fs.len(), bytes(size)),
            names: fs.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "),
            button,
        });
    }
    let k = o.failed.iter().filter(|f| f.reason != Reason::Purged).count();
    if k > 0 {
        let text = if k == 1 { "Reintentar".to_string() } else { format!("Reintentar los {k}") };
        nt.main = Some(btn(text, "refresh", Act::Retry));
    }
    nt
}

/// Aviso de lo que no ha vuelto de la papelera al deshacer.
pub fn undo_failed(failed: &[String]) -> Notice {
    match failed {
        [one] => Notice::new(format!("No se ha podido restaurar «{one}»"), "Búscalo en la papelera."),
        many => Notice::new(
            format!("No se han podido restaurar {} elementos: {}.", many.len(), many.join(", ")),
            "Búscalos en la papelera.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn ctx<'a>(name: &'a str, path: &'a str) -> Ctx<'a> {
        Ctx { name, path, size: Some(118 * GB), admin: false, can_enter: true, game: None, xbox: false, link_to: None, link_drive: None }
    }

    fn zone(l: Lock, name: &str, path: &str) -> Notice {
        locked(Why::Zone(l), &ctx(name, path))
    }

    #[test]
    fn game_notice() {
        let c = Ctx { game: Some(("Baldur's Gate 3", "Steam")), ..ctx("Baldur's Gate 3", r"C:\Program Files (x86)\Steam\steamapps\common\Baldur's Gate 3") };
        let n = locked(Why::Zone(Lock::Game), &c);
        assert_eq!(n.title, "Baldur's Gate 3 es un juego de Steam");
        assert!(n.text.ends_with("en Juegos se abre Steam con el juego ya elegido."), "{}", n.text);
        assert_eq!(n.lines, [(c.path.to_string(), "118 GB".to_string())]);
        assert_eq!(n.main.unwrap().act, Act::Games(Some(c.path.into())));
        assert_eq!(n.close, "Cerrar");
        let n = locked(Why::Zone(Lock::Game), &Ctx { xbox: true, ..c });
        assert!(n.text.ends_with("en Juegos se abre la Configuración de Windows con el juego ya elegido."), "{}", n.text);
    }

    #[test]
    fn game_unknown() {
        let n = zone(Lock::Game, "Cosas", r"C:\Games\Cosas");
        assert_eq!(n.title, "«Cosas» es de un juego");
        assert_eq!(n.text, "Si la borras, el lanzador seguirá creyendo que está instalado. Desinstálalo desde Juegos.");
        let b = n.main.unwrap();
        assert_eq!((b.text.as_str(), b.icon, b.act), ("Ir a Juegos", "game", Act::Games(Some(r"C:\Games\Cosas".into()))));
    }

    #[test]
    fn holds_games() {
        let n = zone(Lock::HoldsGames, "Games", r"C:\Games");
        assert_eq!(n.title, "«Games» tiene juegos dentro");
        assert!(n.text.starts_with("Si la borras, los lanzadores seguirán creyendo"), "{}", n.text);
        assert_eq!(n.main.unwrap().act, Act::Games(None));
    }

    #[test]
    fn programs_inside_and_root() {
        let n = zone(Lock::Programs, "Foo", r"C:\Program Files\Foo");
        assert_eq!(n.title, "«Foo» es de un programa instalado");
        assert_eq!(n.main.unwrap().act, Act::Programs(Some(r"C:\Program Files\Foo".into())));
        let n = zone(Lock::Programs, "Program Files", r"C:\Program Files");
        assert_eq!(n.title, "Program Files no se puede borrar");
        assert!(n.text.starts_with("Aquí se instalan los programas"), "{}", n.text);
        assert_eq!(n.main.unwrap().act, Act::Programs(None));
        assert_eq!(zone(Lock::Programs, "x", r"D:\Program Files (x86)\").main.unwrap().act, Act::Programs(None));
    }

    #[test]
    fn known_offers_enter() {
        let n = locked(Why::Zone(Lock::Known("Descargas")), &ctx("Descargas", r"C:\Users\ana\Downloads"));
        assert_eq!(n.title, "Descargas no se puede borrar");
        assert!(n.text.ends_with("Lo que hay dentro sí lo puedes borrar."), "{}", n.text);
        let b = n.main.unwrap();
        assert_eq!((b.text.as_str(), b.act), ("Entrar en Descargas", Act::Enter));
        let c = Ctx { can_enter: false, ..ctx("Descargas", r"C:\Users\ana\Downloads") };
        assert!(locked(Why::Zone(Lock::Known("Descargas")), &c).main.is_none());
    }

    #[test]
    fn profile_and_holds_known_and_users() {
        let n = zone(Lock::Profile, "ana", r"C:\Users\ana");
        assert_eq!(n.title, "«ana» no se puede borrar");
        assert!(n.text.contains("salvo tus carpetas de Windows"), "{}", n.text);
        assert_eq!(n.main.unwrap().text, "Entrar en ana");
        let n = zone(Lock::HoldsKnown("Documentos"), "datos", r"D:\datos");
        assert_eq!(n.title, "«datos» no se puede borrar");
        assert_eq!(n.text, "Dentro está tu carpeta Documentos, que Windows necesita. Entra y borra lo que no quieras.");
        assert_eq!(n.main.unwrap().act, Act::Enter);
        let n = zone(Lock::Users, "Users", r"C:\Users");
        assert_eq!(n.title, "Users no se puede borrar");
        assert!(n.text.starts_with("Tiene las carpetas de todos los usuarios"), "{}", n.text);
        assert_eq!(n.main.unwrap().text, "Entrar en Users");
    }

    #[test]
    fn windows_has_only_close() {
        let n = locked(Why::Zone(Lock::Windows), &ctx("Windows", r"C:\Windows"));
        assert!(n.main.is_none() && n.left.is_none());
        assert_eq!(n.title, "Windows no se puede borrar");
        assert!(n.text.contains("usa «Limpiar»"));
    }

    #[test]
    fn other_user_and_bin_and_me() {
        let n = zone(Lock::OtherUser, "luis", r"C:\Users\luis");
        assert_eq!(n.title, "Es de otro usuario de este equipo");
        assert!(n.main.is_none());
        let n = zone(Lock::Bin, "x", r"C:\$Recycle.Bin");
        assert_eq!((n.title.as_str(), n.text.as_str()), ("Es la papelera", "Lo que hay aquí ya está en la papelera. Se vacía desde «Limpiar»."));
        let n = zone(Lock::Me, "wincleaner.exe", r"C:\x\wincleaner.exe");
        assert_eq!(n.title, "Es WinCleaner");
        assert!(n.text.ends_with("Ciérralo y bórralo desde el Explorador.") && n.main.is_none());
    }

    #[test]
    fn system_files() {
        let n = zone(Lock::SystemFile, "pagefile.sys", r"C:\pagefile.sys");
        assert_eq!(n.title, "pagefile.sys es un archivo del sistema");
        assert!(n.text.starts_with("Windows lo usa como memoria virtual"), "{}", n.text);
        assert!(zone(Lock::SystemFile, "SwapFile.sys", r"C:\swapfile.sys").text.contains("memoria virtual"));
        let n = zone(Lock::SystemFile, "hiberfil.sys", r"C:\hiberfil.sys");
        assert!(n.text.contains("para hibernar") && n.text.contains("desactivar la hibernación"), "{}", n.text);
        let n = zone(Lock::SystemFile, "DumpStack.log", r"C:\DumpStack.log");
        assert_eq!(n.text, "Windows lo necesita para arrancar o para guardar datos de errores.");
        assert!(n.main.is_none());
    }

    #[test]
    fn unclear_and_drive_offer_explorer() {
        for l in [Lock::Unclear, Lock::Drive] {
            let n = zone(l, "x", r"C:\x");
            assert_eq!(n.title, "WinCleaner no puede comprobar esta ruta");
            assert!(n.text.ends_with("bórrala desde el Explorador."), "{}", n.text);
            let b = n.main.unwrap();
            assert_eq!((b.text.as_str(), b.icon, b.act), ("Abrir en el Explorador", "ext", Act::Explorer));
        }
        assert_eq!(locked(Why::Rest, &ctx("x", r"C:\x")).title, "WinCleaner no puede comprobar esta ruta");
    }

    #[test]
    fn link_to_other_drive() {
        let c = Ctx { link_to: Some(r"D:\Documentos"), link_drive: Some("D:"), ..ctx("Documentos", r"C:\Users\ana\Documentos") };
        let n = locked(Why::Link, &c);
        assert_eq!(n.title, "Es un enlace a otra carpeta");
        assert!(n.text.contains(r"apunta a D:\Documentos") && n.text.ends_with("analizando la unidad D:."), "{}", n.text);
        let b = n.main.unwrap();
        assert_eq!((b.text.as_str(), b.act), ("Analizar D:", Act::Analyze(r"D:\".into())));
        assert!(n.lines.is_empty());
    }

    #[test]
    fn link_elsewhere_and_unreadable() {
        let c = Ctx { link_to: Some(r"C:\Otra"), ..ctx("Enlace", r"C:\Enlace") };
        let n = locked(Why::Link, &c);
        assert!(n.text.starts_with("«Enlace» apunta a C:\\Otra. WinCleaner no borra a través de enlaces"), "{}", n.text);
        assert!(n.text.ends_with("para no borrar nada fuera de aquí.") && n.main.is_none() && n.lines.is_empty(), "{}", n.text);
        let n = locked(Why::Link, &ctx("Enlace", r"C:\Enlace"));
        assert!(n.text.starts_with("«Enlace» apunta a otra carpeta."), "{}", n.text);
    }

    #[test]
    fn odd_name() {
        let n = locked(Why::Odd, &ctx("rar\u{fffd}o", r"C:\rar\u{fffd}o"));
        assert_eq!(n.title, "El nombre tiene caracteres que WinCleaner no puede comprobar");
        assert_eq!(n.text, "Para no equivocarse de archivo, no lo borra desde aquí. Puedes borrarlo desde el Explorador.");
        assert_eq!(n.lines[0].0, "rar\u{fffd}o");
        assert_eq!(n.main.unwrap().act, Act::Explorer);
    }

    #[test]
    fn denied_depends_on_admin() {
        let c = ctx("Juegos viejos", r"C:\Juegos viejos");
        let n = locked(Why::Denied, &c);
        assert_eq!(n.title, "Hacen falta permisos de administrador");
        let b = n.main.unwrap();
        assert_eq!((b.text.as_str(), b.icon, b.act), ("Reiniciar como administrador", "shield", Act::Relaunch));
        let n = locked(Why::Denied, &Ctx { admin: true, ..c });
        assert_eq!((n.title.as_str(), n.main), ("Windows no deja abrirla", None));
        assert_eq!(n.text, "Es de otro propietario o está protegida por el sistema.");
    }

    #[test]
    fn no_zones() {
        let n = locked(Why::NoZones, &ctx("x", r"C:\x"));
        assert_eq!(n.title, "No se puede borrar nada desde aquí");
        assert!(n.text.ends_with("todo es solo para ver.") && n.main.is_none());
    }

    fn fail(n: &str, size: u64, reason: Reason) -> Fail {
        Fail { name: n.into(), size, reason }
    }

    fn one(reason: Reason) -> Notice {
        let o = Outcome { names: vec!["a.mkv".into()], left: GB, failed: vec![fail("a.mkv", GB, reason)], ..Default::default() };
        failed(&o, false, true, false, false)
    }

    #[test]
    fn in_use_single() {
        let o = Outcome { names: vec!["directo.mkv".into()], left: 6 * GB,
            failed: vec![fail("directo.mkv", 6 * GB, Reason::InUse)],
            users: vec![(4242, "OBS Studio".into())], ..Default::default() };
        let n = failed(&o, false, true, false, false);
        assert_eq!(n.title, "No se ha podido borrar: está en uso");
        assert!(n.text.starts_with("OBS Studio tiene abierto este archivo."));
        assert_eq!(n.left.unwrap().act, Act::Procs(Some(4242)));
        let m = n.main.unwrap();
        assert_eq!((m.text.as_str(), m.act), ("Reintentar", Act::Retry));
        let o = Outcome { users: vec![], ..o };
        assert!(failed(&o, false, true, false, false).text.starts_with("Otro programa tiene abierto"));
        assert!(failed(&o, false, true, false, true).text.starts_with("Otro programa tiene abierto algo de esta carpeta."));
    }

    #[test]
    fn denied_single() {
        let n = one(Reason::Denied);
        assert_eq!(n.title, "Hacen falta permisos de administrador");
        assert_eq!(n.main.unwrap().act, Act::Relaunch);
        let o = Outcome { names: vec!["a".into()], failed: vec![fail("a", 1, Reason::Denied)], ..Default::default() };
        let n = failed(&o, false, true, true, false);
        assert_eq!((n.title.as_str(), n.main), ("Windows no deja borrarlo", None));
    }

    #[test]
    fn no_bin_offers_purge_only_with_alt() {
        let o = Outcome { names: vec!["juego.iso".into()], left: 40 * GB,
            failed: vec![fail("juego.iso", 40 * GB, Reason::NoBin)], ..Default::default() };
        let n = failed(&o, false, true, false, false);
        assert_eq!(n.title, "No cabe en la papelera");
        assert_eq!(n.text, "«juego.iso» no cabe en la papelera o su unidad no tiene. No se ha borrado nada.");
        assert_eq!(n.main.unwrap().act, Act::Purge);
        let n = failed(&o, false, false, false, false);
        assert!(n.main.is_none());
        assert!(n.text.ends_with("búscalo en Carpetas y usa «Borrar del todo…»."), "{}", n.text);
    }

    #[test]
    fn protected_and_link_single() {
        let n = one(Reason::Protected);
        assert_eq!(n.title, "No se ha borrado: está protegido");
        assert!(n.text.ends_with("Vuelve a analizar la unidad.") && n.main.is_none());
        let n = one(Reason::Link);
        assert_eq!((n.title.as_str(), n.text.as_str()), ("No se ha borrado: es un enlace", "WinCleaner no borra a través de enlaces."));
    }

    #[test]
    fn changed_offers_search() {
        let n = one(Reason::Changed);
        assert_eq!(n.title, "Ha cambiado desde la búsqueda");
        assert_eq!(n.text, "La copia ya no es igual a la que se conserva. Vuelve a buscar duplicados.");
        let b = n.main.unwrap();
        assert_eq!((b.text.as_str(), b.icon, b.act), ("Buscar otra vez", "refresh", Act::Search));
    }

    #[test]
    fn purged_only_accept() {
        let n = one(Reason::Purged);
        assert_eq!(n.title, "Se ha borrado del todo «a.mkv»");
        assert_eq!(n.text, "Windows lo ha borrado sin pasar por la papelera: no se puede deshacer.");
        assert_eq!((n.close, n.main), ("Aceptar", None));
    }

    #[test]
    fn other_shows_code() {
        let n = one(Reason::Other(0x80070057_u32 as i32));
        assert_eq!(n.title, "No se ha podido borrar «a.mkv»");
        assert_eq!(n.text, "Windows ha devuelto el error 0x80070057. Vuelve a intentarlo.");
        assert_eq!(n.main.unwrap().act, Act::Retry);
    }

    #[test]
    fn purge_partial_groups() {
        let o = Outcome { names: vec!["OBS".into()], sent: 38 * GB, left: 3 * GB,
            failed: vec![fail("a.mkv", GB, Reason::InUse), fail("b.mkv", GB, Reason::InUse), fail(r"2026-08\cache", GB, Reason::Denied)],
            users: vec![(1, "OBS Studio".into())], ..Default::default() };
        let n = failed(&o, true, true, false, true);
        assert_eq!(n.title, "Se ha borrado parte de «OBS»");
        assert_eq!(n.done, "Se han liberado 38 GB");
        assert_eq!(n.head, "No se han podido borrar 3 elementos (3 GB):");
        assert_eq!(n.groups.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(), ["En uso por OBS Studio", "Sin permisos"]);
        assert_eq!(n.groups[0].meta, "2 · 2 GB");
        assert_eq!(n.groups[0].names, "a.mkv, b.mkv");
        assert_eq!(n.groups[0].icon, "pulse");
        assert_eq!(n.groups[0].button.as_ref().unwrap().act, Act::Procs(Some(1)));
        assert_eq!(n.groups[1].button.as_ref().unwrap().act, Act::Relaunch);
        assert_eq!(n.main.unwrap().text, "Reintentar los 3");
        assert_eq!(n.close, "Cerrar");
    }

    #[test]
    fn many_groups_order_and_titles() {
        let o = Outcome {
            names: vec!["a".into(), "b".into(), "c".into()],
            sent: GB,
            left: 8,
            failed: vec![
                fail("p", 1, Reason::Purged), fail("o", 1, Reason::Other(1)), fail("c", 1, Reason::Changed),
                fail("x", 1, Reason::Protected), fail("l", 1, Reason::Link), fail("n", 1, Reason::NoBin),
                fail("d", 1, Reason::Denied), fail("u", 1, Reason::InUse),
            ],
            users: vec![(7, "A".into()), (8, "B".into()), (9, "C".into())],
            ..Default::default()
        };
        let n = failed(&o, false, true, true, false);
        assert_eq!(n.title, "Se ha borrado parte de 3 elementos");
        assert_eq!(n.done, "1 GB enviados a la papelera");
        let t: Vec<_> = n.groups.iter().map(|g| (g.title.as_str(), g.icon)).collect();
        assert_eq!(t, [
            ("En uso por A y 2 más", "pulse"), ("Sin permisos", "shield"), ("No caben en la papelera", "trash"),
            ("Enlaces", "link"), ("Protegidos", "lock"), ("Han cambiado desde la búsqueda", "copy"),
            ("Otros errores", "warn"), ("Borrados sin pasar por la papelera", "warn"),
        ]);
        assert!(n.groups[1].button.is_none(), "siendo admin no hay botón");
        assert_eq!(n.groups[5].button.as_ref().unwrap().act, Act::Search);
        assert_eq!(n.main.unwrap().text, "Reintentar los 7");
        let one = Outcome { users: vec![(7, "A".into()), (8, "B".into())], failed: vec![fail("u", 1, Reason::InUse), fail("v", 1, Reason::Link)], ..Default::default() };
        assert_eq!(failed(&one, false, true, false, false).groups[0].title, "En uso por A y B");
        let none = Outcome { users: vec![], ..one };
        assert_eq!(failed(&none, false, true, false, false).groups[0].title, "En uso por otro programa");
    }

    #[test]
    fn many_without_sent() {
        let o = Outcome { names: vec!["a".into(), "b".into()], failed: vec![fail("a", 1, Reason::Link), fail("b", 2, Reason::Link)], ..Default::default() };
        let n = failed(&o, false, true, false, false);
        assert_eq!(n.title, "No se han podido borrar 2 elementos");
        assert_eq!(n.done, "");
        let o = Outcome { names: vec!["a".into()], failed: vec![fail("a", 1, Reason::Link), fail("b", 2, Reason::Link)], ..Default::default() };
        assert_eq!(failed(&o, false, true, false, false).title, "No se ha podido borrar «a»");
        let o = Outcome { names: vec!["a".into()], sent: 5, failed: vec![fail("a", 1, Reason::Other(3))], ..Default::default() };
        let n = failed(&o, true, true, false, false);
        assert_eq!(n.head, "No se ha podido borrar 1 elemento (1 B):");
        assert_eq!(n.main.unwrap().text, "Reintentar");
        let o = Outcome { names: vec!["a".into()], sent: 5, failed: vec![fail("a", 1, Reason::Purged), fail("b", 1, Reason::Purged)], ..Default::default() };
        assert!(failed(&o, true, true, false, false).main.is_none());
    }

    #[test]
    fn missing_is_accept() {
        let o = Outcome { names: vec!["partida.mkv".into()], missing: true, ..Default::default() };
        let n = failed(&o, false, true, false, false);
        assert_eq!((n.title.as_str(), n.close, n.main), ("«partida.mkv» ya no está", "Aceptar", None));
        assert!(n.text.ends_with("La lista ya está actualizada."));
        let o = Outcome { names: vec!["a".into(), "b".into()], missing: true, ..Default::default() };
        assert_eq!(failed(&o, false, true, false, false).title, "Ya no estaban: 2 elementos");
    }

    #[test]
    fn undo_failed_texts() {
        let n = undo_failed(&["a.mov".to_string()]);
        assert_eq!((n.title.as_str(), n.text.as_str()), ("No se ha podido restaurar «a.mov»", "Búscalo en la papelera."));
        let n = undo_failed(&["a".to_string(), "b".to_string()]);
        assert_eq!((n.title.as_str(), n.text.as_str()), ("No se han podido restaurar 2 elementos: a, b.", "Búscalos en la papelera."));
        assert_eq!(n.close, "Cerrar");
    }
}
