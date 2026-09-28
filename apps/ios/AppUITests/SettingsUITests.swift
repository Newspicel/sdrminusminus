import XCTest

@MainActor
final class SettingsUITests: XCTestCase {
    func testForgetReturnsToPair() {
        let app = Launch.app(scenario: "paired")
        Launch.openSettings(app)
        let row = app.buttons["settings.server.s1"]
        XCTAssertTrue(row.waitForExistence(timeout: Launch.timeout))
        row.swipeLeft()
        app.buttons["Forget"].firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Forget server?"].waitForExistence(timeout: Launch.timeout))
        app.buttons["Forget"].firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Pair"].waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(app.textFields["pair.manual.address"].exists)
    }

    func testAlignShowsProgress() {
        let app = Launch.app(scenario: "paired")
        Launch.openSettings(app)
        let align = app.buttons["settings.align"]
        Launch.reveal(align, in: app)
        align.tap()
        XCTAssertTrue(app.staticTexts["Drive straight"].waitForExistence(timeout: Launch.timeout))
        let cancel = app.buttons["Cancel"]
        XCTAssertTrue(cancel.exists)
        cancel.tap()
        XCTAssertTrue(app.staticTexts["Drive straight"].waitForNonExistence(timeout: Launch.timeout))
    }
}
