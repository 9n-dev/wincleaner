# Parche de WinCleaner
Copia exacta de `i-slint-backend-winit` 1.18.1; solo cambia `Cargo.toml`: la dependencia `i-slint-core` pierde las features `image-decoders` y `svg`.
Motivo: la app solo crea imágenes desde píxeles y dibuja iconos con `Path`, y esos decodificadores añadían unos 700 KB al `.exe` (límite de 8 MiB).
Actualizar: al subir Slint, copiar el crate nuevo del registro de cargo a esta carpeta, repetir el cambio del `Cargo.toml` y subir la versión en `[patch.crates-io]`.
