import XCTest
final class OnboardingTests: XCTestCase {
    func testFlow() throws {
        let app = launch(["-resetOnboarding"])
        XCTAssertTrue(app.staticTexts["Welcome to DropBeam"].waitForExistence(timeout: 20))
        sleep(1); shot("ob-1-welcome")
        app.buttons["Get Started"].tap()
        let field = app.textFields["Your name"]
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        sleep(1)
        if let v = field.value as? String, !v.isEmpty, v != "Your name" { field.tap(); field.press(forDuration: 1.2); if app.menuItems["Select All"].waitForExistence(timeout: 2) { app.menuItems["Select All"].tap() }; field.typeText(XCUIKeyboardKey.delete.rawValue) }
        field.typeText("Ashton Miller")
        shot("ob-2-name")
        app.buttons["Continue"].tap()
        XCTAssertTrue(app.staticTexts["Add a profile photo"].waitForExistence(timeout: 8))
        sleep(1); shot("ob-3-photo")
        app.buttons["Choose Photo"].tap()
        sleep(3); shot("ob-3b-picker")
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.83, dy: 0.43)).tap()
        sleep(4); shot("ob-3c-photo-chosen")
        if app.buttons["Continue"].waitForExistence(timeout: 8) { app.buttons["Continue"].tap() } else { app.buttons["Not Now"].tap() }
        XCTAssertTrue(app.staticTexts["Do you have other devices?"].waitForExistence(timeout: 5))
        sleep(1); shot("ob-4-devices")
        app.buttons["Not Now"].tap()
        XCTAssertTrue(app.staticTexts["Add your first friend"].waitForExistence(timeout: 5))
        sleep(1); shot("ob-5-friend")
        app.buttons["Not Now"].tap()
        if app.staticTexts["Know when files arrive"].waitForExistence(timeout: 4) {
            sleep(1); shot("ob-6-notifications")
            app.buttons["Turn On Notifications"].tap()
            let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
            let allow = springboard.buttons["Allow"]
            if allow.waitForExistence(timeout: 5) { shot("ob-6b-prompt"); allow.tap() }
        }
        XCTAssertTrue(app.buttons["Photos & Videos"].waitForExistence(timeout: 8) || app.staticTexts["Send"].waitForExistence(timeout: 2))
        sleep(2); shot("ob-7-done")
    }
}
