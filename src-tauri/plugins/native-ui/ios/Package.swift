// swift-tools-version:5.9
import Foundation
import PackageDescription

// Calls: Google's WebRTC (stasel/WebRTC M155, BSD-3-Clause), fetched by
// ios/fetch-webrtc.sh into ios/Frameworks. The plugin only needs its headers to
// compile (`import WebRTC`); the app target links and embeds the framework
// (gen/apple/project.yml). The headers are the same in every slice.
let here = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let webrtcHeaders = here.appendingPathComponent("Frameworks/WebRTC.xcframework/ios-arm64").path

let package = Package(
    name: "tauri-plugin-native-ui",
    platforms: [.iOS(.v17)],
    products: [.library(name: "tauri-plugin-native-ui", type: .static, targets: ["NativeUIPlugin"])],
    dependencies: [.package(name: "Tauri", path: "./.tauri/tauri-api")],
    targets: [.target(
        name: "NativeUIPlugin",
        dependencies: [.byName(name: "Tauri")],
        swiftSettings: [.unsafeFlags(["-F", webrtcHeaders])]
    )]
)
