import CoreLocation
import XCTest

@MainActor
final class NavigationUITests: XCTestCase {
    private func startNavigation() -> XCUIApplication {
        let app = Drive.app(routes: true)
        Drive.openDf(app)
        app.buttons["df.navigate"].tap()
        let ok = app.buttons["nav.notice.ok"]
        if ok.waitForExistence(timeout: 3) {
            ok.tap()
        }
        return app
    }

    private func banner(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["nav.banner"]
    }

    func testNavigateShowsBannerAndEnd() {
        let app = startNavigation()
        XCTAssertTrue(banner(app).waitForExistence(timeout: 15))
        XCTAssertTrue(app.staticTexts["nav.summary"].exists)
        Drive.keep(app, "navigation")
        app.buttons["nav.end"].tap()
        XCTAssertTrue(app.staticTexts["df.bearing"].waitForExistence(timeout: Launch.timeout))
        XCTAssertFalse(banner(app).exists)
    }

    func testLocationDrivesProgress() {
        let app = startNavigation()
        let element = banner(app)
        XCTAssertTrue(element.waitForExistence(timeout: 15))
        let before = element.label
        XCTAssertTrue(before.contains("600 m"), before)
        XCUIDevice.shared.location = XCUILocation(location: CLLocation(latitude: 52.5227, longitude: 13.4050))
        XCTAssertTrue(eventually(15) { !element.label.contains("600 m") }, element.label)
        XCTAssertTrue(element.label.contains("300 m"), element.label)
    }

    private func eventually(_ seconds: TimeInterval, _ check: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if check() {
                return true
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        }
        return check()
    }
}
