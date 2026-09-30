import XCTest

@MainActor
final class MissionsUITests: XCTestCase {
    func testSectionsAndBlocker() {
        let app = Launch.app(scenario: "paired")
        XCTAssertTrue(app.buttons["missions.row.hunt-1"].waitForExistence(timeout: Launch.timeout))
        for header in ["Hunt", "DF drive", "Radar", "Survey"] {
            XCTAssertTrue(app.staticTexts[header].exists, header)
        }
        let spare = app.buttons["missions.row.df-2"]
        XCTAssertTrue(spare.exists)
        XCTAssertTrue(spare.label.contains("No array"), spare.label)
        XCTAssertTrue(app.buttons["missions.link"].label.contains("Online"))
    }

    func testWorkspaceSwitchAsksFirst() {
        let app = Launch.app(scenario: "paired")
        let menu = app.buttons["missions.workspace"]
        XCTAssertTrue(menu.waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(menu.label.contains("Field"), menu.label)
        menu.tap()
        let lab = app.buttons["Lab"]
        XCTAssertTrue(lab.waitForExistence(timeout: Launch.timeout))
        lab.tap()
        XCTAssertTrue(app.staticTexts["Switch workspace?"].waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(app.buttons["Switch"].exists)
        let cancel = app.buttons["Cancel"]
        if cancel.exists {
            cancel.tap()
        } else {
            app.otherElements["PopoverDismissRegion"].tap()
        }
        XCTAssertTrue(app.staticTexts["Switch workspace?"].waitForNonExistence(timeout: Launch.timeout))
        XCTAssertTrue(menu.label.contains("Field"), menu.label)
    }
}
