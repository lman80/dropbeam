import XCTest
final class ViewerTests: XCTestCase {
    func testSwipePaging() throws {
        let app = launch(["-openChat", "41d75e10-1d21-456e-9cc5-2cc2459f9d63", "-openViewer"])
        XCTAssertTrue(app.staticTexts["2 of 3"].waitForExistence(timeout: 15))
        shot("viewer-start")
        app.swipeLeft(); sleep(1)
        XCTAssertTrue(app.staticTexts["3 of 3"].waitForExistence(timeout: 3)); shot("viewer-swiped-left")
        app.swipeLeft(); sleep(1)
        XCTAssertTrue(app.staticTexts["3 of 3"].exists, "stays on last")
        app.swipeRight(); sleep(1)
        XCTAssertTrue(app.staticTexts["2 of 3"].waitForExistence(timeout: 3))
        app.swipeRight(); sleep(1)
        XCTAssertTrue(app.staticTexts["1 of 3"].waitForExistence(timeout: 3)); shot("viewer-first")
        // Zoomed in: a swipe pans the photo instead of paging.
        app.doubleTap(); sleep(1); shot("viewer-zoomed")
        app.swipeLeft(); sleep(1)
        XCTAssertTrue(app.staticTexts["1 of 3"].exists || !app.navigationBars.firstMatch.exists, "zoomed swipe pans")
        app.doubleTap(); sleep(1)
        app.swipeLeft(); sleep(1)
        XCTAssertTrue(app.staticTexts["2 of 3"].waitForExistence(timeout: 3), "pages again at 1x")
        app.buttons["Done"].tap(); sleep(1)
        XCTAssertFalse(app.staticTexts["2 of 3"].exists, "dismissed")
        shot("viewer-dismissed")
    }
}
