import MapKit
import SdrmmCore
import SwiftUI
import XCTest

@testable import SDRmm

@MainActor
final class DfDriveModelTests: XCTestCase {
    func testNavigateStartsNavigation() async throws {
        let harness = DriveHarness()
        harness.openDf()
        harness.fix()
        let view = Fixtures.df()
        harness.model.apply(.df(view: view))
        XCTAssertTrue(harness.model.df.canNavigate)
        harness.model.df.navigate()
        await harness.model.navigation.settled()
        XCTAssertEqual(harness.routes.requests.count, 1)
        let target = try XCTUnwrap(view.target)
        XCTAssertEqual(harness.routes.requests.first?.1, target.at)
        XCTAssertEqual(harness.model.path, [.mission(FakeScenarios.dfID), .navigation])
        XCTAssertEqual(harness.model.navigation.activePlan?.name, "Route 1")
    }

    func testNavigateDisabledWithoutTarget() {
        let harness = DriveHarness()
        harness.openDf()
        harness.fix()
        harness.model.apply(.df(view: Fixtures.df(target: .some(nil))))
        XCTAssertFalse(harness.model.df.canNavigate)
        harness.model.df.navigate()
        XCTAssertEqual(harness.model.navigation.phase, .idle)
        XCTAssertEqual(harness.model.path, [.mission(FakeScenarios.dfID)])
    }

    func testClearNeedsConfirmation() async {
        let harness = DriveHarness()
        harness.openDf()
        harness.model.df.askClear()
        XCTAssertTrue(harness.model.df.confirmClear)
        XCTAssertFalse(harness.core.calls.contains(.send(.clearFusion)))
        await harness.model.df.clearFusion()
        XCTAssertFalse(harness.model.df.confirmClear)
        XCTAssertTrue(harness.core.calls.contains(.send(.clearFusion)))
    }

    func testCalibrateSendsCommand() async {
        let harness = DriveHarness()
        harness.openDf()
        await harness.model.df.calibrate()
        XCTAssertTrue(harness.core.calls.contains(.send(.calibrate)))
    }

    func testTargetModeAndTuneSend() async {
        let harness = DriveHarness()
        harness.openDf()
        await harness.model.df.setTargetMode(.direct)
        XCTAssertTrue(harness.core.calls.contains(.send(.setTargetMode(mode: .direct))))
        let error = await harness.model.df.tune(megahertz: "433,92")
        XCTAssertNil(error)
        XCTAssertTrue(harness.core.calls.contains(.send(.tune(hz: 433_920_000))))
        let bad = await harness.model.df.tune(megahertz: "abc")
        XCTAssertEqual(bad, "Bad frequency")
    }

    func testCommandErrorReachesBanner() async {
        let harness = DriveHarness()
        harness.openDf()
        harness.core.fail(next: .NotConnected)
        await harness.model.df.calibrate()
        XCTAssertEqual(harness.model.banner?.text, "Offline")
    }

    func testClearResultFollowsTheCommand() async {
        let harness = DriveHarness()
        harness.openDf()
        harness.core.fail(next: .NotConnected)
        let failed = await harness.model.df.clearFusion()
        XCTAssertFalse(failed)
        XCTAssertEqual(harness.model.banner?.text, "Offline")
        XCTAssertFalse(harness.model.df.busy)
        let cleared = await harness.model.df.clearFusion()
        XCTAssertTrue(cleared)
    }

    func testStateLabels() {
        let expected: [(DfState, String?)] = [
            (.waiting, "Waiting"),
            (.calibrating, "Calibrating"),
            (.phaseUnknown, "Phase unknown"),
            (.noHeading, "No heading"),
            (.squelched, "Squelched"),
            (.live, nil),
        ]
        let harness = DriveHarness()
        XCTAssertEqual(harness.model.df.stateLabel, "Waiting")
        for (state, label) in expected {
            harness.model.df.apply(Fixtures.df(state: state))
            XCTAssertEqual(harness.model.df.stateLabel, label)
        }
    }

    func testGuidanceText() {
        let harness = DriveHarness()
        let model = harness.model.df
        model.apply(Fixtures.df(guidance: Fixtures.guidance(.probe)))
        XCTAssertEqual(model.guidanceText, "Cross 215\u{00B0} \u{00B7} 1.2 km")
        model.apply(Fixtures.df(guidance: Fixtures.guidance(.estimate)))
        XCTAssertEqual(model.guidanceText, "Approach 1.2 km")
        model.apply(Fixtures.df(guidance: .some(nil)))
        XCTAssertEqual(model.guidanceText, "No guidance")
    }

    func testBearingAndConfidenceText() {
        let harness = DriveHarness()
        let model = harness.model.df
        model.apply(Fixtures.df())
        model.apply(pose: FakeScenarios.pose(heading: 90))
        XCTAssertEqual(model.bearingText, "047\u{00B0}")
        XCTAssertEqual(model.confidenceText, "62%")
        model.apply(Fixtures.df(state: .squelched))
        XCTAssertEqual(model.bearingText, "-")
        XCTAssertEqual(model.confidenceText, "-")
    }

    func testOverlayAppliesAtMostEveryHalfSecond() {
        let harness = DriveHarness()
        let model = harness.model.df
        var now = Date(timeIntervalSince1970: 1_000)
        model.now = { now }
        let first = Fixtures.df()
        model.apply(first)
        XCTAssertEqual(model.overlay, first.overlay)
        let thinner = DfOverlay(rays: [], stations: [], ellipse: [], heat: [])
        model.apply(Fixtures.df(overlay: thinner))
        XCTAssertEqual(model.overlay, first.overlay)
        now = now.addingTimeInterval(0.6)
        model.apply(Fixtures.df(overlay: thinner))
        XCTAssertEqual(model.overlay, thinner)
        XCTAssertFalse(model.heatAvailable)
    }

    func testMissionOpenResetsView() {
        let harness = DriveHarness()
        harness.model.df.apply(Fixtures.df())
        harness.openDf()
        XCTAssertNil(harness.model.df.view)
        XCTAssertEqual(harness.model.df.overlay, DfDriveModel.emptyOverlay)
    }

    func testRetargetShowsBannerAndTicks() {
        let harness = DriveHarness()
        harness.openDf()
        harness.fix()
        harness.model.apply(.retarget(notice: Fixtures.retarget(Fixtures.offset(Fixtures.origin, 90, 1_200))))
        XCTAssertEqual(harness.model.df.retargetTick, 1)
        XCTAssertEqual(harness.model.banner?.text, "New target 1.2 km")
        XCTAssertEqual(harness.model.banner?.level, .info)
    }

    func testDeniedAlertsShowInfoBanner() async {
        let harness = DriveHarness()
        harness.notifier.allow = false
        harness.openDf()
        for _ in 0..<20 where harness.model.banner == nil {
            await Task.yield()
        }
        XCTAssertEqual(harness.notifier.authorizations, 1)
        XCTAssertEqual(harness.model.banner?.text, "Alerts off")
    }

    func testRetargetInBackgroundPostsAlert() {
        let harness = DriveHarness()
        harness.openDf()
        harness.fix()
        harness.model.scene(.background)
        let notice = Fixtures.retarget(Fixtures.offset(Fixtures.origin, 90, 1_200))
        harness.model.apply(.retarget(notice: notice))
        XCTAssertEqual(harness.notifier.posted.map(\.1), ["1.2 km"])
        XCTAssertEqual(harness.notifier.posted.first?.0, notice)
    }

    func testRetargetTextNamesTheKind() {
        let probe = Fixtures.retarget(Fixtures.origin, kind: .probe)
        XCTAssertEqual(RetargetText.body(probe, distance: "1.2 km"), "1.2 km \u{00B7} Cross")
        let estimate = Fixtures.retarget(Fixtures.origin, kind: .estimate)
        XCTAssertEqual(RetargetText.body(estimate, distance: "300 m"), "300 m \u{00B7} Target")
        XCTAssertEqual(RetargetText.identifier(estimate), "retarget.\(FakeScenarios.dfID)")
    }

    func testRouteNoticeAcceptancePersists() {
        let defaults = TestDefaults.make()
        let settings = SettingsStore(defaults: defaults)
        XCTAssertFalse(settings.routeNoticeAccepted)
        settings.routeNoticeAccepted = true
        XCTAssertTrue(SettingsStore(defaults: defaults).routeNoticeAccepted)
    }

    func testFitCoversOverlay() {
        let harness = DriveHarness()
        let model = harness.model.df
        model.fitAll()
        XCTAssertEqual(model.camera, .userLocation(fallback: .automatic))
        model.apply(Fixtures.df())
        model.fitAll()
        XCTAssertNotNil(model.camera.rect)
    }
}
