import MapKit
import Observation
import SdrmmCore
import SwiftUI

struct MapLayers: Equatable {
    var rays = true
    var heat = true
    var ellipse = true
}

@Observable
final class DfDriveModel {
    static let overlayInterval: TimeInterval = 0.5
    static let emptyOverlay = DfOverlay(rays: [], stations: [], ellipse: [], heat: [])

    private(set) var view: DfView?
    private(set) var pose: PoseView?
    private(set) var overlay = DfDriveModel.emptyOverlay
    private(set) var retargetTick = 0
    private var sending = 0
    var layers = MapLayers()
    var camera: MapCameraPosition = .userLocation(fallback: .automatic)
    var confirmClear = false
    @ObservationIgnored var showNavigation: (@MainActor () -> Void)?
    @ObservationIgnored var now: () -> Date = { Date() }
    @ObservationIgnored private let core: any CoreService
    @ObservationIgnored private let navigation: NavigationModel
    @ObservationIgnored private let report: @MainActor (Error) -> Void
    @ObservationIgnored private var overlayAt: Date?
    @ObservationIgnored private var pendingOverlay: DfOverlay?
    @ObservationIgnored private var overlayFlush: Task<Void, Never>?

    init(core: any CoreService, navigation: NavigationModel, report: @escaping @MainActor (Error) -> Void) {
        self.core = core
        self.navigation = navigation
        self.report = report
    }

    func apply(_ view: DfView) {
        self.view = view
        stage(view.overlay)
    }

    func apply(pose: PoseView) {
        self.pose = pose
    }

    func missionOpened() {
        view = nil
        overlayFlush?.cancel()
        overlayFlush = nil
        pendingOverlay = nil
        overlayAt = nil
        overlay = Self.emptyOverlay
        confirmClear = false
        camera = .userLocation(fallback: .automatic)
    }

    func retargeted(_ notice: RetargetNotice) {
        retargetTick += 1
    }

    func setTargetMode(_ mode: TargetMode) async {
        await send(.setTargetMode(mode: mode))
    }

    @discardableResult
    func calibrate() async -> Bool {
        await send(.calibrate)
    }

    func askClear() {
        confirmClear = true
    }

    @discardableResult
    func clearFusion() async -> Bool {
        confirmClear = false
        return await send(.clearFusion)
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

    func navigate() {
        guard let target = view?.target else {
            return
        }
        navigation.start(to: target)
        showNavigation?()
    }

    func fitAll() {
        guard let rect = DfMapStyle.region(DfMapStyle.points(view: view)) else {
            camera = .userLocation(fallback: .automatic)
            return
        }
        camera = .rect(rect)
    }

    var rose: RoseState { RoseState.make(view: view, pose: pose) }

    var bearingText: String {
        rose.bearingDeg.map(AngleText.degrees) ?? "-"
    }

    var confidenceText: String {
        DfText.confidence(view) ?? "-"
    }

    var stateLabel: String? {
        guard let view else {
            return "Waiting"
        }
        return DfText.state(view.state)
    }

    var guidanceText: String {
        DfText.guidance(view?.guidance, units: navigation.units)
    }

    var canNavigate: Bool { view?.target != nil }

    var heatAvailable: Bool { !overlay.heat.isEmpty }

    var busy: Bool { sending > 0 }

    @discardableResult
    private func send(_ command: MissionCommand) async -> Bool {
        sending += 1
        defer { sending -= 1 }
        do {
            try await core.send(command)
            return true
        } catch {
            report(error)
            return false
        }
    }

    private func stage(_ next: DfOverlay) {
        guard next != overlay else {
            pendingOverlay = nil
            return
        }
        let current = now()
        guard let last = overlayAt, current.timeIntervalSince(last) < Self.overlayInterval else {
            commit(next, at: current)
            return
        }
        pendingOverlay = next
        guard overlayFlush == nil else {
            return
        }
        let wait = Self.overlayInterval - current.timeIntervalSince(last)
        overlayFlush = Task { [weak self] in
            do {
                try await Task.sleep(for: .seconds(wait))
            } catch {
                return
            }
            self?.flush()
        }
    }

    private func flush() {
        overlayFlush = nil
        guard let pending = pendingOverlay else {
            return
        }
        pendingOverlay = nil
        commit(pending, at: now())
    }

    private func commit(_ next: DfOverlay, at time: Date) {
        overlay = next
        overlayAt = time
    }
}

nonisolated enum DfText {
    static func state(_ state: DfState) -> String? {
        switch state {
        case .waiting: "Waiting"
        case .calibrating: "Calibrating"
        case .phaseUnknown: "Phase unknown"
        case .noHeading: "No heading"
        case .squelched: "Squelched"
        case .live: nil
        }
    }

    static func guidance(_ guidance: GuidanceView?, units: UnitSystem) -> String {
        guard let guidance else {
            return "No guidance"
        }
        let distance = DistanceText.short(guidance.distanceM, units)
        switch guidance.kind {
        case .probe: return "Cross \(AngleText.degrees(guidance.headingTrueDeg)) \u{00B7} \(distance)"
        case .estimate: return "Approach \(distance)"
        }
    }

    static func confidence(_ view: DfView?) -> String? {
        guard let view, view.state == .live, view.confidence.isFinite else {
            return nil
        }
        return "\(Int((view.confidence * 100).rounded()))%"
    }
}
