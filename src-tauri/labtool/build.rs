// Windows: the app library pulls in tao/tauri, which import comctl32 v6-only
// functions (TaskDialogIndirect, SetWindowSubclass…). Without an application
// manifest selecting Common-Controls 6.0 the loader binds comctl32 v5.82 and the
// exe dies before main with 0xC0000139 (STATUS_ENTRYPOINT_NOT_FOUND). The app
// gets this manifest from tauri-build; this console tool asks the MSVC linker
// to embed the same dependency.
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") && target.contains("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
