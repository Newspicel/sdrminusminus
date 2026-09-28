import XCTest

@MainActor
final class PairUITests: XCTestCase {
    private let link =
        "sdrmm://pair?h=10.0.0.2:8443&c=48210937&fp=1a2b3c4d1a2b3c4d1a2b3c4d1a2b3c4d1a2b3c4d1a2b3c4d1a2b3c4d1a2b3c4d&p=1"

    private func manual(_ app: XCUIApplication, code: String) {
        let address = app.textFields["pair.manual.address"]
        XCTAssertTrue(address.waitForExistence(timeout: Launch.timeout))
        address.tap()
        address.typeText("10.0.0.2:8443")
        let field = app.textFields["pair.manual.code"]
        field.tap()
        field.typeText(code)
        let submit = app.buttons["pair.manual.submit"]
        Launch.reveal(submit, in: app)
        submit.tap()
    }

    func testManualPairShowsMissions() {
        let app = Launch.app(scenario: "fresh")
        manual(app, code: "48210937")
        let trust = app.buttons["pair.trust"]
        XCTAssertTrue(trust.waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(app.staticTexts["Key 1A2B 3C4D 5E6F 7081 92A3"].exists)
        trust.tap()
        XCTAssertTrue(app.buttons["missions.row.hunt-1"].waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(app.buttons["missions.row.hunt-1"].label.contains("Fox 2m"))
    }

    func testWrongCodeShowsError() {
        let app = Launch.app(scenario: "fresh")
        manual(app, code: "00000000")
        let error = app.staticTexts["pair.error"]
        Launch.reveal(error, in: app)
        XCTAssertTrue(error.waitForExistence(timeout: Launch.timeout))
        XCTAssertEqual(error.label, "Wrong code")
    }

    func testDeepLinkAsksTrust() throws {
        let app = Launch.app(scenario: "fresh")
        XCTAssertTrue(app.navigationBars["Pair"].waitForExistence(timeout: Launch.timeout))
        app.open(try XCTUnwrap(URL(string: link)))
        XCTAssertTrue(app.staticTexts["Trust server?"].waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(app.buttons["pair.trust"].exists)
    }

    func testDemoOpensWithoutAServer() {
        let app = Launch.app(scenario: "fresh")
        let demo = app.buttons["pair.demo"]
        Launch.reveal(demo, in: app)
        demo.tap()
        XCTAssertTrue(app.buttons["missions.row.hunt-1"].waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(app.staticTexts["Demo"].exists)
        app.buttons["missions.demo.leave"].tap()
        XCTAssertTrue(app.navigationBars["Pair"].waitForExistence(timeout: Launch.timeout))
    }
}
