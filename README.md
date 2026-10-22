<div align="center">

# 🧹 WinCleaner

**Limpieza y puesta a punto de Windows, sin complicaciones.**

![Estado](https://img.shields.io/badge/estado-v1%20sin%20probar%20en%20Windows-orange)
![Plataforma](https://img.shields.io/badge/plataforma-Windows%2010-0078D6?logo=windows&logoColor=white)
![Licencia](https://img.shields.io/badge/licencia-MIT-green)

</div>

---

## ✨ Qué es

WinCleaner es una herramienta para dejar Windows limpio y ligero: muestra dónde se va el espacio del disco y libera el que ocupan los archivos temporales y las cachés. Es un único `.exe` portable de unos 0,5 MB, sin instalación, sin telemetría y con la interfaz en español.

> 🚧 **Estado.** La v1 está compilada desde Linux y pendiente de su primera prueba en un Windows real.

## 🔍 Funciones

**Análisis.** Elige una unidad y WinCleaner lista las 50 carpetas y los 50 archivos más grandes, con un botón para abrir su ubicación en el Explorador. Se puede detener. Las carpetas se muestran sin repetir la cadena de antecesoras, para que se vea dónde se concentra el espacio.

**Limpieza.** Nueve categorías, cada una con su casilla y su tamaño (bytes y archivos) antes de borrar:

| Categoría | Qué se limpia | Antigüedad | Admin |
|---|---|---|---|
| Temporales del usuario | `%TEMP%` | sin modificar en más de 24 h | no |
| Temporales de Windows | `%SystemRoot%\Temp` | sin modificar en más de 24 h | sí |
| Papelera de reciclaje | Todas las unidades, con la API del sistema | se vacía entera | no |
| Caché de miniaturas | `thumbcache_*.db` en `%LOCALAPPDATA%\Microsoft\Windows\Explorer` | todos | no |
| Caché de Chrome | `Cache`, `Code Cache` y `GPUCache` de cada perfil (`Default`, `Profile N`) en `%LOCALAPPDATA%\Google\Chrome\User Data` | todo | no |
| Caché de Edge | Lo mismo, en `%LOCALAPPDATA%\Microsoft\Edge\User Data` | todo | no |
| Caché de Firefox | `cache2` de cada perfil en `%LOCALAPPDATA%\Mozilla\Firefox\Profiles` | todo | no |
| Volcados de errores y logs antiguos | `%LOCALAPPDATA%\CrashDumps` y `%LOCALAPPDATA%\Microsoft\Windows\WER\{ReportArchive,ReportQueue}`. Con administrador, también `%ProgramData%\Microsoft\Windows\WER\{ReportArchive,ReportQueue}`, `%SystemRoot%\Minidump` y `%SystemRoot%\Logs\CBS` | sin modificar en más de 30 días | no (las rutas de sistema, sí) |
| Caché de Windows Update | `%SystemRoot%\SoftwareDistribution\Download` | todo | sí |

Solo se borra el contenido de esas carpetas, nunca la carpeta raíz. Las cookies, el historial y las contraseñas de los navegadores no se tocan porque no están en ninguna de esas rutas. Si una carpeta no existe, la categoría sale como «no encontrado» y queda desactivada.

## 🛡️ Seguridad

- **Lista blanca.** Un archivo solo se borra si su ruta está estrictamente dentro de una de las carpetas de la tabla. No se aceptan `..`, rutas relativas, `\\?\`, rutas de red ni flujos alternativos.
- **Carpetas protegidas.** Nunca se limpia una carpeta igual o antecesora de la raíz de una unidad, tu perfil, Documentos, Escritorio, Descargas, Imágenes, Música, Vídeos (con su ruta real, aunque estén en OneDrive), `Windows`, `System32` o `Program Files`. Si `%TEMP%` apunta a un sitio raro, esa categoría sale como «ruta no reconocida». Si Windows no puede indicar cuáles son las carpetas protegidas, la app se niega a limpiar nada salvo la papelera.
- **Enlaces.** Los enlaces simbólicos y los junctions no se siguen ni se borran, tampoco las carpetas intermedias, que se vuelven a comprobar justo antes de cada borrado.
- **Archivos en uso.** Los bloqueados o sin permiso se saltan sin errores y se cuentan en el informe.
- **Confirmación.** Nada se borra sin pasar por una ventana que lista las categorías marcadas y el total. Es el cuadro de mensaje del sistema con «Cancelar» como botón por defecto, así que pulsar Intro no borra por accidente. Mientras se limpia, la ventana no se puede cerrar.
- **Informe.** Al terminar, lo liberado por categoría y los archivos saltados.

## 🚀 Uso

1. Descarga `wincleaner.exe` (o [compílalo](#-compilar)).
2. Ejecútalo, sin instalar nada. Arranca sin permisos de administrador.
3. Las categorías que lo necesitan (Temporales de Windows y Windows Update) salen desactivadas. Pulsa «Reiniciar como administrador» para activarlas.

Notas:

- Si elevas con las credenciales de otra cuenta, se limpian el perfil y la papelera de esa cuenta, no los tuyos.
- Explorer suele tener bloqueados los archivos de miniaturas, así que esa categoría puede liberar poco o nada. No es un fallo.
- Con el navegador abierto, sus archivos en uso se saltan.

## 🔨 Compilar

En Linux (compilación cruzada), necesitas `rustup target add x86_64-pc-windows-gnu` y mingw-w64 (`x86_64-w64-mingw32-gcc` y `windres`):

```sh
cargo test
cargo build --release --target x86_64-pc-windows-gnu
```

El resultado queda en `target/x86_64-pc-windows-gnu/release/wincleaner.exe`.

En Windows basta con `cargo build --release`.

## 🗺️ Hoja de ruta

- [x] v1: análisis del top 50 y limpieza de 9 categorías
- [ ] Primera prueba en un Windows real

No están previstos: duplicados, desinstalar programas, registro, programas de arranque, servicios, limpiezas programadas, actualizaciones automáticas ni elegir carpetas a mano para borrar.

## 📄 Licencia

[MIT](LICENSE)

## 👤 Autor

Hecho por [@9n-dev](https://github.com/9n-dev).
