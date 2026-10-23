const COMMANDS: &[&str] = &["activate", "reply", "state", "event", "pick_folder", "pick_files"];

fn main() {
    // Direct `cargo check --target ...` does not get Xcode's deployment setting.
    // swift-rs otherwise forces iOS 13 even though this package requires iOS 17.
    // The Xcode-driven build does not forward IPHONEOS_DEPLOYMENT_TARGET to cargo,
    // and tauri-utils then defaults the Swift target to iOS 13; this package needs 17.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        std::env::set_var("IPHONEOS_DEPLOYMENT_TARGET", "17.0");
    }
    // Calls: WebRTC for iOS (ios/fetch-webrtc.sh; once, checksum-verified).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        let ok = std::process::Command::new("sh").arg("ios/fetch-webrtc.sh").status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "could not fetch WebRTC for iOS calls (ios/fetch-webrtc.sh)");
        // The app crate also builds a cdylib, which rustc links itself: point it at
        // the right slice. (Xcode links + embeds the framework for the app.)
        let slice = if std::env::var("TARGET").unwrap_or_default().ends_with("-sim") || std::env::var("TARGET").unwrap_or_default().starts_with("x86_64") {
            "ios-x86_64_arm64-simulator"
        } else {
            "ios-arm64"
        };
        let dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("ios/Frameworks/WebRTC.xcframework").join(slice);
        println!("cargo:rustc-link-search=framework={}", dir.display());
        println!("cargo:rustc-link-lib=framework=WebRTC");
        println!("cargo:rerun-if-changed=ios/fetch-webrtc.sh");
    }
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}
