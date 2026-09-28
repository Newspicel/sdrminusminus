import CoreLocation
import SdrmmCore
import XCTest

@testable import SDRmm

@MainActor
final class SensorHubTests: XCTestCase {
    private func fix(accuracy: Double = 5) -> CLLocation {
        CLLocation(
            coordinate: CLLocationCoordinate2D(latitude: 52.52, longitude: 13.405),
            altitude: 40,
            horizontalAccuracy: accuracy,
            verticalAccuracy: 3,
            course: 90,
            courseAccuracy: 5,
            speed: 10,
            speedAccuracy: 1,
            timestamp: Date(timeIntervalSince1970: 1_000)
        )
    }

    func testFixForwardedToCoreAndNavigation() {
        let core = FakeCore(scenario: .fresh, ticking: false)
        let feeds = FakeFeeds()
        let hub = feeds.hub(core: core)
        var fixes: [CLLocation] = []
        hub.onFix = { fixes.append($0) }
        hub.start(profile: .walk)
        feeds.location.send(.fix(fix()))
        XCTAssertEqual(core.calls, [.location])
        XCTAssertEqual(fixes.count, 1)
        XCTAssertTrue(hub.status.running)
    }

    func testFixWithoutAccuracyIsDroppedAndCounted() {
        let core = FakeCore(scenario: .fresh, ticking: false)
        let feeds = FakeFeeds()
        let hub = feeds.hub(core: core)
        var fixes = 0
        hub.onFix = { _ in fixes += 1 }
        hub.start(profile: .drive)
        feeds.location.send(.fix(fix(accuracy: -1)))
        XCTAssertTrue(core.calls.isEmpty)
        XCTAssertEqual(fixes, 0)
        XCTAssertEqual(hub.droppedFixes, 1)
    }

    func testDeniedAccessShowsInStatus() {
        let feeds = FakeFeeds()
        let hub = feeds.hub(core: FakeCore(scenario: .fresh, ticking: false))
        feeds.location.send(.access(.denied))
        XCTAssertEqual(hub.status.access, .denied)
        hub.start(profile: .walk)
        feeds.location.send(.precise(false))
        feeds.location.send(.unavailable)
        XCTAssertFalse(hub.status.precise)
        XCTAssertEqual(hub.status.lastError, "No location")
    }

    func testStopStopsAllFeeds() {
        let feeds = FakeFeeds()
        let hub = feeds.hub(core: FakeCore(scenario: .fresh, ticking: false))
        hub.start(profile: .walk)
        hub.start(profile: .walk)
        XCTAssertEqual(feeds.location.started, [.walk])
        hub.stop()
        XCTAssertEqual(feeds.location.stops, 1)
        XCTAssertEqual(feeds.heading.stops, 1)
        XCTAssertEqual(feeds.motion.stops, 1)
        XCTAssertFalse(hub.status.running)
    }

    func testHeadingAndMotionReachTheCore() {
        let core = FakeCore(scenario: .fresh, ticking: false)
        let feeds = FakeFeeds()
        let hub = feeds.hub(core: core)
        hub.start(profile: .drive)
        feeds.heading.send(
            SampleMapping.heading(trueDeg: 10, magneticDeg: 8, accuracyDeg: 5, at: Date())
        )
        feeds.motion.send(motion())
        XCTAssertEqual(core.calls, [.heading, .motion])
        XCTAssertTrue(hub.status.headingAvailable)
        XCTAssertTrue(hub.status.motionAvailable)
    }

    func testMissingSensorsAreNotStarted() {
        let feeds = FakeFeeds()
        feeds.heading.available = false
        feeds.motion.available = false
        let hub = feeds.hub(core: FakeCore(scenario: .fresh, ticking: false))
        hub.start(profile: .walk)
        XCTAssertEqual(feeds.heading.starts, 0)
        XCTAssertEqual(feeds.motion.starts, 0)
        XCTAssertFalse(hub.status.headingAvailable)
        XCTAssertFalse(hub.status.motionAvailable)
    }

    func testMotionFailureStopsMotionAndShows() {
        let feeds = FakeFeeds()
        let hub = feeds.hub(core: FakeCore(scenario: .fresh, ticking: false))
        hub.start(profile: .walk)
        feeds.motion.fail("Motion off")
        XCTAssertEqual(feeds.motion.stops, 1)
        XCTAssertEqual(hub.status.lastError, "Motion off")
        feeds.location.send(.fix(fix()))
        XCTAssertEqual(hub.status.lastError, "Motion off")
    }

    func testFixClearsOnlyALocationError() {
        let feeds = FakeFeeds()
        let hub = feeds.hub(core: FakeCore(scenario: .fresh, ticking: false))
        hub.start(profile: .walk)
        feeds.location.send(.unavailable)
        XCTAssertEqual(hub.status.lastError, "No location")
        feeds.location.send(.fix(fix()))
        XCTAssertNil(hub.status.lastError)
        feeds.heading.fail("Heading off")
        feeds.location.send(.fix(fix()))
        XCTAssertEqual(hub.status.lastError, "Heading off")
    }

    private func motion() -> MotionSample {
        MotionSample(
            tUnixMs: 1,
            frame: .trueNorth,
            qw: 1,
            qx: 0,
            qy: 0,
            qz: 0,
            rotX: 0,
            rotY: 0,
            rotZ: 0,
            gravX: 0,
            gravY: 0,
            gravZ: -1,
            headingDeg: nil,
            magAccuracy: .high
        )
    }
}
