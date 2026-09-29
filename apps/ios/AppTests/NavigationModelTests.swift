import MapKit
import SdrmmCore
import XCTest

@testable import SDRmm

@MainActor
final class NavigationModelTests: XCTestCase {
    @MainActor
    private struct Rig {
        let clock = TestClock()
        let speech = SpeechRecorder()
        let routes: FakeRouteProvider
        let settings: SettingsStore
        let navigation: NavigationModel
        let errors: ErrorBox

        init(plan: RoutePlan) {
            routes = FakeRouteProvider(plans: [plan])
            settings = SettingsStore(defaults: TestDefaults.make())
            settings.units = .metric
            let errors = ErrorBox()
            self.errors = errors
            navigation = NavigationModel(
                routes: routes,
                speech: speech,
                settings: settings,
                clock: clock,
                report: { errors.all.append($0) }
            )
        }

        func at(_ point: LatLon, speed: Double = 10, accuracy: Double = 5) {
            navigation.update(location: Fixtures.fix(point, speed: speed, accuracy: accuracy))
        }
    }

    private final class ErrorBox {
        var all: [Error] = []
    }

    private let origin = Fixtures.origin

    private func active(_ plan: RoutePlan = Fixtures.straightPlan()) async -> Rig {
        let rig = Rig(plan: plan)
        rig.at(origin)
        rig.navigation.start(to: Fixtures.target(Fixtures.offset(origin, 0, 1_000)))
        await rig.navigation.settled()
        return rig
    }

    func testStartRoutesAndBecomesActive() async {
        let rig = await active()
        XCTAssertEqual(rig.routes.requests.count, 1)
        XCTAssertEqual(rig.navigation.activePlan?.name, "Straight")
        XCTAssertEqual(rig.navigation.banner?.distance, "1.0 km")
        XCTAssertEqual(rig.navigation.summary?.remaining, "1.0 km")
        XCTAssertFalse(rig.navigation.isRerouting)
    }

    func testStartWaitsForAFix() async {
        let rig = Rig(plan: Fixtures.straightPlan())
        let target = Fixtures.target(Fixtures.offset(origin, 0, 1_000))
        rig.navigation.start(to: target)
        await rig.navigation.settled()
        XCTAssertEqual(rig.navigation.phase, .routing(target, .start))
        XCTAssertTrue(rig.routes.requests.isEmpty)
        rig.at(origin)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 1)
        XCTAssertNotNil(rig.navigation.activePlan)
    }

    func testRetargetRequestsRouteAndSpeaks() async {
        let rig = await active()
        rig.clock.advance(by: 20)
        let moved = Fixtures.offset(origin, 90, 1_200)
        rig.navigation.retarget(Fixtures.retarget(moved))
        XCTAssertEqual(rig.speech.spoken.last?.text, "New target, 1.2 kilometers")
        XCTAssertEqual(rig.speech.spoken.last?.urgent, true)
        XCTAssertTrue(rig.navigation.isRerouting)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        XCTAssertEqual(rig.routes.requests.last?.1, moved)
        XCTAssertEqual(rig.navigation.target?.at, moved)
        XCTAssertFalse(rig.navigation.isRerouting)
    }

    func testOffRouteNeedsThreeQualifyingFixes() async {
        let rig = await active()
        rig.clock.advance(by: 20)
        let off = Fixtures.offset(Fixtures.offset(origin, 0, 300), 90, 200)
        rig.at(off)
        rig.at(off)
        XCTAssertEqual(rig.routes.requests.count, 1)
        rig.at(off, accuracy: 100)
        rig.at(off, speed: 0)
        XCTAssertEqual(rig.routes.requests.count, 1)
        rig.at(off)
        XCTAssertTrue(rig.navigation.isRerouting)
        XCTAssertEqual(rig.speech.spoken.last?.text, "Rerouting")
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        XCTAssertEqual(rig.routes.requests.last?.0, off)
    }

    func testOnRouteFixResetsStreak() async {
        let rig = await active()
        rig.clock.advance(by: 20)
        let off = Fixtures.offset(Fixtures.offset(origin, 0, 300), 90, 200)
        rig.at(off)
        rig.at(off)
        rig.at(Fixtures.offset(origin, 0, 320))
        rig.at(off)
        rig.at(off)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 1)
    }

    func testRequestSpacingTenSeconds() async {
        let rig = await active()
        rig.clock.advance(by: 20)
        rig.navigation.retarget(Fixtures.retarget(Fixtures.offset(origin, 90, 500)))
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        rig.clock.advance(by: 2)
        let second = Fixtures.offset(origin, 90, 900)
        rig.navigation.retarget(Fixtures.retarget(second))
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        rig.clock.advance(by: 7)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        rig.clock.advance(by: 1)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 3)
        XCTAssertEqual(rig.routes.requests.last?.1, second)
    }

    func testThrottledBacksOff() async throws {
        let rig = Rig(plan: Fixtures.straightPlan())
        rig.at(origin)
        let target = Fixtures.target(Fixtures.offset(origin, 0, 1_000))
        var delays: [TimeInterval] = []
        rig.routes.fail(next: .throttled)
        rig.navigation.start(to: target)
        await rig.navigation.settled()
        for attempt in 1...5 {
            let retry = try XCTUnwrap(rig.navigation.retryAt)
            delays.append(retry.timeIntervalSince(rig.clock.current))
            XCTAssertEqual(rig.navigation.phase, .noRoute(target, .throttled(retryAt: retry)))
            XCTAssertEqual(rig.routes.requests.count, attempt)
            rig.routes.fail(next: .throttled)
            rig.clock.advance(to: retry)
            await rig.navigation.settled()
        }
        XCTAssertEqual(delays, [15, 30, 60, 120, 120])
        XCTAssertTrue(rig.errors.all.isEmpty)
    }

    func testRetargetWaitsOutBackoff() async throws {
        let rig = Rig(plan: Fixtures.straightPlan())
        rig.at(origin)
        rig.routes.fail(next: .throttled)
        rig.navigation.start(to: Fixtures.target(Fixtures.offset(origin, 0, 1_000)))
        await rig.navigation.settled()
        let retry = try XCTUnwrap(rig.navigation.retryAt)
        rig.clock.advance(by: 12)
        let moved = Fixtures.target(Fixtures.offset(origin, 90, 800))
        rig.navigation.retarget(Fixtures.retarget(moved.at))
        await rig.navigation.settled()
        XCTAssertEqual(rig.navigation.phase, .noRoute(moved, .throttled(retryAt: retry)))
        XCTAssertEqual(rig.routes.requests.count, 1)
        rig.clock.advance(to: retry)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        XCTAssertEqual(rig.routes.requests.last?.1, moved.at)
        XCTAssertEqual(rig.navigation.target, moved)
        XCTAssertNotNil(rig.navigation.activePlan)
    }

    func testMapKitErrorsMapToRouteErrors() {
        let mapKit = { (code: MKError.Code) in NSError(domain: MKErrorDomain, code: Int(code.rawValue)) }
        XCTAssertEqual(MapKitRouteProvider.map(mapKit(.loadingThrottled)), .throttled)
        XCTAssertEqual(MapKitRouteProvider.map(mapKit(.directionsNotFound)), .notFound)
        XCTAssertEqual(MapKitRouteProvider.map(mapKit(.placemarkNotFound)), .notFound)
        XCTAssertEqual(MapKitRouteProvider.map(mapKit(.serverFailure)), .network)
        XCTAssertEqual(MapKitRouteProvider.map(CancellationError()), .cancelled)
        let url = { (code: Int) in NSError(domain: NSURLErrorDomain, code: code) }
        XCTAssertEqual(MapKitRouteProvider.map(url(NSURLErrorCancelled)), .cancelled)
        XCTAssertEqual(MapKitRouteProvider.map(url(NSURLErrorNotConnectedToInternet)), .network)
        guard case .other = MapKitRouteProvider.map(mapKit(.decodingFailed)) else {
            return XCTFail("decoding failure is not a distinct route error")
        }
    }

    func testThrottledKeepsActivePlan() async {
        let rig = await active()
        rig.clock.advance(by: 20)
        rig.routes.fail(next: .throttled)
        rig.navigation.retarget(Fixtures.retarget(Fixtures.offset(origin, 90, 500)))
        await rig.navigation.settled()
        XCTAssertNotNil(rig.navigation.activePlan)
        XCTAssertTrue(rig.navigation.isRerouting)
        XCTAssertNotNil(rig.navigation.retryAt)
    }

    func testNotFoundGivesDirectGuidanceAndRetriesAfter500m() async {
        let rig = Rig(plan: Fixtures.straightPlan())
        rig.at(origin)
        let target = Fixtures.target(Fixtures.offset(origin, 90, 1_200))
        rig.routes.fail(next: .notFound)
        rig.navigation.start(to: target)
        await rig.navigation.settled()
        XCTAssertEqual(rig.navigation.phase, .noRoute(target, .notFound))
        XCTAssertEqual(rig.navigation.directGuidance, "Steer 090\u{00B0} \u{00B7} 1.2 km")
        rig.clock.advance(by: 60)
        rig.at(Fixtures.offset(origin, 0, 300))
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 1)
        rig.at(Fixtures.offset(origin, 0, 520))
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        XCTAssertNotNil(rig.navigation.activePlan)
    }

    func testNetworkFailureReportsOnceAndRetries() async {
        let rig = Rig(plan: Fixtures.straightPlan())
        rig.at(origin)
        let target = Fixtures.target(Fixtures.offset(origin, 0, 1_000))
        rig.routes.fail(next: .network)
        rig.navigation.start(to: target)
        await rig.navigation.settled()
        XCTAssertEqual(rig.navigation.phase, .noRoute(target, .network))
        XCTAssertEqual(rig.errors.all.count, 1)
        rig.routes.fail(next: .network)
        rig.clock.advance(by: 30)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        XCTAssertEqual(rig.errors.all.count, 1)
        rig.clock.advance(by: 30)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 3)
        XCTAssertNotNil(rig.navigation.activePlan)
    }

    func testArrival() async {
        let rig = await active()
        rig.at(Fixtures.offset(origin, 0, 985))
        XCTAssertEqual(rig.navigation.phase, .arrived(Fixtures.target(Fixtures.offset(origin, 0, 1_000))))
        XCTAssertEqual(rig.speech.spoken.last?.text, "Arrived")
        XCTAssertEqual(rig.speech.spoken.last?.urgent, true)
    }

    func testArrivedRetargetSetsPending() async {
        let rig = await active()
        rig.at(Fixtures.offset(origin, 0, 990))
        let next = Fixtures.target(Fixtures.offset(origin, 90, 300))
        rig.navigation.retarget(Fixtures.retarget(next.at))
        XCTAssertEqual(rig.navigation.pendingTarget, next)
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 1)
        rig.clock.advance(by: 20)
        rig.navigation.goToPending()
        await rig.navigation.settled()
        XCTAssertEqual(rig.routes.requests.count, 2)
        XCTAssertNil(rig.navigation.pendingTarget)
    }

    func testEndStopsSpeechAndCancels() async {
        let rig = await active()
        rig.navigation.end()
        XCTAssertEqual(rig.navigation.phase, .idle)
        XCTAssertGreaterThan(rig.routes.cancels, 0)
        XCTAssertEqual(rig.speech.stops, 1)
        XCTAssertNil(rig.navigation.banner)
    }

    func testPassedManeuverNotAnnouncedAgain() async {
        let plan = Fixtures.steppedPlan()
        let rig = await active(plan)
        let start = plan.steps[2].startM
        for along in [start - 45, start + 10, start + 2, start + 4, start - 3] {
            rig.at(Fixtures.offset(origin, 0, along), speed: 10)
        }
        let stepTwo = rig.speech.spoken.filter { $0.text.hasSuffix("Step 2") }
        XCTAssertEqual(stepTwo.map(\.text), ["In 200 meters, Step 2", "Step 2"])
        XCTAssertEqual(stepTwo.map(\.urgent), [false, true])
        XCTAssertEqual(rig.navigation.activePosition?.nextStep, 3)
        XCTAssertGreaterThan(rig.navigation.nearTick, 0)
    }

    func testStartFromCarPlayPlanIsActiveAtOnce() {
        let rig = Rig(plan: Fixtures.straightPlan())
        rig.at(origin)
        let plan = Fixtures.lPlan()
        rig.navigation.start(plan: plan, to: Fixtures.target(Fixtures.offset(origin, 0, 1_000)))
        XCTAssertEqual(rig.navigation.activePlan, plan)
        XCTAssertTrue(rig.routes.requests.isEmpty)
        XCTAssertEqual(rig.navigation.banner?.instruction, "Turn right")
    }
}
