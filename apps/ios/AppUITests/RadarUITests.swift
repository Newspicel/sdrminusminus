import XCTest

@MainActor
final class RadarUITests: XCTestCase {
    func testTrackRows() {
        let app = MissionLaunch.open("radar", row: "radar-1")
        let first = MissionLaunch.element("radar.track.1", in: app)
        XCTAssertTrue(first.waitForExistence(timeout: Launch.timeout))
        XCTAssertTrue(first.label.contains("T01"), first.label)
        XCTAssertTrue(first.label.contains("137\u{00B0}"), first.label)
        let echoes = app.staticTexts["radar.echoes"]
        Launch.reveal(echoes, in: app)
        XCTAssertEqual(echoes.label, "6 echoes")
        XCTAssertFalse(MissionLaunch.element("radar.empty", in: app).exists)
    }

    func testImageShows() {
        let app = MissionLaunch.open("radar", row: "radar-1")
        XCTAssertTrue(MissionLaunch.element("radar.image", in: app).waitForExistence(timeout: Launch.timeout))
        XCTAssertFalse(MissionLaunch.element("radar.noimage", in: app).exists)
        XCTAssertFalse(MissionLaunch.element("radar.stale", in: app).exists)
        XCTAssertTrue(app.staticTexts["60 km"].exists)
        XCTAssertTrue(app.staticTexts["+200 Hz"].exists)
    }
}
