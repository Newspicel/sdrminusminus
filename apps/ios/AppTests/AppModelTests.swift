import SdrmmCore
import XCTest

@testable import SDRmm

@MainActor
final class AppModelTests: XCTestCase {
    private func hunt(mission: String) -> HuntView {
        FakeScenarios.hunt(strength: 0.5, trend: .warmer, running: true).with(mission: mission)
    }

    func testRunConnectsToActiveServer() async {
        let harness = Harness()
        await harness.start(activeServer: "s1")
        XCTAssertTrue(harness.core.calls.contains(.connect("s1")))
        XCTAssertTrue(harness.core.calls.contains(.pose(harness.settings.poseSettings)))
        XCTAssertEqual(harness.model.servers.map(\.id), ["s1"])
    }

    func testLinkEventUpdatesStatus() {
        let harness = Harness()
        harness.model.apply(.link(state: .online(server: "Lab Pi")))
        XCTAssertEqual(harness.model.link, .online(server: "Lab Pi"))
    }

    func testMissionsGroupedInFixedOrder() {
        let harness = Harness()
        let all = FakeScenarios.missions()
        let shuffled = MissionsView(
            workspace: all.workspace,
            workspaces: all.workspaces,
            missions: all.missions.reversed().filter { $0.kind != .radarWatch }
        )
        harness.model.apply(.missions(view: shuffled))
        let groups = harness.model.groupedMissions
        XCTAssertEqual(groups.map(\.0), [.hunt, .dfDrive, .survey])
        XCTAssertEqual(groups[1].1.map(\.id), ["df-2", "df-1"])
    }

    func testMissionGoneClosesAndBanners() {
        let harness = Harness()
        let view = FakeScenarios.missions()
        harness.model.apply(.missions(view: view))
        harness.model.open(missionID: FakeScenarios.huntID)
        XCTAssertEqual(harness.model.openMission?.id, FakeScenarios.huntID)
        let without = MissionsView(
            workspace: view.workspace,
            workspaces: view.workspaces,
            missions: view.missions.filter { $0.id != FakeScenarios.huntID }
        )
        harness.model.apply(.missions(view: without))
        XCTAssertNil(harness.model.openMission)
        XCTAssertEqual(harness.model.banner?.text, "Mission gone")
        XCTAssertTrue(harness.core.calls.contains(.close))
    }

    func testRevokedForgetsServerAndShowsPair() async {
        let harness = Harness()
        await harness.start()
        XCTAssertFalse(harness.model.needsPairing)
        harness.model.apply(.link(state: .refused(reason: .revoked, text: "Removed on the server")))
        XCTAssertTrue(harness.core.calls.contains(.forget("s1")))
        XCTAssertTrue(harness.model.needsPairing)
        XCTAssertEqual(harness.model.banner?.text, "Phone removed")
        XCTAssertNil(harness.settings.activeServerID)
    }

    func testOtherRefusalsShowTheirLabel() {
        let harness = Harness()
        harness.model.apply(.link(state: .refused(reason: .serverTooOld, text: "protocol 1")))
        XCTAssertEqual(harness.model.banner?.text, "Server too old")
        XCTAssertEqual(harness.model.banner?.detail, "protocol 1")
    }

    func testDroppedEventsSurfaceBanner() async {
        let harness = Harness()
        harness.core.drop(3)
        await harness.start(activeServer: nil, events: [.link(state: .offline)])
        XCTAssertEqual(harness.model.banner?.text, "Missed 3 updates")
        XCTAssertEqual(harness.model.banner?.level, .warn)
    }

    func testEventForOtherMissionIgnored() {
        let harness = Harness()
        harness.model.apply(.missions(view: FakeScenarios.missions()))
        harness.model.open(missionID: FakeScenarios.huntID)
        harness.model.apply(.hunt(view: hunt(mission: "other")))
        XCTAssertNil(harness.model.hunt.view)
        harness.model.apply(.hunt(view: hunt(mission: FakeScenarios.huntID)))
        XCTAssertEqual(harness.model.hunt.view?.mission, FakeScenarios.huntID)
    }

    func testOpenAndCloseMissionDriveSensors() async {
        let harness = Harness()
        harness.model.apply(.missions(view: FakeScenarios.missions()))
        harness.model.open(missionID: FakeScenarios.huntID)
        XCTAssertEqual(harness.feeds.location.started, [.walk])
        XCTAssertTrue(harness.core.calls.contains(.open(FakeScenarios.huntID)))
        XCTAssertEqual(harness.model.path, [.mission(FakeScenarios.huntID)])
        await eventually { harness.session.active == [true] }
        XCTAssertEqual(harness.session.active, [true])
        harness.model.closeMission()
        await eventually { harness.session.active == [true, false] }
        XCTAssertEqual(harness.session.active, [true, false])
        XCTAssertEqual(harness.feeds.location.stops, 1)
        XCTAssertEqual(harness.feeds.heading.stops, 1)
        XCTAssertEqual(harness.feeds.motion.stops, 1)
        XCTAssertTrue(harness.core.calls.contains(.close))
        harness.model.open(missionID: FakeScenarios.dfID)
        XCTAssertEqual(harness.feeds.location.started, [.walk, .drive])
        harness.model.path = []
        XCTAssertNil(harness.model.openMission)
        XCTAssertEqual(harness.feeds.location.stops, 2)
    }

    func testAudioFailureShowsBannerAndStillOpens() async {
        let harness = Harness()
        harness.session.failActivation = true
        harness.model.apply(.missions(view: FakeScenarios.missions()))
        harness.model.open(missionID: FakeScenarios.huntID)
        XCTAssertEqual(harness.model.openMission?.id, FakeScenarios.huntID)
        await eventually { harness.model.banner != nil }
        XCTAssertEqual(harness.model.banner?.text, "Audio off")
    }

    func testNotReadyMissionDoesNotOpen() {
        let harness = Harness()
        harness.model.apply(.missions(view: FakeScenarios.missions()))
        harness.model.open(missionID: "df-2")
        XCTAssertNil(harness.model.openMission)
        XCTAssertEqual(harness.model.banner?.text, "No array")
        XCTAssertFalse(harness.core.calls.contains(.open("df-2")))
    }

    private func aligned(_ offset: Double) -> PoseView {
        let pose = FakeScenarios.pose(heading: 90)
        return PoseView(
            headingDeg: pose.headingDeg,
            accuracyDeg: pose.accuracyDeg,
            source: pose.source,
            align: .done(offsetDeg: offset),
            sending: pose.sending,
            fixAgeMs: pose.fixAgeMs
        )
    }

    func testAlignDoneStoresOffset() {
        let harness = Harness()
        let done = aligned(3.5)
        harness.model.apply(.pose(view: done))
        XCTAssertEqual(harness.settings.mountOffsetDeg, 3.5)
        XCTAssertEqual(harness.model.pose, done)
        XCTAssertEqual(harness.core.calls.last, .pose(harness.settings.poseSettings))
        let calls = harness.core.calls.count
        harness.model.apply(.pose(view: done))
        XCTAssertEqual(harness.core.calls.count, calls)
    }

    func testAlignDoneAcrossTheSeamKeepsTheOffset() {
        let harness = Harness()
        harness.settings.mountOffsetDeg = -180
        let calls = harness.core.calls.count
        harness.model.apply(.pose(view: aligned(180)))
        XCTAssertEqual(harness.settings.mountOffsetDeg, -180)
        XCTAssertEqual(harness.core.calls.count, calls)
    }

    func testWorkspaceSwitchCallsCore() async {
        let harness = Harness()
        harness.model.confirmWorkspace = FakeScenarios.lab
        await harness.model.switchWorkspace(FakeScenarios.lab)
        XCTAssertTrue(harness.core.calls.contains(.switchWorkspace("lab")))
        XCTAssertNil(harness.model.confirmWorkspace)
    }

    func testCoreErrorBannerUsesShortLabel() {
        let harness = Harness()
        harness.model.report(CoreError.WrongCode)
        XCTAssertEqual(harness.model.banner?.text, "Wrong code")
        XCTAssertEqual(harness.model.banner?.level, .error)
        XCTAssertFalse(harness.model.banner?.detail?.isEmpty ?? true)
    }

    func testNoticeBecomesBanner() {
        let harness = Harness()
        harness.model.apply(.notice(notice: Notice(level: .warn, text: "Missed 2 updates")))
        XCTAssertEqual(harness.model.banner?.text, "Missed 2 updates")
        XCTAssertEqual(harness.model.banner?.level, .warn)
    }

    func testForgetLastServerLeavesSettingsForPair() async throws {
        let harness = Harness()
        await harness.start()
        harness.model.showSettings = true
        harness.model.forget(try XCTUnwrap(harness.model.servers.first))
        XCTAssertTrue(harness.model.needsPairing)
        XCTAssertFalse(harness.model.showSettings)
        XCTAssertTrue(harness.core.calls.contains(.disconnect))
    }

    func testReconnectingLinkWatchesForHosts() async {
        let harness = Harness()
        await harness.start()
        harness.model.apply(.link(state: .connecting(attempt: 2, host: "10.0.0.2:8443")))
        let moved = DiscoveredServer(name: "Lab Pi", hosts: ["10.0.0.9:8443"], txt: ["id": "s1"])
        harness.browser.found([moved])
        XCTAssertTrue(harness.core.calls.contains(.updateHosts("s1", ["10.0.0.9:8443"])))
        harness.model.apply(.link(state: .online(server: "Lab Pi")))
        XCTAssertEqual(harness.browser.stops, 1)
    }
}

extension HuntView {
    func with(mission: String) -> HuntView {
        HuntView(
            mission: mission,
            freqHz: freqHz,
            levelDb: levelDb,
            smoothDb: smoothDb,
            floorDb: floorDb,
            bestDb: bestDb,
            strength: strength,
            trend: trend,
            running: running,
            refusal: refusal,
            readings: readings,
            sweep: sweep
        )
    }
}
