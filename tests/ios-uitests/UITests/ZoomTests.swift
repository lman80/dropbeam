import XCTest
final class ZoomTests: XCTestCase {
    func testDoubleTap() throws {
        let app = launch(["-openChat", "41d75e10-1d21-456e-9cc5-2cc2459f9d63", "-openViewer"])
        XCTAssertTrue(app.staticTexts["2 of 3"].waitForExistence(timeout: 15))
        sleep(2)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).doubleTap(); sleep(1); shot("z-double")
    }
}
