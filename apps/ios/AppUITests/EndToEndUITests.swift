import XCTest

@MainActor
final class EndToEndUITests: XCTestCase {
    private let patience: TimeInterval = 30

    func testPairAndHuntAgainstServer() throws {
        guard let raw = ProcessInfo.processInfo.environment["SDRMM_E2E_LINK"] else {
            throw XCTSkip("Needs SDRMM_E2E_LINK from cargo xtask ios e2e")
        }
        let link = try XCTUnwrap(URL(string: raw))
        allowLocation()
        let app = XCUIApplication()
        app.launchEnvironment["SDRMM_E2E"] = "1"
        app.launchEnvironment["SDRMM_UITEST"] = "1"
        app.launch()
        XCTAssertTrue(app.navigationBars["Pair"].waitForExistence(timeout: patience))
        app.open(link)
        let trust = app.buttons["pair.trust"]
        XCTAssertTrue(trust.waitForExistence(timeout: patience))
        trust.tap()
        let hunt = app.buttons["missions.row.hunt"]
        XCTAssertTrue(hunt.waitForExistence(timeout: patience))
        XCTAssertTrue(hunt.label.contains("Fox"), hunt.label)
        hunt.tap()
        let run = app.buttons["hunt.run"]
        XCTAssertTrue(run.waitForLabel("Start", timeout: patience), run.label)
        run.tap()
        XCTAssertTrue(run.waitForLabel("Stop", timeout: patience), run.label)
        let trend = app.staticTexts["hunt.trend"]
        XCTAssertTrue(trend.waitForExistence(timeout: patience))
        XCTAssertTrue(trend.waitForLabel(timeout: 10) { $0 != "Listening" }, trend.label)
        run.tap()
        XCTAssertTrue(run.waitForLabel("Start", timeout: patience), run.label)
    }

    private func allowLocation() {
        addUIInterruptionMonitor(withDescription: "Location") { alert in
            for label in ["Allow While Using App", "Allow Once", "Allow"] {
                let button = alert.buttons[label]
                if button.exists {
                    button.tap()
                    return true
                }
            }
            return false
        }
    }
}
