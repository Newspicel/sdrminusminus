import XCTest

@MainActor
final class SurveyUITests: XCTestCase {
    func testTrailAndClear() {
        let app = MissionLaunch.open("survey", row: "survey-1")
        let clear = app.buttons["survey.clear"]
        XCTAssertTrue(clear.waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(app.staticTexts["survey.freq"].waitForLabel("433.920 MHz"))
        clear.tap()
        app.buttons["survey.fit"].tap()
        XCTAssertTrue(clear.exists)
        XCTAssertFalse(MissionLaunch.element("survey.trimmed", in: app).exists)
        XCTAssertFalse(app.otherElements["banner"].exists)
    }

    func testRecordAndStop() {
        let app = MissionLaunch.open("survey", row: "survey-1")
        let run = app.buttons["survey.run"]
        XCTAssertTrue(run.waitForLabel("Stop"), run.label)
        run.tap()
        XCTAssertTrue(run.waitForLabel("Record"), run.label)
        run.tap()
        XCTAssertTrue(run.waitForLabel("Stop"), run.label)
    }
}
