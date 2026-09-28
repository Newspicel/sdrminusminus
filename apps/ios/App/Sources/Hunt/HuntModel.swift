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

    init(
        core: any CoreService,
        settings: SettingsStore,
        clicks: any ClickPlaying,
        report: @escaping @MainActor (Error) -> Void
    ) {
        self.core = core
        self.settings = settings
        self.clicks = clicks
        self.report = report
    }

    func apply(_ view: HuntView) {
        self.view = view
    }

    func toggleRun() async {
        busy = true
        defer { busy = false }
        do {
            try await core.send(view?.running == true ? .stopHunt : .startHunt)
        } catch {
            report(error)
        }
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
    }

    func missionClosed() {
        view = nil
        clicks.stop()
    }

    func setClicks(_ on: Bool) {
        settings.clicksOn = on
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
}
