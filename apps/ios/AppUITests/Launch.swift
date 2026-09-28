import XCTest

@MainActor
enum Launch {
    static let timeout: TimeInterval = 10

    static func app(scenario: String, routes: Bool = false, staticData: Bool = true) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["SDRMM_FAKE_CORE"] = scenario
        app.launchEnvironment["SDRMM_UITEST"] = "1"
        if routes {
            app.launchEnvironment["SDRMM_FAKE_ROUTES"] = "1"
        }
        if staticData {
            app.launchEnvironment["SDRMM_UITEST_STATIC"] = "1"
        }
        app.launch()
        return app
    }

    static func reveal(_ element: XCUIElement, in app: XCUIApplication, swipes: Int = 6) {
        var left = swipes
        while !(element.exists && element.isHittable), left > 0 {
            app.swipeUp()
            left -= 1
        }
    }

    static func openSettings(_ app: XCUIApplication) {
        let gear = app.buttons["missions.settings"]
        XCTAssertTrue(gear.waitForExistence(timeout: timeout))
        gear.tap()
        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: timeout))
    }
}
