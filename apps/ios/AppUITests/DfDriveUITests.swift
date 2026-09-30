import CoreLocation
import XCTest

@MainActor
enum Drive {
    static let origin = CLLocation(latitude: 52.5200, longitude: 13.4050)

    static func app(routes: Bool = false) -> XCUIApplication {
        XCUIDevice.shared.location = XCUILocation(location: origin)
        let app = XCUIApplication()
        app.launchEnvironment["SDRMM_FAKE_CORE"] = "df"
        app.launchEnvironment["SDRMM_UITEST"] = "1"
        app.launchEnvironment["SDRMM_UITEST_STATIC"] = "1"
        if routes {
            app.launchEnvironment["SDRMM_FAKE_ROUTES"] = "1"
        }
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_DE"]
        app.launch()
        return app
    }

    static func openDf(_ app: XCUIApplication) {
        let row = app.buttons["missions.row.df-1"]
        XCTAssertTrue(row.waitForExistence(timeout: Launch.timeout))
        row.tap()
        allowAlerts()
        XCTAssertTrue(app.staticTexts["df.bearing"].waitForExistence(timeout: Launch.timeout))
    }

    static func keep(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        XCTContext.runActivity(named: name) { $0.add(shot) }
    }

    static func allowAlerts() {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        for _ in 0..<3 {
            let alert = springboard.alerts.firstMatch
            guard alert.waitForExistence(timeout: 3) else {
                return
            }
            let allow = ["Allow While Using App", "Allow Once", "Allow"].map { alert.buttons[$0] }.first {
                $0.exists
            }
            guard let allow else {
                return
            }
            allow.tap()
        }
    }
}

@MainActor
final class DfDriveUITests: XCTestCase {
    func testRoseAndGuidance() {
        let app = Drive.app()
        Drive.openDf(app)
        XCTAssertEqual(app.staticTexts["df.bearing"].label, "047\u{00B0}")
        XCTAssertEqual(app.staticTexts["df.guidance"].label, "Cross 215\u{00B0} \u{00B7} 1.2 km")
        XCTAssertEqual(app.staticTexts["df.confidence"].label, "62%")
        XCTAssertFalse(app.staticTexts["df.northup"].exists)
        XCTAssertTrue(app.buttons["df.navigate"].isEnabled)
        Drive.keep(app, "df")
        app.buttons["df.fit"].tap()
        RunLoop.current.run(until: Date().addingTimeInterval(4))
        Drive.keep(app, "df fit")
    }

    func testClearAsksConfirmation() {
        let app = Drive.app()
        Drive.openDf(app)
        let clear = app.buttons["df.clear"]
        XCTAssertTrue(clear.waitForExistence(timeout: Launch.timeout))
        clear.tap()
        XCTAssertTrue(app.staticTexts["Clear fusion?"].waitForExistence(timeout: Launch.timeout))
        let cancel = app.buttons["Cancel"]
        if cancel.exists {
            cancel.tap()
        } else {
            app.otherElements["PopoverDismissRegion"].tap()
        }
        XCTAssertTrue(app.staticTexts["Clear fusion?"].waitForNonExistence(timeout: Launch.timeout))
    }
}
