import XCTest
/// #44: chat header presence + offline note. Alex (QA fake friend) is never reachable;
/// the real Mac friend is online when the Mac app is running.
final class PresenceTests: XCTestCase {
    /// Finish first-run setup if the QA sim lost its "named" flag.
    func skipSetup(_ app: XCUIApplication) {
        guard app.buttons["Get Started"].waitForExistence(timeout: 4) else { return }
        app.buttons["Get Started"].tap()
        let field = app.textFields["Your name"]
        if field.waitForExistence(timeout: 5), (field.value as? String ?? "").isEmpty || field.value as? String == "Your name" { field.tap(); field.typeText("Ashton Miller") }
        app.buttons["Continue"].tap()
        for _ in 0..<5 {
            if app.buttons["Not Now"].waitForExistence(timeout: 4) { app.buttons["Not Now"].tap() }
            else if app.buttons["Continue"].exists { app.buttons["Continue"].tap() }
            else { break }
        }
        if app.buttons["Turn On Notifications"].waitForExistence(timeout: 2) {
            app.buttons["Turn On Notifications"].tap()
            let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Allow"]
            if allow.waitForExistence(timeout: 4) { allow.tap() }
        }
    }
    func testSetup() throws { let app = launch([]); skipSetup(app); sleep(2); shot("presence-0-setup") }
    func testOfflineThread() throws {
        let app = launch(["-openChat", "f81788a1-9444-424e-8690-060c7fbd350b"])
        _ = app.staticTexts["Alex Rivera"].waitForExistence(timeout: 8); shot("presence-1-connecting")
        XCTAssertTrue(app.staticTexts["chat.offlineNote"].waitForExistence(timeout: 12))
        let field = app.textFields["Message"].exists ? app.textFields["Message"] : app.textViews.firstMatch
        field.tap(); field.typeText("Are you around later?")
        app.buttons["Send message"].tap()
        sleep(3); app.swipeDown(); sleep(1); shot("presence-2-offline")
    }
    func testMacThread() throws {
        _ = launch(["-openChat", "41d75e10-1d21-456e-9cc5-2cc2459f9d63"])
        sleep(12); shot("presence-3-mac")
    }
    func testChatList() throws {
        _ = launch(["-openTab", "chat"])
        sleep(10); shot("presence-4-list")
    }
}
