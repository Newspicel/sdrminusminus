import Foundation
import Observation
import SdrmmCore

@Observable
final class HuntModel {
    private(set) var view: HuntView?
    private(set) var cue: HapticCueEvent?
    private(set) var busy = false
    @ObservationIgnored private let core: any CoreService
    @ObservationIgnored private let settings: SettingsStore
    @ObservationIgnored private let clicks: any ClickPlaying
    @ObservationIgnored private let report: @MainActor (Error) -> Void
    @ObservationIgnored private let now: @MainActor () -> TimeInterval
    @ObservationIgnored private var policy = HuntHapticPolicy()
    @ObservationIgnored private var cues = 0
    @ObservationIgnored private var open = false
    @ObservationIgnored private var clicking = false

    init(
        core: any CoreService,
        settings: SettingsStore,
        clicks: any ClickPlaying,
        report: @escaping @MainActor (Error) -> Void,
        now: @escaping @MainActor () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }
    ) {
        self.core = core
        self.settings = settings
        self.clicks = clicks
        self.report = report
        self.now = now
        clicks.onFailure = { [weak self] error in self?.clicksFailed(error) }
    }

    func apply(_ view: HuntView) {
        self.view = view
        if let next = policy.cue(trend: view.trend, strength: view.strength, at: now()) {
            cues += 1
            cue = HapticCueEvent(id: cues, cue: next)
        }
        clicks.setStrength(view.strength)
        syncClicks()
    }

    func toggleRun() async {
        await send(view?.running == true ? .stopHunt : .startHunt)
    }

    func toggleSweep() async {
        await send(.sweep(on: !sweeping))
    }

    func mark() async {
        await send(.mark)
    }

    func tune(megahertz: String) async -> String? {
        guard let hz = TuneInput.hertz(megahertz) else {
            return "Bad frequency"
        }
        do {
            try await core.send(.tune(hz: hz))
            return nil
        } catch {
            report(error)
            return CoreErrorText.short(error)
        }
    }

    func missionOpened() {
        view = nil
        policy.reset()
        open = true
        syncClicks()
    }

    func missionClosed() {
        view = nil
        open = false
        syncClicks()
    }

    func setClicks(_ on: Bool) {
        settings.clicksOn = on
        syncClicks()
    }

    func resumeClicks() {
        guard clicking else {
            return
        }
        clicks.stop()
        clicking = false
        syncClicks()
    }

    var sweeping: Bool {
        guard let phase = view?.sweep?.phase else {
            return false
        }
        return phase != .off
    }

    var trendLabel: String {
        switch view?.trend ?? .waiting {
        case .waiting: "Listening"
        case .warmer: "Warmer"
        case .colder: "Colder"
        case .onTop: "On top"
        }
    }

    var trendSymbol: String {
        switch view?.trend ?? .waiting {
        case .waiting: "ear"
        case .warmer: "arrow.up"
        case .colder: "arrow.down"
        case .onTop: "scope"
        }
    }

    private func send(_ command: MissionCommand) async {
        busy = true
        defer { busy = false }
        do {
            try await core.send(command)
        } catch {
            report(error)
        }
    }

    private func syncClicks() {
        let wanted = open && settings.clicksOn && view?.running == true
        guard wanted != clicking else {
            return
        }
        guard wanted else {
            clicks.stop()
            clicking = false
            return
        }
        do {
            try clicks.start()
            clicking = true
        } catch {
            clicksFailed(error)
        }
    }

    private func clicksFailed(_ error: Error) {
        clicks.stop()
        clicking = false
        settings.clicksOn = false
        report(error)
    }
}
