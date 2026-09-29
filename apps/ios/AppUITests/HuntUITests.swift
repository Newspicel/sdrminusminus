import XCTest

@MainActor
extension XCUIElement {
    func waitForLabel(_ label: String, timeout: TimeInterval = Launch.timeout) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if exists, self.label == label {
                return true
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }
        return exists && self.label == label
    }
}

@MainActor
enum MissionLaunch {
    static func open(_ scenario: String, row: String) -> XCUIApplication {
        let app = Launch.app(scenario: scenario)
        let button = app.buttons["missions.row.\(row)"]
        XCTAssertTrue(button.waitForExistence(timeout: Launch.timeout))
        button.tap()
        return app
    }

    static func element(_ id: String, in app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: id).firstMatch
    }
}

@MainActor
final class HuntUITests: XCTestCase {
    func testStartShowsTrendAndStop() {
        let app = MissionLaunch.open("hunt", row: "hunt-1")
        let run = app.buttons["hunt.run"]
        XCTAssertTrue(run.waitForLabel("Start"), run.label)
        XCTAssertEqual(app.staticTexts["hunt.trend"].label, "Listening")
        run.tap()
        XCTAssertTrue(run.waitForLabel("Stop"), run.label)
        XCTAssertTrue(app.staticTexts["hunt.trend"].waitForLabel("Warmer"))
        XCTAssertEqual(MissionLaunch.element("hunt.meter", in: app).value as? String, "60 %")
        run.tap()
        XCTAssertTrue(run.waitForLabel("Start"), run.label)
    }

    func testTuneRejectsBadInput() {
        let app = MissionLaunch.open("hunt", row: "hunt-1")
        let tune = app.buttons["hunt.tune"]
        Launch.reveal(tune, in: app)
        tune.tap()
        let field = app.textFields["tune.field"]
        XCTAssertTrue(field.waitForExistence(timeout: Launch.timeout))
        field.tap()
        field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 12))
        field.typeText("abc")
        app.buttons["tune.set"].tap()
        XCTAssertTrue(app.staticTexts["tune.error"].waitForLabel("Bad frequency"))
    }

    func testSweepShowsPeakAndStops() {
        let app = MissionLaunch.open("hunt", row: "hunt-1")
        let sweep = app.buttons["hunt.sweep"]
        Launch.reveal(sweep, in: app)
        XCTAssertTrue(sweep.waitForLabel("Sweep"), sweep.label)
        sweep.tap()
        XCTAssertTrue(sweep.waitForLabel("Stop sweep"), sweep.label)
        XCTAssertTrue(app.staticTexts["hunt.sweep.peak"].waitForLabel("137\u{00B0}"))
        XCTAssertEqual(app.staticTexts["hunt.sweep.phase"].label, "Sweeping")
        XCTAssertTrue(MissionLaunch.element("hunt.sweep.rose", in: app).exists)
        XCTAssertTrue(app.buttons["hunt.run"].waitForLabel("Stop"))
        sweep.tap()
        XCTAssertTrue(sweep.waitForLabel("Sweep"), sweep.label)
        XCTAssertTrue(
            MissionLaunch.element("hunt.sweep.rose", in: app).waitForNonExistence(timeout: Launch.timeout)
        )
        app.buttons["hunt.mark"].tap()
        XCTAssertFalse(app.otherElements["banner"].exists)
    }
}
