import Foundation
import MapKit
import Observation
import SdrmmCore
import SwiftUI

@Observable
final class SurveyModel {
    private(set) var view: SurveyView?
    private(set) var runs: [TrailRun] = []
    private(set) var trimmed = false
    private(set) var busy = false
    var camera: MapCameraPosition = .automatic
    @ObservationIgnored private let core: any CoreService
    @ObservationIgnored private let report: @MainActor (Error) -> Void
    @ObservationIgnored private let now: @MainActor () -> TimeInterval
    @ObservationIgnored private var points: [SurveyPoint] = []
    @ObservationIgnored private var nextID = 0
    @ObservationIgnored private var built: LevelRange?
    @ObservationIgnored private var lastRebuild: TimeInterval?
    @ObservationIgnored private var open = false

    init(
        core: any CoreService,
        report: @escaping @MainActor (Error) -> Void,
        now: @escaping @MainActor () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }
    ) {
        self.core = core
        self.report = report
        self.now = now
    }

    var pointCount: Int { points.count }

    var scale: LevelRange? {
        guard let view, view.total > 0 else {
            return nil
        }
        return LevelRange(min: view.minDb, max: view.maxDb)
    }

    func apply(_ view: SurveyView) {
        let before = self.view?.total ?? 0
        self.view = view
        if view.total == 0, before > 0 {
            clearTrail()
            return
        }
        rebuildIfShifted(LevelRange(min: view.minDb, max: view.maxDb))
    }

    func append(_ incoming: [SurveyPoint]) {
        guard open, !incoming.isEmpty else {
            return
        }
        points.append(contentsOf: incoming)
        if points.count > SurveyTrail.maxPoints {
            points.removeFirst(points.count - SurveyTrail.keptPoints)
            trimmed = true
            draw(currentRange() ?? built)
            return
        }
        guard let range = built else {
            draw(currentRange())
            return
        }
        SurveyTrail.extend(&runs, with: incoming, min: range.min, max: range.max, nextID: &nextID)
        if SurveyTrail.trim(&runs) {
            trimmed = true
        }
    }

    func clearTrail() {
        points = []
        runs = []
        trimmed = false
        built = nil
    }

    func toggleRecording() async {
        busy = true
        defer { busy = false }
        do {
            try await core.send(view?.recording == true ? .stopSurvey : .startSurvey)
        } catch {
            report(error)
        }
    }

    func fit() {
        guard let region = SurveyTrail.region(runs.flatMap(\.coordinates)) else {
            camera = .automatic
            return
        }
        camera = .region(region)
    }

    func missionOpened() {
        reset()
        open = true
    }

    func missionClosed() {
        reset()
        open = false
    }

    private func reset() {
        view = nil
        clearTrail()
        lastRebuild = nil
        camera = .automatic
    }

    private func currentRange() -> LevelRange? {
        if let view, view.maxDb > view.minDb {
            return LevelRange(min: view.minDb, max: view.maxDb)
        }
        return SurveyTrail.range(of: points)
    }

    private func rebuildIfShifted(_ range: LevelRange) {
        guard range.max > range.min else {
            return
        }
        guard let built else {
            if !points.isEmpty {
                draw(range)
            }
            return
        }
        guard range.shifted(from: built, by: SurveyTrail.rebuildShiftDb) else {
            return
        }
        if let lastRebuild, now() - lastRebuild < SurveyTrail.rebuildInterval {
            return
        }
        lastRebuild = now()
        draw(range)
    }

    private func draw(_ range: LevelRange?) {
        runs = []
        guard let range else {
            return
        }
        built = range
        SurveyTrail.extend(&runs, with: points, min: range.min, max: range.max, nextID: &nextID)
        if SurveyTrail.trim(&runs) {
            trimmed = true
        }
    }
}
