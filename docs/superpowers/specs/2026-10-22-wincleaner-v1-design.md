# WinCleaner v1: diseño

Fecha: 2026-10-22 · Estado: pendiente de aprobación

## Intención

WinCleaner es una app ultra ligera para Windows 10 x64 que dice dónde se va el espacio del disco y libera lo que sobra sin riesgo. Es un único `.exe` portable: sin instalador, sin telemetría y sin servicios. La v1 la construye un agente durante la noche, sin nadie a quien preguntar, y la prueba Manuel en Windows al día siguiente. Por eso todo lo que se pueda verificar sin Windows tiene que estar en lógica pura y cubierto por `cargo test` en Linux.

**Anti-objetivo principal:** borrar algo que no sea basura conocida. Un cleaner que libere mucho espacio pero pueda tocar un documento del usuario es un fracaso, aunque funcione.

## Objetivos

1. **Análisis**
   - Espacio libre y usado de cada unidad fija.
   - Las 50 carpetas y los 50 archivos más grandes de la unidad elegida, con un botón para abrir su ubicación en el Explorador.
2. **Limpieza por categorías**
   - Cada categoría tiene su casilla y muestra cuánto ocupa (bytes y número de archivos) antes de borrar.
   - Antes de borrar, siempre hay vista previa y confirmación. Al terminar, un informe.
3. **Rendimiento:** arranque instantáneo y `.exe` de menos de 3 MB.
4. **Interfaz en español.** El nombre de la app es **WinCleaner** (título de la ventana, `wincleaner.exe` y README).

## Fuera de la v1 (no se construye)

Duplicados, desinstalar programas, registro, programas de arranque, servicios, limpiezas programadas, actualizaciones automáticas, telemetría y elegir carpetas a mano para borrar.

## Restricciones (decididas por Manuel)

- Rust con interfaz Win32 nativa, compilado desde Linux con `cargo build --release --target x86_64-pc-windows-gnu`. Sin avisos de compilación.
- Aquí no hay Windows ni Wine. La lógica va en un módulo que compila y se prueba en Linux con `cargo test`, y las pruebas de seguridad de rutas son obligatorias.
- La app arranca sin administrador. Las categorías que lo necesitan (Temporales de Windows y caché de Windows Update) salen desactivadas, con un botón «Reiniciar como administrador».
- Trabajo en la rama `feat/v1`, con push y PR al final. No se hace merge.
- Nunca se usa `sudo`. Si falta una herramienta, se apunta en Pendientes.
- **Terminado** quiere decir que:
  - `cargo test` pasa;
  - el `.exe` compila para Windows sin avisos;
  - el README explica cómo compilar y usar la app;
  - el PR está abierto.

## Interfaz (maqueta aprobada)

La interfaz es una sola ventana con controles nativos de Windows 10 (estilos visuales mediante manifiesto) y sin colores propios.

- **Arriba, siempre visibles:** las unidades fijas, cada una con una barra de uso y el texto «21,4 GB libres de 237 GB». La barra se pone roja si queda menos del 10 % libre.
- **Pestaña «Limpieza»** (abierta al arrancar):
  - Una lista de categorías con casilla, nombre, tamaño y número de archivos.
  - Los tamaños se calculan en segundo plano nada más abrir la app. Mientras tanto se ve «Calculando…».
  - Las categorías de administrador salen en gris, con «requiere admin.», si la app no está elevada.
  - Abajo, el botón «🛡 Reiniciar como administrador» (oculto si ya es administrador) y «Revisar y limpiar (X GB)…», con el total de las categorías marcadas.
- **Pestaña «Análisis»:**
  - Un selector de unidad, las opciones «Carpetas» y «Archivos», y el botón «Analizar», que pasa a «Detener» durante el análisis.
  - Una lista con #, ruta y tamaño. Un doble clic o el botón «Abrir ubicación» abren el Explorador con el elemento seleccionado (`explorer.exe /select,"ruta"`).
  - Debajo, «Analizados N archivos en S s».
  - El análisis **solo** arranca al pulsar «Analizar».
- **Barra de estado:** muestra lo que está pasando.
- **Flujo al limpiar:**
  1. **Confirmación:** lista de las categorías marcadas con su tamaño, el total, «Los archivos en uso se saltarán. Esto no se puede deshacer.» y los botones Cancelar y Borrar.
  2. **Borrado** en segundo plano, con la ventana bloqueada y el progreso en la barra de estado.
  3. **Informe:** «Liberados X», lo liberado en cada categoría y los archivos saltados (en uso o sin permiso) con su número y tamaño.
  4. Después se recalculan los tamaños y el espacio de las unidades.

## Categorías (rutas exactas)

Cada categoría tiene una o varias **raíces** y un **filtro**. Solo se borra el contenido de una raíz, nunca la raíz en sí.

| Categoría | Raíces | Filtro | Admin |
|---|---|---|---|
| Temporales del usuario | `%TEMP%` | sin modificar en más de 24 h | no |
| Temporales de Windows | `%SystemRoot%\Temp` | sin modificar en más de 24 h | sí |
| Papelera de reciclaje | (API del sistema, todas las unidades) | — | no |
| Caché de miniaturas | `%LOCALAPPDATA%\Microsoft\Windows\Explorer` | solo `thumbcache_*.db` del primer nivel | no |
| Caché de Chrome | `%LOCALAPPDATA%\Google\Chrome\User Data\<perfil>\{Cache, Code Cache, GPUCache}` | todo | no |
| Caché de Edge | `%LOCALAPPDATA%\Microsoft\Edge\User Data\<perfil>\{Cache, Code Cache, GPUCache}` | todo | no |
| Caché de Firefox | `%LOCALAPPDATA%\Mozilla\Firefox\Profiles\<perfil>\cache2` | todo | no |
| Volcados de errores y logs antiguos | `%LOCALAPPDATA%\CrashDumps` y `%LOCALAPPDATA%\Microsoft\Windows\WER\{ReportArchive, ReportQueue}`. Si la app es administrador, también `%ProgramData%\Microsoft\Windows\WER\{ReportArchive, ReportQueue}`, `%SystemRoot%\Minidump` y `%SystemRoot%\Logs\CBS` | sin modificar en más de 30 días | no (las raíces de sistema solo se añaden con admin) |
| Caché de Windows Update | `%SystemRoot%\SoftwareDistribution\Download` | todo | sí |

- **Perfiles de Chrome y Edge:** las carpetas `Default` y `Profile *` dentro de `User Data`.
- **Perfiles de Firefox:** cualquier subcarpeta de `Profiles`.
- Las cookies, el historial, las contraseñas y el resto de datos del navegador no se tocan nunca, porque no están en ninguna raíz.
- Si una raíz no existe, la categoría muestra «—» con «no encontrado» y se desactiva.
- Si el navegador está abierto, sus archivos en uso simplemente se saltan.

Los umbrales de antigüedad (24 h y 30 días) los decidió Manuel.

## Seguridad (innegociable)

Toda la seguridad se implementa en la lógica pura y se prueba en Linux.

1. **Lista blanca de raíces.** Un archivo solo se borra si su ruta está **estrictamente dentro** de una raíz de la categoría. La comprobación:
   - es una función pura sobre texto, con semántica de rutas de Windows;
   - acepta `\` y `/` y no distingue mayúsculas de minúsculas;
   - rechaza cualquier componente `..` o `.`, las rutas relativas, `\\?\`, las rutas UNC y cualquier `:` fuera de la letra de unidad (flujos alternativos);
   - no se deja engañar por prefijos: `Temp2` no está dentro de `Temp`.
2. **Validación de raíces al arrancar.** Una raíz se descarta (y su categoría queda como «ruta no reconocida») en estos casos:
   - es igual o antecesora de una carpeta protegida: la raíz de una unidad, `%USERPROFILE%`, Documentos, Escritorio, Descargas, Imágenes, Música, Vídeos (con sus rutas reales, aunque estén redirigidas, por ejemplo a OneDrive), `%SystemRoot%`, `%SystemRoot%\System32`, `Program Files` o `Program Files (x86)`;
   - en el caso de `%TEMP%`, su último componente no es `Temp` ni `Tmp`. Como excepción, se acepta un último componente solo numérico (`1`, `2`…) si el anterior es `Temp` o `Tmp`, que es el caso de las sesiones de Escritorio remoto (decisión del agente).
   Así se protege contra un `TEMP` mal configurado que apunte, por ejemplo, a `C:\Users\Manuel`.
3. **No se siguen enlaces.** Los enlaces simbólicos, los junctions y cualquier punto de reanálisis se ignoran del todo: no se entra en ellos ni se borran. Esto vale para la limpieza y para el análisis, donde además evita contar dos veces.
4. **Comprobación antes de borrar.** Justo antes de borrar cada archivo se vuelven a comprobar la lista blanca, que no sea un enlace, que sea un archivo y su antigüedad.
5. **Archivos de solo lectura.** Si el archivo está dentro de una raíz permitida, se le quita el atributo de solo lectura y se borra.
6. **Errores.** Cualquier error al borrar (archivo en uso, sin permiso…) se cuenta como «saltado» y no detiene nada ni muestra errores.
7. **Carpetas vacías.** Después de borrar se eliminan, de abajo arriba, las carpetas que se han quedado vacías y cuya fecha de modificación, tomada al escanear, supera el umbral de la categoría. La raíz no se borra nunca.
8. **Papelera.** Se consulta con `SHQueryRecycleBinW` y se vacía con `SHEmptyRecycleBinW` (sin confirmación del sistema, porque ya la pide la app). No pasa por la lista blanca porque no borra archivos sueltos.
9. **Confirmación obligatoria.** No hay ningún camino en el código que borre sin la ventana de confirmación.

### Detalles decididos por el agente

- **Antigüedad.** Se usa la fecha más reciente entre la creación y la modificación del archivo, que es lo más prudente.
- **Rutas largas.** La capa de Windows convierte las raíces a ruta larga (`GetLongPathNameW`, para deshacer los nombres cortos 8.3 como `MANUEL~1`) antes de pasarlas a la comprobación pura.
- **Administrador con otra cuenta.** Si se eleva con las credenciales de otra cuenta, la instancia elevada limpia el perfil y la Papelera de esa cuenta. Se acepta así en la v1 y se documenta en el README.
- **Windows Update.** No se detiene el servicio `wuauserv`: los archivos bloqueados se saltan.
- **Miniaturas.** Explorer suele bloquear los `thumbcache_*.db`, así que esa categoría puede liberar poco o nada. No es un fallo y se documenta.

## Análisis del top 50

- Se recorre la unidad entera sin seguir enlaces, saltando sin avisar las carpetas sin permiso. Se puede cancelar.
- **Archivos:** los 50 más grandes por tamaño.
- **Carpetas** (decisión del agente): primero se calcula el tamaño recursivo de cada carpeta y se ordenan de mayor a menor, sin contar la raíz de la unidad. Después se recorre esa lista y se van añadiendo las carpetas. Cuando entra una carpeta, se quitan de la selección sus antecesoras, y se para al llegar a 50 carpetas sin antecesoras ni descendientes entre sí. Así la lista señala dónde se concentra el espacio, en lugar de llenarse con la cadena `C:\Users`, `C:\Users\Manuel`, `…\AppData`.
- Es lógica pura y tiene pruebas.

## Formato

- Tamaños en unidades de 1024, con decimal de coma y una cifra: «1,8 GB», «640 MB», «12 KB», «0 B».
- Números de archivos con separador de miles de espacio: «12 304».
- Es lógica pura y tiene pruebas.

## Permisos

- La app detecta si está elevada.
- «Reiniciar como administrador» relanza el propio `.exe` con `ShellExecuteW("runas")` y cierra la instancia actual. Si el usuario cancela el UAC, la app sigue como estaba.
- El manifiesto pide `asInvoker`.

## Lo que queda en manos del constructor

- La biblioteca de interfaz:
  - `native-windows-gui` o el crate `windows` / `windows-sys`. Se elige la que dé menos código con un resultado fiable.
  - La lista de categorías puede ser un ListView con casillas o casillas nativas en rejilla, con el mismo contenido que la maqueta.
- La estructura de módulos y crates, siempre que la lógica pura compile en Linux y lo de Windows vaya tras `cfg(windows)`.
- Cómo pasar el progreso de los hilos de trabajo a la interfaz.
- El perfil de release para que el `.exe` ocupe menos de 3 MB (por ejemplo `opt-level="z"`, LTO, strip y `panic="abort"`) y el manifiesto embebido (DPI aware y Common Controls v6).
- Si el `.exe` pasa de 3 MB, se apunta en Pendientes con su tamaño. No es un bloqueo.

## Entrega

- README actualizado (WinCleaner: qué hace, cómo compilar y cómo usar).
- Licencia MIT (`LICENSE`, autor 9n-dev, 2026).
- `.gitignore` con `target/` y `.superpowers/`.
- PR desde `feat/v1`.
- En la ficha del proyecto: stack, decisiones y, en «## Pendientes», qué está hecho, qué falta y qué probar mañana en Windows.
