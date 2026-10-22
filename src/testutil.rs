use std::path::PathBuf;

/// Carpeta vacía y única para una prueba; borra la anterior si existía.
pub fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wincleaner-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear carpeta de prueba");
    dir
}
