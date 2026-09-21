const COMMANDS: &[&str] = &["activate", "reply", "state", "event"];

fn main() {
    // Direct `cargo check --target ...` does not get Xcode's deployment setting.
    // swift-rs otherwise forces iOS 13 even though this package requires iOS 17.
    // The Xcode-driven build does not forward IPHONEOS_DEPLOYMENT_TARGET to cargo,
    // and tauri-utils then defaults the Swift target to iOS 13; this package needs 17.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        std::env::set_var("IPHONEOS_DEPLOYMENT_TARGET", "17.0");
    }
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}
