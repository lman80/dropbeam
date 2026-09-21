// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "tauri-plugin-native-ui",
    platforms: [.iOS(.v17)],
    products: [.library(name: "tauri-plugin-native-ui", type: .static, targets: ["NativeUIPlugin"])],
    dependencies: [.package(name: "Tauri", path: "./.tauri/tauri-api")],
    targets: [.target(name: "NativeUIPlugin", dependencies: [.byName(name: "Tauri")])]
)
