import CoreLocation
import Foundation
import SdrmmCore

@testable import SDRmm

enum Fixtures {
    static let origin = FakeScenarios.origin

    static func offset(_ from: LatLon, _ bearingDeg: Double, _ meters: Double) -> LatLon {
        FakeScenarios.offset(from, bearingDeg: bearingDeg, meters: meters)
    }

    static func fix(_ at: LatLon, speed: Double = 10, accuracy: Double = 5) -> CLLocation {
        CLLocation(
            coordinate: at.coordinate,
            altitude: 0,
            horizontalAccuracy: accuracy,
            verticalAccuracy: 5,
            course: 0,
            speed: speed,
            timestamp: Date()
        )
    }
}

extension Fixtures {
    static func lPlan() -> RoutePlan {
        FakeScenarios.plan()
    }

    static func straightPlan(lengthM: Double = 1_000) -> RoutePlan {
        let end = offset(origin, 0, lengthM)
        return RoutePlanBuilder.plan(
            name: "Straight",
            travelTimeS: 100,
            steps: [
                RawStep(instruction: "Head north", notice: nil, distanceM: lengthM, points: [origin, end]),
                RawStep(instruction: "Arrive", notice: nil, distanceM: 0, points: [end]),
            ]
        )
    }

    static func steppedPlan(stepEveryM: Double = 200, steps: Int = 5, pointEveryM: Double = 20) -> RoutePlan {
        var raw = [RawStep(instruction: "", notice: nil, distanceM: 0, points: [origin])]
        for index in 0..<steps {
            let start = Double(index) * stepEveryM
            let count = Int(stepEveryM / pointEveryM)
            let points = (0...count).map { offset(origin, 0, start + Double($0) * pointEveryM) }
            raw.append(
                RawStep(instruction: "Step \(index + 1)", notice: nil, distanceM: stepEveryM, points: points)
            )
        }
        let end = offset(origin, 0, Double(steps) * stepEveryM)
        raw.append(RawStep(instruction: "Arrive", notice: nil, distanceM: 0, points: [end]))
        return RoutePlanBuilder.plan(name: "Stepped", travelTimeS: 300, steps: raw)
    }

    static func position(next: Int?, toNextM: Double?, remainingM: Double = 1_000) -> RoutePosition {
        RoutePosition(
            alongM: 0,
            offRouteM: 0,
            segment: 0,
            nextStep: next,
            toNextM: toNextM,
            remainingM: remainingM
        )
    }

    static func target(_ at: LatLon, kind: GuidanceKind = .estimate) -> NavPoint {
        NavPoint(at: at, kind: kind)
    }

    static func retarget(_ at: LatLon, kind: GuidanceKind = .estimate) -> RetargetNotice {
        RetargetNotice(
            mission: FakeScenarios.dfID,
            target: target(at, kind: kind),
            movedM: 400,
            reason: .moved
        )
    }
}

extension Fixtures {
    static func df(
        state: DfState = .live,
        heading: Double? = 90,
        guidance: GuidanceView?? = nil,
        target: NavPoint?? = nil
    ) -> DfView {
        let base = FakeScenarios.df(bearing: 137, heading: heading)
        return DfView(
            mission: base.mission,
            state: state,
            bearingTrueDeg: base.bearingTrueDeg,
            bearingRelDeg: base.bearingRelDeg,
            confidence: base.confidence,
            sigmaDeg: base.sigmaDeg,
            freqHz: base.freqHz,
            targetMode: base.targetMode,
            guidance: guidance ?? base.guidance,
            target: target ?? base.target,
            estimate: base.estimate,
            overlay: base.overlay
        )
    }

    static func df(overlay: DfOverlay) -> DfView {
        let base = df()
        return DfView(
            mission: base.mission,
            state: base.state,
            bearingTrueDeg: base.bearingTrueDeg,
            bearingRelDeg: base.bearingRelDeg,
            confidence: base.confidence,
            sigmaDeg: base.sigmaDeg,
            freqHz: base.freqHz,
            targetMode: base.targetMode,
            guidance: base.guidance,
            target: base.target,
            estimate: base.estimate,
            overlay: overlay
        )
    }

    static func guidance(_ kind: GuidanceKind, heading: Double = 215, distance: Double = 1_200)
        -> GuidanceView
    {
        GuidanceView(kind: kind, headingTrueDeg: heading, headingRelDeg: nil, distanceM: distance)
    }
}

@MainActor
struct DriveHarness {
    let core: FakeCore
    let settings: SettingsStore
    let feeds = FakeFeeds()
    let routes: FakeRouteProvider
    let speech = SpeechRecorder()
    let notifier = NotifierRecorder()
    let model: AppModel

    init(plans: [RoutePlan] = [Fixtures.lPlan()]) {
        core = FakeCore(scenario: .paired, ticking: false)
        settings = SettingsStore(defaults: TestDefaults.make())
        settings.units = .metric
        routes = FakeRouteProvider(plans: plans)
        model = AppModel(
            core: core,
            settings: settings,
            routes: routes,
            speech: speech,
            clicks: ClickRecorder(),
            browser: FakeBrowser(),
            sensors: feeds.hub(core: core),
            notifier: notifier,
            audio: AudioSessionController(session: SilentAudioSession())
        )
    }

    func openDf() {
        model.apply(.missions(view: FakeScenarios.missions()))
        model.open(missionID: FakeScenarios.dfID)
    }

    func fix(_ at: LatLon = Fixtures.origin) {
        feeds.location.send(.fix(Fixtures.fix(at)))
    }
}
