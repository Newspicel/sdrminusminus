import CoreLocation
import Foundation
import Observation
import SdrmmCore

protocol NavClock: Sendable {
    func now() -> Date
    func sleep(until: Date) async throws
}

nonisolated struct SystemNavClock: NavClock {
    func now() -> Date { Date() }

    func sleep(until: Date) async throws {
        try await Task.sleep(for: .seconds(max(0, until.timeIntervalSinceNow)))
    }
}

@Observable
final class NavigationModel {
    private(set) var pendingTarget: NavPoint?
    private(set) var lastRetarget: RetargetNotice?
    private(set) var lastLocation: CLLocation?
    @ObservationIgnored private let routes: any RouteProviding
    @ObservationIgnored private let speech: any SpeechPrompting
    @ObservationIgnored private let settings: SettingsStore
    @ObservationIgnored private let clock: any NavClock
    @ObservationIgnored private let report: @MainActor (Error) -> Void

    init(
        routes: any RouteProviding,
        speech: any SpeechPrompting,
        settings: SettingsStore,
        clock: any NavClock,
        report: @escaping @MainActor (Error) -> Void
    ) {
        self.routes = routes
        self.speech = speech
        self.settings = settings
        self.clock = clock
        self.report = report
    }

    func start(to target: NavPoint) {
        report(NotBuilt(feature: "Navigation"))
    }

    func end() {
        routes.cancel()
    }

    func update(location: CLLocation) {
        guard location.horizontalAccuracy >= 0 else {
            return
        }
        lastLocation = location
    }

    func retarget(_ notice: RetargetNotice) {
        lastRetarget = notice
    }
}
