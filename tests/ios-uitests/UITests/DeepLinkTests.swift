import XCTest
final class DeepLinkTests: XCTestCase {
    func openLink(_ s: String) {
        XCUIDevice.shared.system.open(URL(string: s)!)
        let sb = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        if sb.buttons["Open"].waitForExistence(timeout: 5) { sb.buttons["Open"].tap() }
    }
    func testWarmAndCold() throws {
        let app = launch([])
        XCTAssertTrue(app.buttons["Friends"].firstMatch.waitForExistence(timeout: 15)); sleep(3)
        openLink("dropbeam:eyJ2IjoxLCJlaWQiOiJiYmJiMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAzIiwibmFtZSI6IkpvcmRhbiBLaW0ifQ")
        XCTAssertTrue(app.staticTexts["You added Jordan Kim"].waitForExistence(timeout: 10)); shot("link-warm")
        app.terminate(); sleep(1)
        openLink("dropbeam://add?code=dropbeam%3AeyJ2IjoxLCJlaWQiOiJiYmJiMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAzIiwibmFtZSI6IkpvcmRhbiBLaW0ifQ")
        XCTAssertTrue(app.staticTexts["You added Jordan Kim"].waitForExistence(timeout: 25)); sleep(1); shot("link-cold")
    }
}
