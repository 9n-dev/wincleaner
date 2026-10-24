# Parche de WinCleaner
Copia exacta de `i-slint-backend-winit` 1.18.1; solo cambia `Cargo.toml`: la dependencia `i-slint-core` pierde las features `image-decoders` y `svg`.
Motivo: la app solo crea imágenes desde píxeles y dibuja iconos con `Path`, y esos decodificadores añadían unos 700 KB al `.exe` (límite de 8 MiB).
Consecuencia: en la interfaz no puede haber `@image-url`, ni PNG ni SVG (tampoco `AboutSlint`, cuyo logo es un SVG que se decodifica al ejecutar). Las imágenes salen de `SharedPixelBuffer` y los iconos son `Path`.
Versión: `slint = "=1.18.1"` en `Cargo.toml` tiene que coincidir con la de esta carpeta. Si divergen, cargo avisa con `warning: Patch … was not used`.
Actualizar: al subir Slint, copiar el crate nuevo del registro de cargo a esta carpeta, repetir el cambio del `Cargo.toml` y subir la versión en `[patch.crates-io]` y en las dependencias `slint` y `slint-build`.
