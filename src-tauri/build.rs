fn main() {
    // On Windows MSVC, tauri-build only embeds the Common-Controls v6 manifest
    // into the app binary, so `cargo test` executables die at load with
    // STATUS_ENTRYPOINT_NOT_FOUND (comctl32 v5 lacks TaskDialogIndirect).
    // Embed the same manifest through the linker for every target instead,
    // and turn tauri-build's copy off so the app doesn't get two.
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows/app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        let attrs = tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        tauri_build::try_build(attrs).expect("tauri-build failed");
    } else {
        tauri_build::build()
    }
}
