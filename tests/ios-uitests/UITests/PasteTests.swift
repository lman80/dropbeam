import XCTest
final class PasteTests: XCTestCase {
    func testPasteWholeMessage() throws {
        let app = launch([])
        XCTAssertTrue(app.buttons["Friends"].firstMatch.waitForExistence(timeout: 15)); sleep(2)
        app.buttons["Friends"].firstMatch.tap(); sleep(1)
        app.buttons["Add friend or folder"].tap(); sleep(1)
        app.buttons["Add Friend"].tap(); sleep(2)
        app.buttons["Paste"].tap(); sleep(5); shot("paste-added")
    }
}
