import XCTest

/// Photos → Share → DropBeam (share extension) → pick a recipient → DropBeam opens and sends.
/// Env (via TEST_RUNNER_ prefix): SHARE_TO = recipient name (default "Ashton's MacBook Pro").
final class ShareExtensionTests: XCTestCase {
    let photos = XCUIApplication(bundleIdentifier: "com.apple.mobileslideshow")

    /// Opens the newest photo in Photos and brings up its share sheet.
    func openShareSheet() {
        photos.terminate()
        photos.launch()
        sleep(3)
        for label in ["Continue", "Not Now", "Allow Full Access"] where photos.buttons[label].exists { photos.buttons[label].tap(); sleep(2) }
        shot("share-01-photos")
        try? photos.debugDescription.write(toFile: "/tmp/shots/photos-tree.txt", atomically: true, encoding: .utf8)
        // Newest photo = last photo cell of the library grid.
        let cells = photos.images.matching(NSPredicate(format: "identifier == 'PXGGridLayout-Info' AND label BEGINSWITH 'Photo'"))
        XCTAssertTrue(cells.firstMatch.waitForExistence(timeout: 10))
        cells.element(boundBy: cells.count - 1).tap()
        sleep(2); shot("share-02-photo")
        let share = photos.buttons["Share"].firstMatch
        XCTAssertTrue(share.waitForExistence(timeout: 10))
        share.tap()
        sleep(3); shot("share-03-sheet")
    }

    /// Finds DropBeam in the share sheet's app row (scrolling it / using More if needed).
    func tapDropBeam() {
        let sheet = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        func candidate() -> XCUIElement? {
            for app in [photos, sheet] {
                let cell = app.cells.matching(NSPredicate(format: "label == 'DropBeam'")).firstMatch
                if cell.exists && cell.isHittable { return cell }
                let button = app.buttons.matching(NSPredicate(format: "label == 'DropBeam'")).firstMatch
                if button.exists && button.isHittable { return button }
            }
            return nil
        }
        for _ in 0..<6 {
            if let c = candidate() { c.tap(); return }
            // The app row scrolls horizontally.
            let row = photos.collectionViews.firstMatch
            row.swipeLeft()
            sleep(1)
        }
        shot("share-03b-notfound")
        let more = photos.buttons["More"].firstMatch
        if more.exists { more.tap(); sleep(2); shot("share-03c-more") }
        XCTFail("DropBeam not in share sheet")
    }

    func testShareToFriend() throws {
        let target = ProcessInfo.processInfo.environment["SHARE_TO"] ?? "Ashton's MacBook Pro"
        openShareSheet()
        tapDropBeam()
        let recipient = photos.buttons["Send to \(target)"].firstMatch
        XCTAssertTrue(recipient.waitForExistence(timeout: 15))
        sleep(2); shot("share-04-extension")
        recipient.tap()
        sleep(1); shot("share-05-tapped")
        // iOS asks "Open in “DropBeam”?" before an extension brings its app forward.
        for host in [XCUIApplication(bundleIdentifier: "com.apple.springboard"), photos] {
            let open = host.buttons["Open"].firstMatch
            if open.waitForExistence(timeout: 3) { open.tap(); break }
        }
        let app = XCUIApplication(bundleIdentifier: appID)
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 20))
        sleep(3); shot("share-06-app")
        sleep(6); shot("share-07-app-later")
    }

    func testCancel() throws {
        openShareSheet()
        tapDropBeam()
        let cancel = photos.buttons["Cancel"].firstMatch
        XCTAssertTrue(cancel.waitForExistence(timeout: 15))
        sleep(2); shot("share-cancel-01")
        cancel.tap()
        sleep(2); shot("share-cancel-02")
    }

    /// Cold launch: DropBeam not running. 3 items (incl. a video) → Quick Send.
    func testColdMultiQuickSend() throws {
        XCUIApplication(bundleIdentifier: appID).terminate()
        photos.terminate(); photos.launch(); sleep(3)
        for label in ["Continue", "Not Now"] where photos.buttons[label].exists { photos.buttons[label].tap(); sleep(2) }
        let select = photos.buttons["Select"].firstMatch
        if !select.waitForExistence(timeout: 5) {
            try? photos.debugDescription.write(toFile: "/tmp/shots/multi-tree.txt", atomically: true, encoding: .utf8)
            // Photos restored its selection mode: leave it, then start fresh.
            photos.buttons.matching(NSPredicate(format: "label IN {'Back', 'Cancel', 'Close', 'Done'}")).firstMatch.tap(); sleep(1)
        }
        XCTAssertTrue(select.waitForExistence(timeout: 10)); select.tap(); sleep(1)
        // Grid cells (points, 3 columns of 134): the Big Sur still, the 2 s video, the waterfall.
        let origin = photos.coordinate(withNormalizedOffset: .zero)
        for (x, y) in [(201.0, 606.0), (201.0, 471.0), (335.0, 202.0)] {
            origin.withOffset(CGVector(dx: x, dy: y)).tap(); usleep(700_000)
        }
        sleep(1); shot("share-multi-01-selected")
        photos.buttons["Share"].firstMatch.tap(); sleep(3)
        tapDropBeam()
        let quick = photos.buttons.matching(NSPredicate(format: "label CONTAINS 'Quick Send'")).firstMatch
        XCTAssertTrue(quick.waitForExistence(timeout: 15))
        sleep(3); shot("share-multi-02-extension")
        // Scroll to Quick Send (below the friend list).
        for _ in 0..<4 where !quick.isHittable { photos.swipeUp() }
        sleep(1); shot("share-multi-03-bottom")
        quick.tap()
        sleep(1); shot("share-multi-03b-tapped")
        for host in [XCUIApplication(bundleIdentifier: "com.apple.springboard"), photos] {
            let open = host.buttons["Open"].firstMatch
            if open.waitForExistence(timeout: 5) { open.tap(); break }
        }
        let app = XCUIApplication(bundleIdentifier: appID)
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 20))
        sleep(8); shot("share-multi-04-app")
    }

    /// A send that fails in the app (fake friend with no address) explains why, then offers
    /// the normal Send To sheet with the same files so nothing is lost.
    func testFailedSendFallsBackToSendTo() throws {
        openShareSheet()
        tapDropBeam()
        let target = photos.buttons["Send to Sofia"].firstMatch
        XCTAssertTrue(target.waitForExistence(timeout: 15))
        target.tap()
        for host in [XCUIApplication(bundleIdentifier: "com.apple.springboard"), photos] {
            let open = host.buttons["Open"].firstMatch
            if open.waitForExistence(timeout: 3) { open.tap(); break }
        }
        let app = XCUIApplication(bundleIdentifier: appID)
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 20))
        let ok = app.alerts.buttons["OK"].firstMatch
        XCTAssertTrue(ok.waitForExistence(timeout: 20))
        shot("share-fail-01-alert")
        ok.tap()
        XCTAssertTrue(app.navigationBars["Send To"].waitForExistence(timeout: 10))
        sleep(1); shot("share-fail-02-sendto")
        app.buttons["Cancel"].firstMatch.tap()
    }
}
