fn main() {
    // Expose the build target triple so the app can locate its bundled croc
    // sidecar binary during development (src-tauri/binaries/croc-<triple>).
    if let Ok(triple) = std::env::var("TARGET") {
        println!("cargo:rustc-env=DROPBEAM_TARGET_TRIPLE={triple}");
    }
    tauri_build::build()
}
