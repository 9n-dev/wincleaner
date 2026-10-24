fn main() {
    let config = || slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    // Página de muestra para `examples/capturas.rs`. Va antes: la última llamada fija
    // `SLINT_INCLUDE_GENERATED`, que es lo que incluye `ui.rs`.
    if !windows {
        slint_build::compile_with_config("ui/muestra.slint", config()).unwrap();
    }
    slint_build::compile_with_config("ui/app.slint", config()).unwrap();
    if windows {
        embed_resource::compile("wincleaner.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
