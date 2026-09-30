import AVFAudio
import SdrmmCore
import UIKit
import XCTest

@testable import SDRmm

@MainActor
final class ErrorSink {
    private(set) var errors: [Error] = []

    func report(_ error: Error) {
        errors.append(error)
    }

    var labels: [String] { errors.map(CoreErrorText.short) }
}

@MainActor
struct HuntFixture {
    let core = FakeCore(scenario: .paired, ticking: false)
    let settings = SettingsStore(defaults: TestDefaults.make())
    let clicks = ClickRecorder()
    let sink = ErrorSink()
    let clock = ManualClock()
    let model: HuntModel

    init() {
        let sink = sink
        let clock = clock
        model = HuntModel(
            core: core,
            settings: settings,
            clicks: clicks,
            report: { sink.report($0) },
            now: { clock.now }
        )
    }

    var sent: [MissionCommand] {
        core.calls.compactMap { call in
            if case .send(let command) = call {
                return command
            }
            return nil
        }
    }
}

@MainActor
final class ManualClock {
    var now: TimeInterval = 0
}

@MainActor
final class HuntModelTests: XCTestCase {
    private func view(
        strength: Float = 0.5,
        trend: Trend = .warmer,
        running: Bool = true,
        sweep: SweepView? = nil
    ) -> HuntView {
        FakeScenarios.hunt(strength: strength, trend: trend, running: running, sweep: sweep)
    }

    func testToggleSendsStartThenStop() async {
        let fixture = HuntFixture()
        fixture.model.apply(view(running: false))
        await fixture.model.toggleRun()
        fixture.model.apply(view(running: true))
        await fixture.model.toggleRun()
        XCTAssertEqual(fixture.sent, [.startHunt, .stopHunt])
        XCTAssertFalse(fixture.model.busy)
    }

    func testTuneParsesCommaDecimal() async {
        let fixture = HuntFixture()
        let error = await fixture.model.tune(megahertz: "145,5")
        XCTAssertNil(error)
        XCTAssertEqual(fixture.sent, [.tune(hz: 145_500_000)])
    }

    func testTuneRejectsGarbage() async {
        let fixture = HuntFixture()
        let error = await fixture.model.tune(megahertz: "abc")
        XCTAssertEqual(error, "Bad frequency")
        XCTAssertTrue(fixture.sent.isEmpty)
    }

    func testTuneRefusalIsReportedAndReturned() async {
        let fixture = HuntFixture()
        fixture.core.fail(next: .Refused(message: "Tuned away"))
        let error = await fixture.model.tune(megahertz: "145.5")
        XCTAssertEqual(error, "Tuned away")
        XCTAssertEqual(fixture.sink.labels, ["Tuned away"])
    }

    func testClicksFollowRunningAndToggle() {
        let fixture = HuntFixture()
        fixture.model.apply(view(running: true))
        XCTAssertEqual(fixture.clicks.starts, 0)
        fixture.model.missionOpened()
        fixture.model.apply(view(running: false))
        XCTAssertEqual(fixture.clicks.starts, 0)
        fixture.model.apply(view(running: true))
        fixture.model.apply(view(running: true))
        XCTAssertEqual(fixture.clicks.starts, 1)
        XCTAssertTrue(fixture.clicks.running)
        fixture.model.setClicks(false)
        XCTAssertFalse(fixture.settings.clicksOn)
        XCTAssertFalse(fixture.clicks.running)
        fixture.model.apply(view(running: true))
        XCTAssertEqual(fixture.clicks.starts, 1)
        fixture.model.setClicks(true)
        XCTAssertEqual(fixture.clicks.starts, 2)
        fixture.model.apply(view(running: false))
        XCTAssertFalse(fixture.clicks.running)
        fixture.model.apply(view(running: true))
        fixture.model.missionClosed()
        XCTAssertFalse(fixture.clicks.running)
        XCTAssertEqual(fixture.clicks.starts, 3)
    }

    func testStrengthForwardedToClicks() {
        let fixture = HuntFixture()
        fixture.model.missionOpened()
        fixture.model.apply(view(strength: 0.3))
        fixture.model.apply(view(strength: 0.7))
        XCTAssertEqual(fixture.clicks.strengths.last, 0.7)
    }

    func testClickStartFailureTurnsClicksOffWithBanner() {
        let fixture = HuntFixture()
        fixture.clicks.failStart = AudioOff(reason: "No audio output")
        fixture.model.missionOpened()
        fixture.model.apply(view(running: true))
        XCTAssertFalse(fixture.settings.clicksOn)
        XCTAssertEqual(fixture.sink.labels, ["Audio off"])
        XCTAssertEqual(CoreErrorText.detail(fixture.sink.errors[0]), "Audio off: No audio output")
    }

    func testPlayerFailureLaterTurnsClicksOff() {
        let fixture = HuntFixture()
        fixture.model.missionOpened()
        fixture.model.apply(view(running: true))
        XCTAssertTrue(fixture.clicks.running)
        fixture.clicks.fail(AudioOff(reason: "route lost"))
        XCTAssertFalse(fixture.clicks.running)
        XCTAssertFalse(fixture.settings.clicksOn)
        XCTAssertEqual(fixture.sink.labels, ["Audio off"])
    }

    func testResumeRestartsOnlyRunningClicks() {
        let fixture = HuntFixture()
        fixture.model.missionOpened()
        fixture.model.resumeClicks()
        XCTAssertEqual(fixture.clicks.starts, 0)
        fixture.model.apply(view(running: true))
        fixture.model.resumeClicks()
        XCTAssertEqual(fixture.clicks.starts, 2)
        XCTAssertEqual(fixture.clicks.stops, 1)
        XCTAssertTrue(fixture.clicks.running)
    }

    func testClicksStartedBeforeActivationRestartOnceActive() async {
        let harness = Harness(scenario: .hunt)
        harness.model.apply(.missions(view: FakeScenarios.missions()))
        harness.model.open(missionID: FakeScenarios.huntID)
        harness.model.apply(.hunt(view: view(running: true)))
        XCTAssertEqual(harness.clicks.starts, 1)
        await eventually { harness.clicks.starts == 2 }
        XCTAssertEqual(harness.clicks.starts, 2)
        XCTAssertEqual(harness.clicks.stops, 1)
        XCTAssertEqual(harness.session.active, [true])
        XCTAssertTrue(harness.clicks.running)
    }

    func testInterruptionEndedResumesClicksThroughTheApp() async {
        let harness = Harness(scenario: .hunt)
        harness.model.apply(.missions(view: FakeScenarios.missions()))
        harness.model.open(missionID: FakeScenarios.huntID)
        harness.model.apply(.hunt(view: view(running: true)))
        await eventually { harness.clicks.starts == 2 }
        XCTAssertEqual(harness.session.active, [true])
        harness.center.post(
            name: AVAudioSession.interruptionNotification,
            object: nil,
            userInfo: [
                AVAudioSessionInterruptionTypeKey: AVAudioSession.InterruptionType.ended.rawValue,
                AVAudioSessionInterruptionOptionKey: AVAudioSession.InterruptionOptions.shouldResume.rawValue,
            ]
        )
        await eventually { harness.clicks.starts == 3 }
        XCTAssertEqual(harness.clicks.starts, 3)
        XCTAssertEqual(harness.session.active, [true, true])
        XCTAssertTrue(harness.clicks.running)
    }

    func testHapticCueOnWarmerTransition() {
        let fixture = HuntFixture()
        fixture.model.missionOpened()
        fixture.model.apply(view(strength: 0.2, trend: .waiting))
        XCTAssertNil(fixture.model.cue)
        fixture.clock.now = 1
        fixture.model.apply(view(strength: 0.3, trend: .warmer))
        XCTAssertEqual(fixture.model.cue?.cue, .increase)
        let first = fixture.model.cue?.id
        fixture.clock.now = 2
        fixture.model.apply(view(strength: 0.2, trend: .colder))
        XCTAssertEqual(fixture.model.cue?.cue, .decrease)
        XCTAssertNotEqual(fixture.model.cue?.id, first)
    }

    func testSweepToggleSendsOnThenOff() async {
        let fixture = HuntFixture()
        fixture.model.apply(view())
        XCTAssertFalse(fixture.model.sweeping)
        await fixture.model.toggleSweep()
        fixture.model.apply(view(sweep: FakeScenarios.sweep()))
        XCTAssertTrue(fixture.model.sweeping)
        await fixture.model.toggleSweep()
        fixture.model.apply(view(sweep: FakeScenarios.sweep(phase: .off)))
        XCTAssertFalse(fixture.model.sweeping)
        XCTAssertEqual(fixture.sent, [.sweep(on: true), .sweep(on: false)])
    }

    func testMarkSendsCommandAndReportsRefusal() async {
        let fixture = HuntFixture()
        await fixture.model.mark()
        fixture.core.fail(next: .Refused(message: "No heading"))
        await fixture.model.mark()
        XCTAssertEqual(fixture.sent, [.mark, .mark])
        XCTAssertEqual(fixture.sink.labels, ["No heading"])
    }

    func testSweepPhaseLabels() {
        let expected: [(SweepPhase, String?, Bool)] = [
            (.off, nil, false),
            (.idle, "Turn slowly", false),
            (.sweeping, "Sweeping", false),
            (.noHeading, "No heading", true),
            (.shortSpan, "Short", true),
            (.lowContrast, "Low contrast", true),
            (.poorFit, "Poor fit", true),
            (.headingPoor, "Heading poor", true),
            (.tooFast, "Too fast", true),
            (.done, "Done", false),
        ]
        for (phase, label, problem) in expected {
            XCTAssertEqual(SweepText.phase(phase), label)
            XCTAssertEqual(SweepText.problem(phase), problem)
        }
        XCTAssertEqual(SweepText.sigma(7.6), "\u{00B1}8\u{00B0}")
        XCTAssertEqual(SweepText.sigma(nil), "-")
        XCTAssertEqual(SweepText.covered(270.2), "270\u{00B0}")
    }

    func testSweepPetalsTurnWithHeading() {
        let bins: [UInt8] = [0, 255, 0, 128]
        let north = SweepPlot.petals(bins: bins, up: 0)
        XCTAssertEqual(north.map(\.startDeg), [90, 270])
        XCTAssertEqual(north.map(\.endDeg), [180, 360])
        XCTAssertEqual(north[0].length, 1)
        XCTAssertEqual(north[1].length, 128.0 / 255, accuracy: 1e-9)
        let headingUp = SweepPlot.petals(bins: bins, up: 90)
        XCTAssertEqual(headingUp.map(\.startDeg), [0, 180])
        XCTAssertEqual(SweepPlot.binWidth(count: 72), 5)
        XCTAssertEqual(SweepPlot.screenDeg(bearing: 10, up: 350), 20)
    }

    func testSweepPetalPointsUpForNorth() {
        let center = CGPoint(x: 100, y: 100)
        let up = SweepPlot.point(center: center, radius: 50, screenDeg: 0)
        XCTAssertEqual(up.x, 100, accuracy: 1e-9)
        XCTAssertEqual(up.y, 50, accuracy: 1e-9)
        let right = SweepPlot.point(center: center, radius: 50, screenDeg: 90)
        XCTAssertEqual(right.x, 150, accuracy: 1e-9)
        XCTAssertEqual(right.y, 100, accuracy: 1e-9)
    }

    func testTrendLabelsAndSymbols() {
        let fixture = HuntFixture()
        XCTAssertEqual(fixture.model.trendLabel, "Listening")
        let expected: [(Trend, String, String)] = [
            (.waiting, "Listening", "ear"),
            (.warmer, "Warmer", "arrow.up"),
            (.colder, "Colder", "arrow.down"),
            (.onTop, "On top", "scope"),
        ]
        for (trend, label, symbol) in expected {
            fixture.model.apply(view(trend: trend))
            XCTAssertEqual(fixture.model.trendLabel, label)
            XCTAssertEqual(fixture.model.trendSymbol, symbol)
            XCTAssertNotNil(UIImage(systemName: symbol))
        }
    }
}

@MainActor
func eventually(_ condition: () -> Bool) async {
    for _ in 0..<100 where !condition() {
        try? await Task.sleep(for: .milliseconds(10))
    }
}
