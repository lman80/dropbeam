import XCTest
let appID = "com.ashtonmiller.dropbeam"
func shot(_ name: String) {
    let data = XCUIScreen.main.screenshot().pngRepresentation
    try? data.write(to: URL(fileURLWithPath: "/tmp/shots/ui-\(name).png"))
}
func launch(_ args: [String]) -> XCUIApplication {
    let app = XCUIApplication(bundleIdentifier: appID)
    app.terminate()
    app.launchArguments = args
    app.launch()
    return app
}
