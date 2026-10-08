import XCTest
import UIKit

/// fix-ios checks (#71 #72 #79, Paste Image, Erase All Data). Needs the QA media files in
/// the app's Documents/qa-media (IMG_0001-3.jpg, IMG_0004.mov) — see the fix-ios notes.
final class FixIOSTests: XCTestCase {
    /// Fresh install: get through first-run setup (runs first: tests are alphabetical).
    func testA_Onboard() throws {
        let app = launch([])
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        if springboard.buttons["Allow"].waitForExistence(timeout: 8) { springboard.buttons["Allow"].tap() }
        guard app.buttons["Get Started"].waitForExistence(timeout: 10) else { return }
        app.buttons["Get Started"].tap()
        let field = app.textFields["Your name"]
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap(); field.typeText("Jamie Rivera")
        app.buttons["Continue"].firstMatch.tap(); sleep(2)
        for _ in 0..<4 {
            if app.buttons["Not Now"].firstMatch.waitForExistence(timeout: 4) { app.buttons["Not Now"].firstMatch.tap(); sleep(1) }
            if app.buttons["Turn On Notifications"].exists { app.buttons["Turn On Notifications"].tap(); sleep(1)
                if springboard.buttons["Allow"].waitForExistence(timeout: 4) { springboard.buttons["Allow"].tap() } }
        }
        sleep(2); shot("fix-onboarded")
    }
    /// Received album: one "Save All to Photos" button under the grid.
    func testSaveAllBubble() throws {
        let app = launch(["-previewMediaChat"])
        let saveAll = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Save all'")).firstMatch
        XCTAssertTrue(saveAll.waitForExistence(timeout: 20))
        sleep(2); shot("fix-chat-save-all")
        saveAll.tap(); sleep(2)
        acceptPhotosPrompt()
        sleep(3); shot("fix-chat-save-all-done")
    }

    /// Video page (#71): the player's controls sit below our Done/Share bar.
    func testVideoViewerLayout() throws {
        let app = launch(["-previewMediaChat", "-openViewerAt", "4"])
        XCTAssertTrue(app.staticTexts["4 of 4"].waitForExistence(timeout: 20))
        sleep(3); shot("fix-viewer-video")
        app.buttons["Save to Photos"].tap(); sleep(1); shot("fix-viewer-save-menu")
        XCTAssertTrue(app.buttons["Save All 4 to Photos"].exists)
        app.buttons["Save All 4 to Photos"].tap(); sleep(2)
        acceptPhotosPrompt()
        sleep(3); shot("fix-viewer-saved")
    }

    /// Photo page: screen-sized decode, still swipes and zooms.
    func testPhotoViewer() throws {
        let app = launch(["-previewMediaChat", "-openViewerAt", "1"])
        XCTAssertTrue(app.staticTexts["1 of 4"].waitForExistence(timeout: 20))
        sleep(2); shot("fix-viewer-photo")
        app.swipeLeft(); sleep(1)
        XCTAssertTrue(app.staticTexts["2 of 4"].waitForExistence(timeout: 3))
        app.doubleTap(); sleep(2); shot("fix-viewer-zoomed")
    }

    /// Paste Image uses PasteButton: no "Allow Paste" prompt, the image is staged.
    func testPasteImage() throws {
        let image = UIGraphicsImageRenderer(size: CGSize(width: 300, height: 200)).image { ctx in
            UIColor.systemTeal.setFill(); ctx.fill(CGRect(x: 0, y: 0, width: 300, height: 200))
            UIColor.white.setFill(); ctx.fill(CGRect(x: 100, y: 60, width: 100, height: 80))
        }
        UIPasteboard.general.image = image
        let app = launch(["-previewMediaChat"])
        let add = app.buttons["Add attachment"]
        XCTAssertTrue(add.waitForExistence(timeout: 20))
        sleep(2); add.tap(); sleep(1); shot("fix-paste-menu")
        let paste = app.buttons["Paste"].firstMatch
        XCTAssertTrue(paste.waitForExistence(timeout: 5))
        paste.tap(); sleep(3)
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        XCTAssertFalse(springboard.buttons["Allow Paste"].exists, "no paste prompt")
        shot("fix-paste-staged")
    }

    /// Settings → Privacy & Your Data → Erase All Data: confirmation, then a fresh start.
    func testZ_EraseAllData() throws {
        let app = launch(["-openTab", "settings", "-openPrivacy", "-showErase"])
        let erase = app.buttons["Erase All Data"].firstMatch
        XCTAssertTrue(erase.waitForExistence(timeout: 20))
        sleep(2); shot("fix-erase-confirm")
        // The dialog's destructive button (the last "Erase All Data").
        let buttons = app.buttons.matching(NSPredicate(format: "label == 'Erase All Data'"))
        buttons.element(boundBy: buttons.count - 1).tap()
        sleep(1); shot("fix-erase-progress")
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 90), "closes when done")
        app.launchArguments = []
        app.launch()
        sleep(8); shot("fix-erase-fresh-start")
    }

    private func acceptPhotosPrompt() {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        for label in ["Allow", "Allow Access", "OK", "Allow Full Access"] {
            let button = springboard.buttons[label]
            if button.waitForExistence(timeout: 2) { shot("fix-photos-prompt"); button.tap(); return }
        }
    }
}
