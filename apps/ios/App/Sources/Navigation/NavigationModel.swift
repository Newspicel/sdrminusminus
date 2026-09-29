import CoreLocation
import Foundation
import Observation
import SdrmmCore
import os

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

nonisolated struct ActiveRoute: Equatable, Sendable {
    let plan: RoutePlan
    let target: NavPoint
    var position: RoutePosition?
    var offRouteStreak: Int
    var rerouting: Bool
}

nonisolated enum NoRouteReason: Equatable, Sendable {
    case notFound
    case throttled(retryAt: Date)
    case network
    case failed(String)
}

nonisolated struct ManeuverBannerState: Equatable, Sendable {
    let symbol: String
    let distance: String
    let instruction: String
}

nonisolated struct TripSummary: Equatable, Sendable {
    let duration: String
    let remaining: String
    let arrival: String
}

@Observable
final class NavigationModel {
    enum RouteReason: Equatable {
        case start, retarget, offRoute, retry
    }

    enum Phase: Equatable {
        case idle
        case routing(NavPoint, RouteReason)
        case active(ActiveRoute)
        case noRoute(NavPoint, NoRouteReason)
        case arrived(NavPoint)
    }

    enum Rule {
        static let spacingS: TimeInterval = 10
        static let minBackoffS: TimeInterval = 15
        static let maxBackoffS: TimeInterval = 120
        static let failureRetryS: TimeInterval = 30
        static let notFoundMoveM = 500.0
        static let offRouteM = 50.0
        static let offRouteFixes = 3
        static let qualifyingAccuracyM = 30.0
        static let qualifyingSpeedMps = 1.5
        static let arriveRemainingM = 30.0
        static let arriveTargetM = 40.0
    }

    private(set) var phase: Phase = .idle
    private(set) var pendingTarget: NavPoint?
    private(set) var lastRetarget: RetargetNotice?
    private(set) var maneuverTick = 0
    private(set) var nearTick = 0
    private(set) var lastLocation: CLLocation?
    private(set) var retryAt: Date?
    @ObservationIgnored private let routes: any RouteProviding
    @ObservationIgnored private let speech: any SpeechPrompting
    @ObservationIgnored private let settings: SettingsStore
    @ObservationIgnored private let clock: any NavClock
    @ObservationIgnored private let report: @MainActor (Error) -> Void
    @ObservationIgnored private var track: RouteTrack?
    @ObservationIgnored private var announcer = AnnouncePolicy()
    @ObservationIgnored private var inFlight: Task<Void, Never>?
    @ObservationIgnored private var scheduled: Task<Void, Never>?
    @ObservationIgnored private var scheduledFor: Date?
    @ObservationIgnored private var lastRequestAt: Date?
    @ObservationIgnored private var backoffS: TimeInterval = 0
    @ObservationIgnored private var notFoundFrom: LatLon?
    @ObservationIgnored private var awaitingFix: (NavPoint, RouteReason)?
    @ObservationIgnored private var reported: Set<String> = []
    @ObservationIgnored private var generation = 0

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

    var units: UnitSystem { settings.unitSystem }

    var activePlan: RoutePlan? {
        guard case .active(let route) = phase else {
            return nil
        }
        return route.plan
    }

    var activePosition: RoutePosition? {
        guard case .active(let route) = phase else {
            return nil
        }
        return route.position
    }

    var isRerouting: Bool {
        guard case .active(let route) = phase else {
            return false
        }
        return route.rerouting
    }

    var target: NavPoint? {
        switch phase {
        case .idle: nil
        case .routing(let target, _), .noRoute(let target, _), .arrived(let target): target
        case .active(let route): route.target
        }
    }

    var isNavigating: Bool { phase != .idle }

    func start(to target: NavPoint) {
        stopRequests()
        track = nil
        announcer.reset()
        pendingTarget = nil
        backoffS = 0
        retryAt = nil
        notFoundFrom = nil
        reported = []
        phase = .routing(target, .start)
        request(target, .start)
    }

    func start(plan: RoutePlan, to target: NavPoint) {
        stopRequests()
        pendingTarget = nil
        backoffS = 0
        retryAt = nil
        activate(plan, target)
    }

    func previews(to target: NavPoint) async throws(RouteError) -> [RoutePlan] {
        guard let from = lastLocation else {
            throw .other("No location")
        }
        lastRequestAt = clock.now()
        return try await routes.routes(from: LatLon(from.coordinate), to: target.at, alternatives: true)
    }

    func end() {
        stopRequests()
        routes.cancel()
        speech.stop()
        track = nil
        announcer.reset()
        pendingTarget = nil
        retryAt = nil
        backoffS = 0
        notFoundFrom = nil
        phase = .idle
    }

    func goToPending() {
        guard let target = pendingTarget else {
            return
        }
        start(to: target)
    }

    func update(location: CLLocation) {
        guard location.horizontalAccuracy >= 0 else {
            return
        }
        lastLocation = location
        if let (target, reason) = awaitingFix {
            awaitingFix = nil
            request(target, reason)
        }
        switch phase {
        case .active(let route):
            progress(route, fix: location)
        case .noRoute(let target, .notFound):
            retryAfterMoving(to: target, fix: location)
        case .idle, .routing, .noRoute, .arrived:
            break
        }
    }

    func retarget(_ notice: RetargetNotice) {
        lastRetarget = notice
        let spoken = PromptText.retarget(distanceM: distance(to: notice.target), units: units)
        switch phase {
        case .idle:
            return
        case .arrived:
            pendingTarget = notice.target
            speech.say(spoken, urgent: true)
        case .active(let route):
            speech.say(spoken, urgent: true)
            phase = .active(
                ActiveRoute(
                    plan: route.plan,
                    target: notice.target,
                    position: route.position,
                    offRouteStreak: 0,
                    rerouting: true
                )
            )
            request(notice.target, .retarget)
        case .noRoute(_, .throttled(let retry)):
            speech.say(spoken, urgent: true)
            phase = .noRoute(notice.target, .throttled(retryAt: retry))
            request(notice.target, .retarget)
        case .routing, .noRoute:
            speech.say(spoken, urgent: true)
            phase = .routing(notice.target, .retarget)
            request(notice.target, .retarget)
        }
    }

    func settled() async {
        while true {
            if let scheduled, let due = scheduledFor, due <= clock.now() {
                await scheduled.value
                continue
            }
            if let inFlight {
                await inFlight.value
                continue
            }
            return
        }
    }

    func distance(to target: NavPoint) -> Double? {
        lastLocation.map { geoDistanceM(from: LatLon($0.coordinate), to: target.at) }
    }
}

extension NavigationModel {
    var banner: ManeuverBannerState? {
        guard case .active(let route) = phase else {
            return nil
        }
        let plan = route.plan
        guard let position = route.position else {
            guard plan.steps.count > 1 else {
                return nil
            }
            return bannerState(plan, step: 1, distance: plan.steps[1].startM)
        }
        guard let step = position.nextStep, let toNext = position.toNextM else {
            return ManeuverBannerState(
                symbol: ManeuverKind.arrive.symbolName,
                distance: DistanceText.short(position.remainingM, units),
                instruction: PromptText.instruction(plan, step: plan.steps.count - 1)
            )
        }
        return bannerState(plan, step: step, distance: toNext)
    }

    var summary: TripSummary? {
        guard case .active(let route) = phase else {
            return nil
        }
        let plan = route.plan
        let remaining = route.position?.remainingM ?? plan.distanceM
        let seconds = plan.distanceM > 0 ? plan.travelTimeS * remaining / plan.distanceM : 0
        let arrival = clock.now().addingTimeInterval(seconds)
        return TripSummary(
            duration: DurationText.text(seconds),
            remaining: DistanceText.short(remaining, units),
            arrival: Self.clockText.string(from: arrival)
        )
    }

    var directGuidance: String? {
        guard case .noRoute(let target, _) = phase, let fix = lastLocation else {
            return nil
        }
        let here = LatLon(fix.coordinate)
        let bearing = AngleText.degrees(geoBearingDeg(from: here, to: target.at))
        let distance = DistanceText.short(geoDistanceM(from: here, to: target.at), units)
        return "Steer \(bearing) \u{00B7} \(distance)"
    }

    private static let clockText: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "HH:mm"
        return formatter
    }()

    private func bannerState(_ plan: RoutePlan, step: Int, distance: Double) -> ManeuverBannerState {
        ManeuverBannerState(
            symbol: plan.steps[step].maneuver.symbolName,
            distance: DistanceText.short(distance, units),
            instruction: PromptText.instruction(plan, step: step)
        )
    }
}

extension NavigationModel {
    private func activate(_ plan: RoutePlan, _ target: NavPoint) {
        track = RouteTrack(plan: plan)
        announcer.reset()
        notFoundFrom = nil
        phase = .active(
            ActiveRoute(plan: plan, target: target, position: nil, offRouteStreak: 0, rerouting: false)
        )
        maneuverTick += 1
        if let fix = lastLocation, case .active(let route) = phase {
            progress(route, fix: fix)
        }
    }

    private func progress(_ current: ActiveRoute, fix: CLLocation) {
        guard var track else {
            return
        }
        var route = current
        let here = LatLon(fix.coordinate)
        let position = track.locate(here)
        self.track = track
        let previous = route.position?.nextStep
        route.position = position
        if Self.qualifies(fix) {
            route.offRouteStreak = position.offRouteM > Rule.offRouteM ? route.offRouteStreak + 1 : 0
        }
        if arrived(route, position: position, here: here) {
            arrive(route.target)
            return
        }
        if position.nextStep != previous {
            maneuverTick += 1
        }
        if route.offRouteStreak >= Rule.offRouteFixes, !route.rerouting {
            route.rerouting = true
            phase = .active(route)
            speech.say(PromptText.rerouting, urgent: false)
            request(route.target, .offRoute)
            return
        }
        phase = .active(route)
        if !route.rerouting {
            announce(route.plan, position: position, speed: max(0, fix.speed))
        }
    }

    private static func qualifies(_ fix: CLLocation) -> Bool {
        fix.horizontalAccuracy <= Rule.qualifyingAccuracyM && fix.speed >= Rule.qualifyingSpeedMps
    }

    private func arrived(_ route: ActiveRoute, position: RoutePosition, here: LatLon) -> Bool {
        if !route.rerouting, position.remainingM < Rule.arriveRemainingM {
            return true
        }
        return geoDistanceM(from: here, to: route.target.at) < Rule.arriveTargetM
    }

    private func arrive(_ target: NavPoint) {
        stopRequests()
        track = nil
        retryAt = nil
        phase = .arrived(target)
        speech.say(PromptText.arrived, urgent: true)
    }

    private func announce(_ plan: RoutePlan, position: RoutePosition, speed: Double) {
        for prompt in announcer.prompts(plan: plan, position: position, speedMps: speed) {
            if prompt.stage == .near {
                nearTick += 1
            }
            let stage = prompt.stage == .near ? "near" : "far"
            Log.nav.info("prompt step \(prompt.step) \(stage, privacy: .public)")
            speech.say(PromptText.text(prompt, plan: plan, units: units), urgent: prompt.stage == .near)
        }
    }

    private func retryAfterMoving(to target: NavPoint, fix: CLLocation) {
        guard let from = notFoundFrom,
            geoDistanceM(from: from, to: LatLon(fix.coordinate)) >= Rule.notFoundMoveM
        else {
            return
        }
        notFoundFrom = nil
        request(target, .retry)
    }
}

extension NavigationModel {
    private func request(_ target: NavPoint, _ reason: RouteReason) {
        guard lastLocation != nil else {
            awaitingFix = (target, reason)
            return
        }
        awaitingFix = nil
        let now = clock.now()
        let spaced = lastRequestAt.map { $0.addingTimeInterval(Rule.spacingS) } ?? now
        let opens = max(spaced, retryAt ?? now)
        if opens > now {
            schedule(at: opens, target, reason)
        } else {
            fire(target, reason)
        }
    }

    private func schedule(at date: Date, _ target: NavPoint, _ reason: RouteReason) {
        scheduled?.cancel()
        scheduledFor = date
        let clock = clock
        scheduled = Task { [weak self] in
            do {
                try await clock.sleep(until: date)
            } catch {
                return
            }
            guard let self, !Task.isCancelled else {
                return
            }
            self.scheduled = nil
            self.scheduledFor = nil
            self.request(target, reason)
        }
    }

    private func fire(_ target: NavPoint, _ reason: RouteReason) {
        guard let fix = lastLocation else {
            awaitingFix = (target, reason)
            return
        }
        scheduled?.cancel()
        scheduled = nil
        scheduledFor = nil
        if inFlight != nil {
            inFlight?.cancel()
            routes.cancel()
        }
        lastRequestAt = clock.now()
        generation += 1
        let token = generation
        let from = LatLon(fix.coordinate)
        let routes = routes
        Log.nav.debug("route request \(String(describing: reason), privacy: .public)")
        inFlight = Task { [weak self] in
            let result: Result<[RoutePlan], RouteError>
            do throws(RouteError) {
                result = .success(try await routes.routes(from: from, to: target.at, alternatives: false))
            } catch {
                result = .failure(error)
            }
            self?.finished(token, target: target, from: from, result: result)
        }
    }

    private func finished(
        _ token: Int,
        target: NavPoint,
        from: LatLon,
        result: Result<[RoutePlan], RouteError>
    ) {
        guard token == generation else {
            return
        }
        inFlight = nil
        switch result {
        case .success(let plans):
            guard let plan = plans.first else {
                failed(.notFound, target: target, from: from)
                return
            }
            backoffS = 0
            retryAt = nil
            reported = []
            activate(plan, target)
        case .failure(let error):
            failed(error, target: target, from: from)
        }
    }

    private func failed(_ error: RouteError, target: NavPoint, from: LatLon) {
        Log.nav.error("route failed: \(error.detail, privacy: .public)")
        switch error {
        case .cancelled:
            return
        case .throttled:
            backoffS = min(Rule.maxBackoffS, max(Rule.minBackoffS, 2 * backoffS))
            let retry = clock.now().addingTimeInterval(backoffS)
            retryAt = retry
            pause(target, reason: .throttled(retryAt: retry))
            schedule(at: retry, target, .retry)
        case .notFound:
            notFoundFrom = from
            phase = .noRoute(target, .notFound)
        case .network:
            fail(target, reason: .network, error: error)
        case .other(let text):
            fail(target, reason: .failed(text), error: error)
        }
    }

    private func pause(_ target: NavPoint, reason: NoRouteReason) {
        guard case .active(var route) = phase else {
            phase = .noRoute(target, reason)
            return
        }
        route.rerouting = true
        phase = .active(route)
    }

    private func fail(_ target: NavPoint, reason: NoRouteReason, error: RouteError) {
        phase = .noRoute(target, reason)
        if reported.insert(error.label).inserted {
            report(error)
        }
        schedule(at: clock.now().addingTimeInterval(Rule.failureRetryS), target, .retry)
    }

    private func stopRequests() {
        scheduled?.cancel()
        scheduled = nil
        scheduledFor = nil
        awaitingFix = nil
        if inFlight != nil {
            inFlight?.cancel()
            inFlight = nil
            routes.cancel()
        }
        generation += 1
    }
}
