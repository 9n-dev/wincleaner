# WinCleaner v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Un `wincleaner.exe` portable (<3 MB, Win32 nativo, en español) que muestra el espacio por unidad, el top 50 de carpetas y archivos, y limpia categorías conocidas de basura con vista previa, confirmación e informe.

**Architecture:** Un solo crate `wincleaner` con dos partes:
- **Biblioteca pura** (`src/lib.rs`): seguridad de rutas, categorías, escaneo y borrado, análisis y formato. Solo usa `std` y se prueba en Linux con `cargo test`.
- **Binario** (`src/main.rs`): en Windows usa la biblioteca más una capa de API de Win32 (`src/win.rs`) y la interfaz NWG (`src/gui.rs`). En otros sistemas, el binario solo imprime un aviso.

Las dependencias de Windows van en `[target.'cfg(windows)'.dependencies]`, así `cargo test` en Linux no las compila. Esto se comprobó con un spike.

**Tech Stack:**
- Rust 1.95, edición 2024.
- `native-windows-gui = 1.0.13` sin las features por defecto, con `tabs`, `list-view`, `progress-bar`, `notice`, `combobox`, `status-bar` y `high-dpi`.
- `winapi = 0.3` (la misma versión que usa NWG).
- `embed-resource = 3.0.12` como dependencia de build, para el manifiesto.

**Spec:** `docs/superpowers/specs/2026-10-22-wincleaner-v1-design.md`. Léelo antes de cada tarea.

## Global Constraints

- **Interfaz en español; nombre «WinCleaner»;** binario `wincleaner.exe`; título de la ventana `WinCleaner`.
- **Comandos que tienen que pasar al final de cada tarea:**
  - `cargo test` pasa en Linux, sin avisos.
  - `cargo build --release --target x86_64-pc-windows-gnu` termina sin ninguna línea `warning:`.
- **Ejecutable y dependencias:**
  - El `.exe` de release debe ocupar menos de 3 MB.
  - No se añaden dependencias aparte de las tres de arriba.
  - Las pruebas usan solo `std`, con el helper `src/testutil.rs` de la Task 1.
- **Umbrales:**
  - Temporales (usuario y Windows): más de 24 h.
  - Volcados y logs: más de 30 días.
  - La antigüedad de un archivo es el valor **más reciente** entre `created()` y `modified()`. Si `created()` da error, se usa `modified()`.
- **Reglas de seguridad:**
  - Solo se borran archivos estrictamente dentro de una raíz validada. Una raíz nunca se borra.
  - No se siguen ni se borran enlaces simbólicos, junctions ni ningún punto de reanálisis.
  - Cualquier error de E/S cuenta como «saltado». Nunca se usa `panic`, `unwrap` ni `expect` sobre E/S.
  - No existe ningún camino de código que borre sin pasar por la confirmación de la interfaz.
- **Commits:**
  - Un commit por tarea, en la rama `feat/v1`.
  - Mensajes en español.
  - Cada mensaje termina con la línea `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.
- **Herramientas y bloqueos:**
  - Nunca se usa `sudo`.
  - Si una herramienta falta o algo bloquea la tarea, se apunta en el informe de la tarea y se sigue.

## Review Focus

1. **`%TEMP%` mal configurado** (que apunte al perfil, a Documentos o a la raíz de la unidad): la categoría tiene que quedar «ruta no reconocida» y no escanear nada. Lo prueba la Task 3, `temp_pointing_to_profile_is_unrecognized` y `temp_inside_documents_is_unrecognized`.
2. **Archivo bloqueado o sin permiso:** se salta, se cuenta y la limpieza continúa. Lo prueba la Task 4, `delete_counts_permission_denied_as_skipped`.
3. **Enlace dentro de una raíz que apunta fuera** (por ejemplo a Documentos): no se recorre ni se borra lo de fuera. Lo prueban la Task 4 (`symlinks_are_never_followed`) y la Task 5 (`analyze_does_not_follow_symlinks`). En Windows, que el atributo de reanálisis lo detecte es algo que Manuel tiene que probar a mano.
4. **Carpetas sin permiso al analizar** (`System Volume Information`): se saltan sin abortar ni hacer `panic`. Lo prueba la Task 5, `analyze_skips_unreadable_dirs`.
5. **Nombres con ñ, tildes o espacios:** se escanean y se borran con normalidad. Lo prueba la Task 4, `unicode_names_are_deleted`.

---

### Task 1: Esqueleto del crate y seguridad de rutas

**Files:**
- Create: `Cargo.toml`, `build.rs`, `wincleaner.rc`, `wincleaner.exe.manifest`, `src/lib.rs`, `src/main.rs`, `src/testutil.rs`, `src/paths.rs`

**Interfaces:**
- Produces:
  - `wincleaner::paths::is_inside(path: &str, root: &str) -> bool`: `path` es descendiente **estricto** de `root`.
  - `wincleaner::paths::root_is_safe(root: &str, protected: &[String], forbidden_trees: &[String]) -> bool`
  - `wincleaner::paths::temp_name_ok(root: &str) -> bool`
  - `crate::testutil::temp_dir(tag: &str) -> std::path::PathBuf` (solo en `#[cfg(test)]`): crea una carpeta vacía y única en `std::env::temp_dir()/wincleaner-test-<tag>-<pid>`, borrando antes la que hubiera.

- [ ] **Step 1: Esqueleto.**
  - **`Cargo.toml`:**
    - `name = "wincleaner"`, `edition = "2024"`.
    - Las dependencias del Tech Stack: NWG y `winapi` bajo `[target.'cfg(windows)'.dependencies]`.
    - Features de `winapi`: `shellapi`, `shlobj`, `knownfolders`, `combaseapi`, `fileapi`, `winbase`, `winuser`, `shtypes`, `winerror`. Si hacen falta más, se añaden.
    - `[profile.release]` con `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"` y `strip = true`.
  - **`build.rs`:** si `CARGO_CFG_TARGET_OS == "windows"`, ejecuta `embed_resource::compile("wincleaner.rc", embed_resource::NONE).manifest_required().unwrap();`.
  - **`wincleaner.rc`:** `#define RT_MANIFEST 24` y `1 RT_MANIFEST "wincleaner.exe.manifest"`.
  - **Manifiesto:**
    - Dependencia de `Microsoft.Windows.Common-Controls` 6.0.0.0 (`publicKeyToken="6595b64144ccf1df"`).
    - `requestedExecutionLevel level="asInvoker"`.
    - `<dpiAware>true/pm</dpiAware>`.
  - **`src/lib.rs`:** declara `pub mod paths;` y `#[cfg(test)] mod testutil;`. Las siguientes tareas añaden aquí sus módulos.
  - **`src/main.rs`:**
    - `#![cfg_attr(windows, windows_subsystem = "windows")]`.
    - En `cfg(not(windows))`, `main` imprime `WinCleaner solo funciona en Windows.` por stderr.
    - En `cfg(windows)`, de momento `main` vacío. La Task 7 lo conecta con la interfaz.

- [ ] **Step 2: Pruebas que fallan, en `src/paths.rs` (`#[cfg(test)] mod tests`).** Con `T = r"C:\Users\M\AppData\Local\Temp"`:
  - **`inside_basic`:**
    - `is_inside(r"C:\Users\M\AppData\Local\Temp\a.txt", T)`.
    - `is_inside(r"C:\Users\M\AppData\Local\Temp\x\y\z.tmp", T)`.
  - **`inside_is_case_insensitive_and_accepts_both_separators`:**
    - `is_inside(r"c:\users\m\appdata\local\TEMP\a", T)`.
    - `is_inside("C:/Users/M/AppData/Local/Temp/a", T)`.
    - `is_inside(r"C:\Users\M\AppData\Local\Temp\a", r"C:\Users\M\AppData\Local\Temp\")` (raíz con barra final).
  - **`root_itself_is_not_inside`:**
    - `!is_inside(T, T)`.
    - `!is_inside(&format!(r"{T}\"), T)`.
  - **`sibling_prefix_is_not_inside`:** `!is_inside(r"C:\Users\M\AppData\Local\Temp2\a", T)`.
  - **`dot_components_are_rejected`:**
    - `!is_inside(r"C:\Users\M\AppData\Local\Temp\..\..\..\Documents\a", T)`.
    - `!is_inside(r"C:\Users\M\AppData\Local\Temp\.\a", T)`.
    - `!is_inside(r"C:\Users\M\AppData\Local\Temp\a", r"C:\Users\M\AppData\Local\Temp\..")`.
  - **`odd_forms_are_rejected`:**
    - `!is_inside(r"\\?\C:\Users\M\AppData\Local\Temp\a", T)`.
    - `!is_inside(r"\\server\share\Temp\a", r"\\server\share\Temp")`.
    - `!is_inside(r"Temp\a", "Temp")`.
    - `!is_inside(r"C:\Users\M\AppData\Local\Temp\a.txt:zone", T)`.
    - `!is_inside(r"D:\Users\M\AppData\Local\Temp\a", T)`.
    - `!is_inside("", T)`.
    - `!is_inside(r"C:\a", "")`.
  - **`unix_absolute_paths_work_for_tests`:**
    - `is_inside("/tmp/w/Temp/a", "/tmp/w/Temp")`.
    - `!is_inside("/tmp/w/Temp", "/tmp/w/Temp")`.
  - **`root_is_safe_rules`.** Con `protected = [r"C:\", r"C:\Users\M", r"C:\Windows", r"C:\Windows\System32", r"C:\Program Files"]` y `forbidden = [r"C:\Users\M\Documents", r"D:\Descargas"]`:
    - `root_is_safe(T, …)`.
    - `!root_is_safe(r"C:\Users\M", …)`.
    - `!root_is_safe(r"c:\users\m\", …)`.
    - `!root_is_safe(r"C:\", …)`.
    - `!root_is_safe(r"C:\Users", …)`.
    - `!root_is_safe(r"C:\Users\M\Documents\Temp", …)`.
    - `!root_is_safe(r"D:\", …)`.
    - `!root_is_safe(r"C:\Windows", …)`.
    - `root_is_safe(r"C:\Windows\Temp", …)`.
    - `!root_is_safe(r"..\Temp", …)`.
  - **`temp_name_rules`.**
    - Verdadero:
      - `temp_name_ok(T)`.
      - `temp_name_ok(r"C:\x\tmp")`.
      - `temp_name_ok(r"C:\x\TEMP\")`.
      - `temp_name_ok(r"C:\x\Temp\2")`.
    - Falso:
      - `!temp_name_ok(r"C:\x\Temp\abc")`.
      - `!temp_name_ok(r"C:\x\Temp\2a")`.
      - `!temp_name_ok(r"C:\Users\M")`.
      - `!temp_name_ok(r"C:\x\Temporal")`.

- [ ] **Step 3: Ejecuta `cargo test paths`.** Esperado: no compila, porque las funciones no existen.

- [ ] **Step 4: Implementa `src/paths.rs`.** La pieza central es una función privada `parse(p: &str) -> Option<Vec<String>>`, que devuelve los componentes en minúsculas (`to_lowercase`):
  - **Separadores:** divide por `\` y `/`; los componentes vacíos se ignoran (barra final o doble barra).
  - **Primer componente:** tiene que ser una letra ASCII seguida de `:` (por ejemplo `c:`). También se acepta una ruta que empiece por `/`, para las pruebas en Linux. Cualquier otra forma devuelve `None`.
  - **Rechazos:** devuelve `None` si la ruta empieza por `\\` o `//` (UNC y `\\?\`), si algún componente es `.` o `..`, o si hay un `:` en cualquier componente que no sea el de la unidad.
  - **`is_inside`:** los dos lados se parsean, `path` es más largo y empieza por `root`.
  - **`root_is_safe`:** `root` se parsea, no es igual a ningún `protected` ni a ningún `forbidden`, no es antecesor de ninguno de los dos y no está dentro de ningún `forbidden`. Las entradas de `protected` y `forbidden` que no se puedan parsear se ignoran.
  - **`temp_name_ok`:** el último componente es `temp` o `tmp`, o bien es solo dígitos y el penúltimo es `temp` o `tmp`.

- [ ] **Step 5: Comprobación.**
  - `cargo test` pasa.
  - `cargo build --release --target x86_64-pc-windows-gnu 2>&1 | grep -E '^(warning|error)'` no imprime nada.

- [ ] **Step 6: Commit** de `.gitignore` (si cambió), `Cargo.toml`, `Cargo.lock`, `build.rs`, `wincleaner.rc`, `wincleaner.exe.manifest` y `src/`, con el mensaje `Esqueleto del crate y seguridad de rutas`.

---

### Task 2: Formato de tamaños y números

**Files:**
- Create: `src/format.rs`
- Modify: `src/lib.rs` (añade `pub mod format;`)

**Interfaces:**
- Produces:
  - `wincleaner::format::bytes(n: u64) -> String`
  - `wincleaner::format::count(n: u64) -> String`

- [ ] **Step 1: Pruebas que fallan.**
  - **`bytes_examples`:** cada valor debe dar exactamente el texto indicado.

    | Entrada | Resultado |
    |---|---|
    | `0` | `"0 B"` |
    | `512` | `"512 B"` |
    | `1536` | `"1,5 KB"` |
    | `12 * 1024` | `"12 KB"` |
    | `640 * 1024 * 1024` | `"640 MB"` |
    | `1932735283` (≈1,8 GiB) | `"1,8 GB"` |
    | `22977575731` (≈21,4 GiB) | `"21,4 GB"` |
    | `237 * 1024u64.pow(3)` | `"237 GB"` |
    | `2 * 1024u64.pow(4)` | `"2 TB"` |
  - **`count_examples`:** `"0"`, `"999"`, `"12 304"` y `"1 000 000"` para `0`, `999`, `12304` y `1000000`.

- [ ] **Step 2: Ejecuta `cargo test format`.** Esperado: no compila.

- [ ] **Step 3: Implementa las dos funciones.**
  - **`bytes`:**
    - Unidades B, KB, MB, GB y TB, con base 1024.
    - Se sube de unidad mientras el valor sea `>= 1024`.
    - En bytes, se muestra el entero.
    - Por debajo de 100: un decimal con coma, quitando `,0`.
    - Desde 100: entero redondeado.
  - **`count`:** grupos de 3 cifras separados por un espacio normal.

- [ ] **Step 4: Comprobación.** `cargo test` pasa.

- [ ] **Step 5: Commit** con el mensaje `Formato de tamaños y números`.

---

### Task 3: Categorías y validación de raíces

**Files:**
- Create: `src/categories.rs`
- Modify: `src/lib.rs` (añade `pub mod categories;`)

**Interfaces:**
- Consumes: `paths::root_is_safe` y `paths::temp_name_ok`, de la Task 1.
- Produces (todo `pub` en `wincleaner::categories`):
  - **Constantes:**
    ```rust
    pub const DAY: Duration = Duration::from_secs(86_400);
    ```
  - **`Env`:**
    ```rust
    pub struct Env {
        pub temp: Option<PathBuf>,
        pub system_root: Option<PathBuf>,
        pub local_app_data: Option<PathBuf>,
        pub program_data: Option<PathBuf>,
        pub protected: Vec<String>,
        pub forbidden_trees: Vec<String>,
        pub is_admin: bool,
    }
    ```
  - **`CategoryId`:**
    ```rust
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum CategoryId {
        UserTemp, WindowsTemp, RecycleBin, Thumbnails, Chrome, Edge, Firefox, DumpsLogs, WindowsUpdate,
    }
    ```
  - **`Filter`:**
    ```rust
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Filter { OlderThan(Duration), Thumbcache, All }
    ```
  - **`Status`:**
    ```rust
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Status { Ready, NeedsAdmin, NotFound, Unrecognized }
    ```
  - **`Category`:**
    ```rust
    #[derive(Clone, Debug)]
    pub struct Category {
        pub id: CategoryId,
        pub name: &'static str,
        pub filter: Filter,
        pub roots: Vec<PathBuf>,
        pub status: Status,
    }
    ```
  - **Función principal:**
    ```rust
    pub fn build_categories(env: &Env) -> Vec<Category>
    ```
    Devuelve siempre 9 categorías, en el orden de `CategoryId`.
  - **Detección de enlaces:**
    ```rust
    pub fn is_link(md: &std::fs::Metadata) -> bool
    ```
    Vale para enlaces simbólicos y, en Windows (`#[cfg(windows)]`, con `std::os::windows::fs::MetadataExt`), también para cualquier archivo con `file_attributes() & 0x400` (`FILE_ATTRIBUTE_REPARSE_POINT`). La usan las Tasks 4 y 5.

**Nombres de las categorías** (exactos, en el orden de `CategoryId`):
1. «Temporales del usuario»
2. «Temporales de Windows»
3. «Papelera de reciclaje»
4. «Caché de miniaturas»
5. «Caché de Chrome»
6. «Caché de Edge»
7. «Caché de Firefox»
8. «Volcados de errores y logs antiguos»
9. «Caché de Windows Update»

**Filtros:**
- **Temporales (usuario y Windows):** `OlderThan(DAY)`.
- **Miniaturas:** `Thumbcache`.
- **Volcados y logs:** `OlderThan(30 * DAY)`.
- **Resto:** `All`.

**Raíces candidatas.** Se unen con `Path::join`, y cada segmento con barra invertida del spec se convierte en un `join` por componente:
- **UserTemp:** `env.temp`.
- **WindowsTemp:** `system_root/Temp`.
- **RecycleBin:** sin raíces, siempre `Ready`.
- **Thumbnails:** `local_app_data/Microsoft/Windows/Explorer`.
- **Chrome:**
  - Se listan las subcarpetas de `local_app_data/Google/Chrome/User Data`.
  - De ellas, solo `Default` y las que empiezan por `Profile `.
  - Dentro de cada perfil, `Cache`, `Code Cache` y `GPUCache`.
- **Edge:** igual que Chrome, con `local_app_data/Microsoft/Edge/User Data`.
- **Firefox:** cada subcarpeta de `local_app_data/Mozilla/Firefox/Profiles`, más `/cache2`.
- **DumpsLogs:**
  - Siempre:
    - `local_app_data/CrashDumps`
    - `local_app_data/Microsoft/Windows/WER/ReportArchive`
    - `local_app_data/Microsoft/Windows/WER/ReportQueue`
  - Solo si `is_admin`:
    - `program_data/Microsoft/Windows/WER/ReportArchive`
    - `program_data/Microsoft/Windows/WER/ReportQueue`
    - `system_root/Minidump`
    - `system_root/Logs/CBS`
- **WindowsUpdate:** `system_root/SoftwareDistribution/Download`.

**Estado de cada categoría:**
1. **`NeedsAdmin`:** si la categoría es WindowsTemp o WindowsUpdate y `!is_admin`. Las raíces se dejan vacías.
2. **Si no:**
   - Una candidata **existe** si `symlink_metadata` dice que es carpeta y `!is_link(&md)`.
   - Una candidata es **segura** si `root_is_safe(ruta, &env.protected, &env.forbidden_trees)`. En el caso de UserTemp, además debe cumplir `temp_name_ok`. Las rutas que no se puedan pasar a `&str` no son seguras.
   - **`Ready`:** quedan raíces existentes y seguras (solo esas se guardan).
   - **`Unrecognized`:** alguna candidata existía, pero ninguna era segura.
   - **`NotFound`:** no existe ninguna candidata.

- [ ] **Step 1: Pruebas que fallan.** Cada una monta un perfil falso en `testutil::temp_dir`: `.../U` como perfil, `.../U/Documents` y `.../U/AppData/Local`. El `Env` usa `protected = [U]` y `forbidden_trees = [U/Documents]`.
  - **`builds_nine_categories_in_order`:**
    - Hay 9 categorías.
    - `ids` en el orden del enum.
    - `[0].name == "Temporales del usuario"`.
  - **`user_temp_ready_when_safe`:**
    - Con `temp = U/AppData/Local/Temp` (creada), queda `Ready`.
    - Sus raíces son `[esa ruta]`.
  - **`temp_pointing_to_profile_is_unrecognized`:**
    - `temp = U` da `Unrecognized` y raíces vacías.
  - **`temp_inside_documents_is_unrecognized`:**
    - `temp = U/Documents/Temp` (creada) da `Unrecognized`.
  - **`missing_dirs_are_not_found`:**
    - Con `temp = None`, UserTemp queda `NotFound`.
    - Sin carpetas de Chrome, Chrome queda `NotFound`.
  - **`admin_categories_need_admin`:**
    - Con `is_admin = false` y `system_root` con `Temp` y `SoftwareDistribution/Download` creadas, WindowsTemp y WindowsUpdate quedan `NeedsAdmin`.
    - Con `is_admin = true`, quedan `Ready`.
  - **`chrome_profiles_only_cache_dirs`:**
    - Se crean:
      - `User Data/Default/Cache`
      - `User Data/Default/Code Cache`
      - `User Data/Default/Network`
      - `User Data/Profile 1/GPUCache`
      - `User Data/System Profile/Cache`
    - Las raíces son exactamente las tres de caché de `Default` y `Profile 1` (comparadas como conjunto).
  - **`dumps_admin_roots_only_when_admin`:**
    - Con `system_root/Minidump` y `local/CrashDumps` creadas, sin admin las raíces son solo `CrashDumps`.
    - Con admin, incluyen `Minidump`.
  - **`recycle_bin_always_ready`.**

- [ ] **Step 2: Ejecuta `cargo test categories`.** Esperado: no compila.

- [ ] **Step 3: Implementa `categories.rs`.**

- [ ] **Step 4: Comprobación.**
  - `cargo test` pasa.
  - El build de Windows termina sin avisos.

- [ ] **Step 5: Commit** con el mensaje `Categorías de limpieza y validación de raíces`.

---

### Task 4: Escaneo y borrado seguros

**Files:**
- Create: `src/clean.rs`
- Modify: `src/lib.rs` (añade `pub mod clean;`)

**Interfaces:**
- Consumes: `Category`, `Filter`, `Status` y `is_link` (de `categories`), y `paths::is_inside`.
- Produces (en `wincleaner::clean`):
  - **Resultado del escaneo:**
    ```rust
    #[derive(Clone, Debug, Default)]
    pub struct ScanResult {
        pub files: Vec<(PathBuf, u64)>,
        pub dirs: Vec<PathBuf>,
        pub bytes: u64,
    }
    ```
    - `dirs`: candidatas a borrar si quedan vacías, ordenadas de la más profunda a la menos profunda.
    - El número de archivos es `files.len()`.
  - **Escaneo:**
    ```rust
    pub fn scan(cat: &Category, now: SystemTime) -> ScanResult
    ```
  - **Informe de borrado:**
    ```rust
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct DeleteReport { pub freed: u64, pub deleted: u64, pub skipped: u64, pub skipped_bytes: u64 }
    ```
  - **Borrado:**
    ```rust
    pub fn delete(cat: &Category, scan: &ScanResult, now: SystemTime) -> DeleteReport
    ```

**Reglas:**
- **Antigüedad.** `age_ok(md, filter, now)`:
  - `All`: siempre cumple.
  - `Thumbcache`: siempre cumple; el filtro por nombre y nivel va aparte.
  - `OlderThan(d)`: cumple si `max(created, modified) + d <= now`. Si `created()` da error, se usa `modified()`. Si `modified()` también falla, no cumple.
- **`scan`:**
  - Si `cat.status != Ready`, devuelve un resultado vacío.
  - Por cada raíz, recorrido iterativo con una pila de carpetas.
  - Las entradas se leen con `symlink_metadata`. Las que son enlace se ignoran del todo, y los errores de lectura se ignoran.
  - **Archivos:** entran si cumplen la antigüedad y `is_inside(archivo, raíz)`.
  - **Carpetas:**
    - Con `Thumbcache` no se entra en subcarpetas.
    - Con `Thumbcache`, solo entran los archivos del primer nivel cuyo nombre en minúsculas empiece por `thumbcache_` y acabe en `.db`.
    - Las subcarpetas (nunca la raíz) se añaden a `dirs` si su `modified()` cumple la antigüedad (`OlderThan`). Con `All`, siempre.
- **`delete`:**
  - **Por cada archivo de `scan.files`:**
    1. Tiene que estar dentro de alguna de `cat.roots` según `is_inside` (si la ruta no se puede pasar a `&str`, no lo está). Si no, cuenta como saltado.
    2. Se relee con `symlink_metadata`:
       - Si da `NotFound`, se ignora sin contarlo.
       - Si da otro error, es un enlace, no es un archivo o ya no cumple la antigüedad, cuenta como saltado.
    3. Si es de solo lectura, se le quita ese atributo con `set_permissions`.
    4. Se borra con `remove_file`:
       - Si va bien: `freed += md.len()` y `deleted += 1`.
       - Si falla: saltado.
    - En cada salto: `skipped += 1` y `skipped_bytes += tamaño escaneado`.
  - **Después, por cada carpeta de `scan.dirs`:** si está dentro de una raíz y no es enlace, se intenta `remove_dir`. Los errores se ignoran y no se cuenta nada.
  - **Recycle bin:** `delete` no la toca. La gestiona la capa de Windows.

**Truco para las pruebas en Linux:** la fecha de creación no se puede cambiar.
- Para tener un archivo «viejo», usa `now = SystemTime::now() + 2 * DAY` y no toques su fecha.
- Para tener uno «nuevo», usa `File::set_modified(SystemTime::now() + 5 * DAY)`.

- [ ] **Step 1: Pruebas que fallan.** Todas montan una categoría a mano (`Category { status: Ready, roots: vec![root], filter, .. }`) sobre `testutil::temp_dir`:
  - **`scan_counts_files_and_bytes`:**
    - Con `a.bin` (10 B) y `sub/b.bin` (20 B), y `All`, salen 2 archivos y `bytes == 30`.
  - **`scan_skips_non_ready`:**
    - Con `status: NeedsAdmin`, sale vacío.
  - **`older_than_filter_uses_newest_date`:**
    - `viejo.tmp` sin tocar y `nuevo.tmp` con `set_modified(now_real + 5 días)`.
    - Con `OlderThan(DAY)` y `now = now_real + 2 días`, solo entra `viejo.tmp`.
  - **`thumbcache_filter`:**
    - Se crean `thumbcache_32.db`, `Thumbcache_IDX.DB`, `iconcache_16.db` y `sub/thumbcache_x.db`.
    - Solo entran los dos primeros.
  - **`symlinks_are_never_followed`:**
    - Hay una carpeta `fuera/importante.txt` y, dentro de la raíz, `enlace -> fuera` y `enlace_archivo -> fuera/importante.txt` (`std::os::unix::fs::symlink`, con `#[cfg(unix)]`).
    - El escaneo no incluye nada de `fuera`.
    - Después de `delete`, `fuera/importante.txt` y los dos enlaces siguen existiendo.
  - **`delete_frees_and_keeps_root`:**
    - Con `All` y dos archivos (10 y 20 B) en `sub/`, el informe es `freed == 30` y `deleted == 2`.
    - `sub/` ya no existe y la raíz sí.
  - **`delete_refuses_paths_outside_roots`:**
    - Se crea un `ScanResult` a mano con un archivo de otra carpeta (10 B).
    - Ese archivo sigue existiendo y el informe es `skipped == 1` y `skipped_bytes == 10`.
  - **`delete_removes_readonly_files`:**
    - Un archivo de solo lectura dentro de la raíz se borra.
  - **`delete_counts_permission_denied_as_skipped`** (`#[cfg(unix)]`):
    - Con `sub/a.bin`, se pone `sub` en modo `0o555` y se hace el `delete`.
    - El informe es `skipped == 1` y no hay `panic`.
    - Restaura `0o755` antes de terminar.
  - **`unicode_names_are_deleted`:**
    - `año ñandú (copia).tmp` se escanea y se borra.
  - **`vanished_files_are_ignored`:**
    - Se borra a mano un archivo después del escaneo.
    - El informe es `skipped == 0` y `deleted == 0`.

- [ ] **Step 2: Ejecuta `cargo test clean`.** Esperado: no compila.

- [ ] **Step 3: Implementa `clean.rs`.**

- [ ] **Step 4: Comprobación.**
  - `cargo test` pasa.
  - El build de Windows termina sin avisos.

- [ ] **Step 5: Commit** con el mensaje `Escaneo y borrado seguros`.

---

### Task 5: Análisis del top 50

**Files:**
- Create: `src/analyze.rs`
- Modify: `src/lib.rs` (añade `pub mod analyze;`)

**Interfaces:**
- Consumes: `categories::is_link`.
- Produces (en `wincleaner::analyze`):
  - **Constante:**
    ```rust
    pub const TOP_N: usize = 50;
    ```
  - **Resultado:**
    ```rust
    #[derive(Clone, Debug, Default)]
    pub struct Analysis {
        pub top_files: Vec<(PathBuf, u64)>,
        pub top_dirs: Vec<(PathBuf, u64)>,
        pub files_scanned: u64,
        pub cancelled: bool,
    }
    ```
    Las dos listas van ordenadas de mayor a menor tamaño.
  - **Análisis:**
    ```rust
    pub fn analyze(root: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Analysis
    ```
  - **Selección de carpetas:**
    ```rust
    pub fn pick_top_dirs(dirs: Vec<(PathBuf, u64)>, n: usize) -> Vec<(PathBuf, u64)>
    ```

**Algoritmo de `analyze`:**
1. **Recorrido** iterativo, con `Vec<(PathBuf, parent: usize, own: u64)>`: el índice 0 es `root` y cada carpeta se añade cuando se descubre.
2. **Errores y enlaces:**
   - Los enlaces (`is_link` sobre `symlink_metadata`) se ignoran.
   - Las carpetas que no se pueden leer se saltan en silencio.
3. **Archivos:**
   - Su tamaño suma al `own` de su carpeta.
   - Se mantienen los `TOP_N` archivos más grandes con un `BinaryHeap<Reverse<(u64, PathBuf)>>`.
   - `files_scanned += 1`.
   - Cada 10 000 archivos se llama a `progress(files_scanned)`.
4. **Cancelación:**
   - Antes de leer cada carpeta se comprueba `cancel.load(Relaxed)`.
   - Si está activa, se devuelve `cancelled: true` con lo acumulado hasta ese momento.
5. **Tamaño recursivo:**
   - Se recorre el vector al revés, sumando `total[i]` a `total[parent[i]]` (los hijos siempre tienen un índice mayor que el padre).
6. **Resultado:**
   - `top_dirs = pick_top_dirs(todas menos el índice 0, TOP_N)`.

**Algoritmo de `pick_top_dirs`:**
```text
ordena por tamaño desc; en empate, primero la de más componentes
selected = []
para cada c:
  si alguna s de selected empieza por c (s.starts_with(c)) → salta c
  quita de selected toda s que sea antecesora de c (c.starts_with(s))
  añade c
  si selected.len() == n → para
devuelve selected ordenado por tamaño desc
```

- [ ] **Step 1: Pruebas que fallan.**
  - **`pick_prefers_where_space_concentrates`.** Con `/r/a` 100, `/r/a/b` 90, `/r/a/b/c` 80, `/r/d` 50 y `/r/a/e` 5:
    - `n = 2` da `[/r/a/b/c 80, /r/d 50]`.
    - `n = 3` da `[/r/a/b/c 80, /r/d 50, /r/a/e 5]`.
  - **`pick_tie_keeps_deepest`:**
    - Con `/r/x` 10 y `/r/x/y` 10, y `n = 5`, da `[/r/x/y 10]`.
  - **`analyze_sizes_and_top_files`.** Con el árbol `a/1.bin` (100 B), `a/b/2.bin` (50 B) y `c/3.bin` (10 B):
    - `files_scanned == 3`.
    - `top_files[0]` es `1.bin` con 100.
    - Ninguna entrada de `top_dirs` es la raíz.
    - `top_dirs` contiene `a/b` (50) y `c` (10).
    - `a` no está en `top_dirs`, porque su descendiente `a/b` ya está seleccionado.
  - **`analyze_keeps_only_top_50_files`:**
    - Con 55 archivos de tamaños del 1 al 55, `top_files.len() == 50`.
    - El primero ocupa 55 y el último 6.
  - **`analyze_does_not_follow_symlinks`** (`cfg(unix)`):
    - Un enlace dentro de la raíz apunta a una carpeta de fuera con un archivo de 1000 B.
    - Ese archivo no aparece y `files_scanned` no lo cuenta.
  - **`analyze_skips_unreadable_dirs`** (`cfg(unix)`):
    - Una subcarpeta con modo `0o000` y otra legible con un archivo.
    - Termina con `cancelled == false` y cuenta el archivo legible.
    - Restaura los permisos.
  - **`analyze_honours_cancel`:**
    - Con `AtomicBool` a `true` desde el principio, `cancelled == true` y `files_scanned == 0`.

- [ ] **Step 2: Ejecuta `cargo test analyze`.** Esperado: no compila.

- [ ] **Step 3: Implementa `analyze.rs`.**

- [ ] **Step 4: Comprobación.**
  - `cargo test` pasa.
  - El build de Windows termina sin avisos.

- [ ] **Step 5: Commit** con el mensaje `Análisis del top 50 de carpetas y archivos`.

---

### Task 6: Capa de Win32

**Files:**
- Create: `src/win.rs` (módulo del binario; en `main.rs`: `#[cfg(windows)] mod win;`)

**Interfaces:**
- Consumes: `wincleaner::categories::Env`.
- Produces (`pub(crate)`, solo Windows):
  - **Entorno:**
    ```rust
    pub fn gather_env() -> Env
    ```
  - **Permisos:**
    ```rust
    pub fn is_admin() -> bool
    pub fn relaunch_as_admin() -> bool
    ```
    `relaunch_as_admin` devuelve `true` si se ha lanzado la instancia elevada.
  - **Papelera:**
    ```rust
    pub fn recycle_bin_size() -> (u64, u64)
    pub fn empty_recycle_bin() -> bool
    ```
    `recycle_bin_size` devuelve `(bytes, elementos)`.
  - **Unidades:**
    ```rust
    pub struct Drive { pub root: String /* "C:\\" */, pub free: u64, pub total: u64 }
    pub fn fixed_drives() -> Vec<Drive>
    ```
  - **Explorador:**
    ```rust
    pub fn open_in_explorer(path: &Path)
    ```

**Implementación** (con `winapi`; cada `unsafe` lleva un comentario de una línea):
- **`gather_env`:**
  - **Carpetas base:**
    - `temp`: `std::env::var_os("TEMP")` pasado por `long_path`.
    - `system_root`: `known_folder(FOLDERID_Windows)`.
    - `local_app_data`: `known_folder(FOLDERID_LocalAppData)`.
    - `program_data`: `known_folder(FOLDERID_ProgramData)`.
  - **`protected`:**
    - Cada raíz de `GetLogicalDriveStringsW` (todas las unidades, de cualquier tipo).
    - `FOLDERID_Profile`, `FOLDERID_Windows`, `FOLDERID_System`, `FOLDERID_ProgramFiles` y `FOLDERID_ProgramFilesX86`.
  - **`forbidden_trees`:** `FOLDERID_Documents`, `Desktop`, `Downloads`, `Pictures`, `Music` y `Videos`.
  - **`is_admin`:** `is_admin()`.
- **`known_folder(id) -> Option<PathBuf>`:** `SHGetKnownFolderPath(id, 0, null)` y luego `CoTaskMemFree`.
- **`long_path(p) -> PathBuf`:** `GetLongPathNameW`. Si falla, devuelve `p` tal cual.
- **`is_admin`:** `IsUserAnAdmin() != 0`.
- **`relaunch_as_admin`:** `ShellExecuteW(null, "runas", current_exe, null, null, SW_SHOWNORMAL) as isize > 32`.
- **`recycle_bin_size`:** `SHQueryRecycleBinW(null, &mut SHQUERYRBINFO { cbSize: size_of, .. })`. Si falla, devuelve `(0, 0)`.
- **`empty_recycle_bin`:** `SHEmptyRecycleBinW(null, null, SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND)`. Es éxito si devuelve `S_OK`, y también si la papelera ya estaba vacía.
- **`fixed_drives`:** solo unidades `DRIVE_FIXED`, con `GetDiskFreeSpaceExW`; se descartan las que fallen.
- **`open_in_explorer`:** `Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn()`. El error se ignora.

- [ ] **Step 1: Implementa `win.rs`.**
  - No tiene pruebas: no se puede ejecutar en Linux.
  - En `main.rs` (`cfg(windows)`), llama provisionalmente a `let _ = win::gather_env();` para que no haya avisos de código muerto. La Task 7 lo sustituye.
  - Si aun así queda algún aviso de código muerto, pon `#[allow(dead_code)]` **solo** en el módulo y quítalo en la Task 7.

- [ ] **Step 2: Comprobación.**
  - `cargo build --release --target x86_64-pc-windows-gnu 2>&1 | grep -E '^(warning|error)'` no imprime nada.
  - `cargo test` sigue pasando.

- [ ] **Step 3: Commit** con el mensaje `Capa de Win32: entorno, permisos, papelera y unidades`.

---

### Task 7: Ventana, unidades y pestaña Limpieza

**Files:**
- Create: `src/gui.rs` (`#[cfg(windows)] mod gui;` en `main.rs`; `main` llama a `gui::run()`)

**Interfaces:**
- Consumes:
  - Task 6: todo `win::*`.
  - Task 3: `build_categories`, `Category`, `CategoryId` y `Status`.
  - Task 4: `clean::scan`, `clean::delete`, `ScanResult` y `DeleteReport`.
  - Task 2: `format::bytes` y `format::count`.
- Produces:
  - `gui::run()`.
  - El `struct App` con el estado de la interfaz, ampliado por la Task 8.

**Estructura:**
- **Ventana y eventos:**
  - Interfaz con la API de builders de NWG, sin el derive.
  - Un `Rc<App>` con los controles y un `RefCell<State>`.
  - Un solo `nwg::full_bind_event_handler`.
  - Fuente global: Segoe UI, tamaño 16.
- **Tamaño de la ventana:**
  - 640×600 lógicos, sin poder cambiarlo: flags `WINDOW | VISIBLE`, sin `RESIZABLE`.
  - Título `WinCleaner`, o `WinCleaner (administrador)` si la app está elevada.
- **Hilos:**
  - Los trabajos van en `std::thread::spawn`, que manda un `Msg` por `std::sync::mpsc::Sender` y llama a `notice_sender.notice()`.
  - En `OnNotice`, la interfaz vacía el canal con `try_recv`.
  - El `enum Msg` es privado de `gui.rs`.
- **Unidades:**
  - Encima de las pestañas, una fila por cada unidad de `fixed_drives()`, como máximo 4.
  - Cada fila tiene una etiqueta `C:`, una `ProgressBar` de 0 a 100 con el % usado y el texto `«{libre} libres de {total}»` con `format::bytes`.
  - Si queda menos del 10 % libre, la barra pasa a `ProgressBarState::Error` (roja).
  - Se agrega `ponytail:` para indicar que solo se muestran 4 unidades y que haría falta scroll si hubiera más.
- **Pestañas:** `TabsContainer` con «Limpieza» y «Análisis». La Task 8 rellena la segunda, que aquí se deja vacía.
- **Pestaña Limpieza:**
  - **Filas de categoría** (9, en el orden de `build_categories`): cada una tiene una `CheckBox` con el nombre, una etiqueta de tamaño alineada a la derecha y una etiqueta de archivos.
  - **Estado inicial:**
    - `Ready`: casilla marcada y habilitada, con «Calculando…».
    - `NeedsAdmin`: casilla desmarcada y deshabilitada, con «—» y «requiere admin.».
    - `NotFound`: casilla desmarcada y deshabilitada, con «—» y «no encontrado».
    - `Unrecognized`: casilla desmarcada y deshabilitada, con «—» y «ruta no reconocida».
  - **Al arrancar:**
    - La ventana se muestra primero.
    - Después se lanza un hilo que, por cada categoría `Ready`, hace `scan(cat, SystemTime::now())`. En la papelera usa `recycle_bin_size()`.
    - Cada categoría, al terminar, manda un `Msg::Scanned(idx, ScanResult)`.
  - **Al recibir un escaneo:**
    - Se actualizan el tamaño (`format::bytes`) y los archivos (`format::count`).
    - Si la categoría tiene 0 B, la casilla se desmarca.
    - El resultado se guarda en `State`.
  - **Papelera:**
    - Se envía con `ScanResult { files: vec![], dirs: vec![], bytes }`.
    - El número de elementos va en un campo aparte de `State`: `recycle_items: u64`.
  - **Botones:**
    - «🛡 Reiniciar como administrador»:
      - Si la app ya es administrador, está oculto.
      - Al pulsarlo, si `relaunch_as_admin()` va bien, se llama a `nwg::stop_thread_dispatch()`.
    - «Revisar y limpiar ({total})…»:
      - El total suma las categorías marcadas que ya tienen escaneo.
      - Se recalcula al marcar o desmarcar una casilla (`OnButtonClick` de la casilla).
      - Se deshabilita mientras queden escaneos pendientes o si el total es 0.
  - **Confirmación:** se usa `nwg::modal_message` con `MessageButtons::OkCancel`, `MessageIcons::Warning` y el título «Confirmar limpieza». El texto es exactamente este:
    ```text
    Se van a borrar:

    {nombre} — {tamaño}        (una línea por categoría marcada)

    Total: {total} ({archivos} archivos)

    Los archivos en uso se saltarán. Esto no se puede deshacer.
    ```
    - Los botones serán los del sistema en el idioma de Windows («Aceptar» y «Cancelar»). Es decisión del agente: evita un diálogo propio.
    - Solo se borra con `MessageChoice::Ok`.
  - **Borrado:**
    - Se deshabilitan los botones y las casillas, y la barra de estado muestra «Limpiando…».
    - Un hilo recorre las categorías marcadas:
      - **Papelera:** si `empty_recycle_bin()` va bien, `DeleteReport { freed: bytes escaneados, deleted: elementos, .. }`.
      - **Resto:** `clean::delete(cat, scan, SystemTime::now())`.
    - Al terminar envía `Msg::Cleaned(Vec<(&'static str, DeleteReport)>)`.
  - **Informe:**
    - Se muestra con `nwg::modal_info_message`, con el título «Limpieza terminada».
    - El texto empieza por `Liberados {total}`, sigue con una línea por categoría (`{nombre} — {liberado}`, más ` ({n} en uso, saltados)` si hubo saltados) y termina, si hubo saltados, con `Saltados: {n} archivos en uso ({tamaño})`.
    - Después se reactiva la interfaz y se repite el escaneo inicial: se reconstruyen las categorías y las unidades, y las filas se actualizan.
- **Barra de estado:**
  - «Calculando tamaños…» mientras hay escaneos.
  - «Tamaños calculados.» cuando terminan.

- [ ] **Step 1: Implementa `gui.rs`.** Si es posible, menos de 500 líneas entre esta tarea y la siguiente.

- [ ] **Step 2: Comprobación.**
  - `cargo build --release --target x86_64-pc-windows-gnu 2>&1 | grep -E '^(warning|error)'` no imprime nada.
  - `ls -l target/x86_64-pc-windows-gnu/release/wincleaner.exe` muestra menos de 3 000 000 bytes.
  - `cargo test` pasa.

- [ ] **Step 3: Revisa con `grep` que no hay `unwrap()` ni `expect(` sobre E/S en `gui.rs` ni en `win.rs`.** Sí se permiten en los builders de NWG al arrancar.

- [ ] **Step 4: Commit** con el mensaje `Ventana principal y pestaña de limpieza`.

---

### Task 8: Pestaña Análisis

**Files:**
- Modify: `src/gui.rs`

**Interfaces:**
- Consumes:
  - Task 5: `analyze::analyze`, `Analysis` y `TOP_N`.
  - Task 6: `win::fixed_drives` y `win::open_in_explorer`.
  - Task 2: `format::*`.

**Contenido de la pestaña:**
- **Fila superior:**
  - La etiqueta «Unidad:».
  - Un `ComboBox<String>` con las raíces de `fixed_drives()`, con la primera seleccionada.
  - Los `RadioButton` «Carpetas» (marcado) y «Archivos».
  - Un botón «Analizar».
- **`ListView` en modo detalles:**
  - Columnas `#` (40), `Ruta` (420) y `Tamaño` (100).
  - Selección de fila completa.
- **Fila inferior:**
  - La etiqueta de resumen.
  - El botón «Abrir ubicación».

**Comportamiento:**
- **Al pulsar «Analizar»:**
  - El botón pasa a «Detener».
  - Se crea un `Arc<AtomicBool>` nuevo y se lanza un hilo con `analyze(Path::new(&raíz), &cancel, &mut |n| enviar Msg::Progress(n))`.
  - El hilo mide la duración con `Instant` y envía `Msg::Analyzed(Analysis, Duration)`.
  - Barra de estado: «Analizando… {count(n)} archivos».
- **Al pulsar «Detener»:**
  - Se hace `cancel.store(true)`.
- **Al recibir `Analyzed`:**
  - El resultado se guarda en `State`, y el botón vuelve a «Analizar».
  - Se rellena la lista con `top_dirs` o `top_files` según la opción elegida.
  - El resumen dice `Analizados {count(files_scanned)} archivos en {segundos} s`. Si se canceló, se añade ` (detenido)`.
  - Barra de estado: «Análisis terminado.».
- **Al cambiar entre las opciones:**
  - La lista se rellena de nuevo desde `State`, sin volver a analizar.
- **«Abrir ubicación» y doble clic en una fila** (`OnListViewDoubleClick`):
  - `open_in_explorer` con la ruta de la fila seleccionada.
  - Si no hay ninguna, no hace nada.

- [ ] **Step 1: Implementa la pestaña.**

- [ ] **Step 2: Comprobación.**
  - El build de release de Windows termina sin avisos.
  - El `.exe` ocupa menos de 3 000 000 bytes.
  - `cargo test` pasa.

- [ ] **Step 3: Commit** con el mensaje `Pestaña de análisis del top 50`.

---

### Task 9: README y licencia

**Files:**
- Create: `LICENSE`
- Modify: `README.md`

- [ ] **Step 1: `LICENSE`:** texto MIT estándar con `Copyright (c) 2026 9n-dev`.

- [ ] **Step 2: Reescribe `README.md` en español.** Mantén la cabecera centrada y el estilo de insignias que ya tiene. Secciones:
  - **Qué es:** el mismo texto, ajustado a la v1.
  - **Funciones:**
    - Análisis.
    - Las 9 categorías, cada una con su ruta y su regla de antigüedad.
  - **Seguridad:** lo que nunca se toca, el comportamiento con los enlaces y los archivos en uso, y la confirmación.
  - **Uso:**
    - Descargar o compilar.
    - Ejecutar `wincleaner.exe` sin instalar.
    - «Reiniciar como administrador».
    - Una nota: si se eleva con otra cuenta, se limpian el perfil y la papelera de esa cuenta.
    - Otra nota: las miniaturas suelen estar bloqueadas por el Explorador.
  - **Compilar:**
    - Requisitos en Linux: `rustup target add x86_64-pc-windows-gnu` y mingw-w64 (`x86_64-w64-mingw32-gcc` y `windres`).
    - Comandos `cargo test` y `cargo build --release --target x86_64-pc-windows-gnu`.
    - Ruta del `.exe`.
    - En Windows: `cargo build --release`.
  - **Hoja de ruta:** la v1 marcada. Las ideas de fuera de alcance se citan como no previstas.
  - **Licencia:** MIT.
  - **Autor:** se mantiene.

- [ ] **Step 3: Comprobación final.**
  - `cargo test` pasa.
  - El build de release de Windows termina sin avisos.
  - El `.exe` ocupa menos de 3 MB.

- [ ] **Step 4: Commit** con el mensaje `README y licencia MIT`.
