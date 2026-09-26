import XCTest
final class InviteTests: XCTestCase {
    func testInviteAndAddFromPhoto() throws {
        let app = launch([])
        if app.buttons["Get Started"].waitForExistence(timeout: 12) { app.buttons["Get Started"].tap(); sleep(1); app.buttons["Continue"].tap(); sleep(2); app.buttons["Skip setup"].tap(); sleep(2) }
        XCTAssertTrue(app.buttons["Friends"].firstMatch.waitForExistence(timeout: 15)); sleep(2); app.buttons["Friends"].firstMatch.tap()
        let invite = app.buttons["Invite Friends"]
        XCTAssertTrue(invite.waitForExistence(timeout: 15))
        invite.tap(); sleep(3); shot("inv-1-card")
        app.buttons["Share"].tap(); sleep(3); shot("inv-2-sharesheet")
        // close share sheet
        if app.buttons["Close"].exists { app.buttons["Close"].tap() } else { app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.1)).tap() }
        sleep(2)
        app.buttons["Done"].tap(); sleep(1)
        app.buttons["Add friend or folder"].tap(); sleep(1)
        app.buttons["Add Friend"].tap(); sleep(3); shot("inv-3-addfriend")
        app.buttons["Scan from Photo"].tap(); sleep(3); shot("inv-4-picker")
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.17, dy: 0.43)).tap()
        sleep(4); shot("inv-5-added")
        sleep(6); shot("inv-6-waiting")
    }
}
