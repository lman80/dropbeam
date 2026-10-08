import XCTest
/// SuperFeedback (iOS widget) placement + a real test report. Screenshots: /tmp/shots/ui-sf-*.png
final class SuperFeedbackTests: XCTestCase {
    private func feedbackButton(_ app: XCUIApplication) -> XCUIElement { app.buttons["Send feedback"].firstMatch }
    /// Answers the notification prompt and skips first-run setup if either shows up.
    private func settle(_ app: XCUIApplication) {
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Allow"]
        if allow.waitForExistence(timeout: 4) { allow.tap(); sleep(1) }
        if app.buttons["Get Started"].waitForExistence(timeout: 2) { app.buttons["Get Started"].tap(); sleep(1); app.buttons["Continue"].tap(); sleep(2); app.buttons["Skip setup"].tap(); sleep(2) }
    }

    /// Button stays over the Send To menu and over a full sheet (raised above its bottom).
    func testStaysOverMenusAndSheets() throws {
        let app = launch(["-feedbackPosition", "right,1", "-openTab", "send"])
        settle(app)
        _ = app.staticTexts["Send To"].firstMatch.waitForExistence(timeout: 20); sleep(2)
        shot("sf-1-home")
        let person = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Send to '")).firstMatch
        XCTAssertTrue(person.waitForExistence(timeout: 10))
        person.tap(); sleep(2); shot("sf-2-person-menu")
        XCTAssertTrue(feedbackButton(app).exists, "button must stay over the menu")
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.15)).tap(); sleep(1)
        app.buttons["Add a friend"].firstMatch.tap(); sleep(3); shot("sf-3-sheet")
        XCTAssertTrue(feedbackButton(app).exists, "button must stay over sheets")
    }

    /// Settings rows, Ideas tab, and one real report from the chat screen after the keyboard was used.
    func testSettingsIdeasAndSubmit() throws {
        let app = launch(["-feedbackPosition", "right,0.5", "-openTab", "settings", "-scrollToFeedback"])
        settle(app)
        let ideas = app.buttons["Ideas & Roadmap"].firstMatch
        XCTAssertTrue(ideas.waitForExistence(timeout: 15))
        sleep(2); shot("sf-4-settings")
        ideas.tap(); sleep(4); shot("sf-5-ideas")
    }

    func testSubmitFromChat() throws {
        // The keyboard comes up in the chat first (it used to turn every later screenshot black),
        // then -feedbackCapture opens the panel the way the button does.
        let chat = launch(["-feedbackPosition", "right,1", "-openChat", "1d7ae19f-268c-55e5-a4af-bc28ae92f6fd", "-focusComposer", "-feedbackCapture", "10"])
        settle(chat)
        sleep(5); shot("sf-7-typing")
        let send = chat.buttons["Send"].firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 15)); sleep(2)
        let field = chat.textViews.firstMatch.exists ? chat.textViews.firstMatch : chat.textFields.element(boundBy: chat.textFields.count - 1)
        field.tap(); sleep(1)
        chat.typeText("[TEST] SuperFeedback 3.3.1 update check from the DropBeam iOS simulator. Please ignore; closing.")
        sleep(1); shot("sf-8-panel")
        send.tap()
        sleep(8); shot("sf-9-sent")
    }
}
