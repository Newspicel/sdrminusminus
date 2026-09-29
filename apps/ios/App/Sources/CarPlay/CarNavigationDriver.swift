import CarPlay
import MapKit
import SdrmmCore
import os

final class CarNavigationDriver: NSObject, CPMapTemplateDelegate {
    private static let symbolSize = UIImage.SymbolConfiguration(pointSize: 48, weight: .bold)
    private static let minimumSpeedMps = 5.0

    private let model: AppModel
    private let template: CPMapTemplate
    private let map: CarMapViewController
    private var session: CPNavigationSession?
    private var sessionPlanID: UUID?
    private var previews: [UUID: RoutePlan] = [:]
    private var previewTarget: NavPoint?
    private var maneuvers: [Int: CPManeuver] = [:]
    private var shownSteps: [Int] = []
    private var restarting = false
    private var pausedFor: String?
    private var currentStep: Int?
    private var stepSince = Date()
    private(set) var previewPlan: RoutePlan?
    var onChange: (@MainActor () -> Void)?
    var onNavigate: (@MainActor () -> Void)?

    init(model: AppModel, template: CPMapTemplate, map: CarMapViewController) {
        self.model = model
        self.template = template
        self.map = map
    }

    var navigating: Bool { session != nil }

    var previewing: Bool { previewTarget != nil && session == nil }

    func cancelPreviews() {
        template.hideTripPreviews()
        clearPreviews()
        onChange?()
    }

    func showPreviews() async {
        guard let target = model.df.view?.target else {
            alert("No target", subtitle: nil)
            return
        }
        guard let here = model.navigation.lastLocation.map({ LatLon($0.coordinate) }) else {
            alert("No location", subtitle: nil)
            return
        }
        do throws(RouteError) {
            let plans = try await model.navigation.previews(to: target)
            guard !plans.isEmpty else {
                throw .notFound
            }
            previews = Dictionary(uniqueKeysWithValues: plans.map { ($0.id, $0) })
            previewTarget = target
            previewPlan = plans.first
            let trip = CarTripFactory.trip(
                origin: here,
                target: target,
                plans: plans,
                units: model.settings.unitSystem
            )
            let text = CPTripPreviewTextConfiguration(
                startButtonTitle: "Go",
                additionalRoutesButtonTitle: "Routes",
                overviewButtonTitle: "Overview"
            )
            template.showTripPreviews([trip], textConfiguration: text)
            onChange?()
        } catch {
            Log.carplay.error("previews: \(error.detail, privacy: .public)")
            alert(error.label, subtitle: nil)
        }
    }

    func sync() {
        switch model.navigation.phase {
        case .idle:
            endSession()
        case .arrived:
            finishSession()
        case .routing:
            pause(.loading, "Routing")
        case .noRoute(_, .throttled):
            pause(.loading, "Routing paused")
        case .noRoute:
            pause(.proceedToRoute, "No route")
        case .active(let route):
            drive(route)
        }
    }

    func presentRetarget(_ notice: RetargetNotice) {
        let distance = model.navigation.distance(to: notice.target).map {
            DistanceText.short($0, model.settings.unitSystem)
        }
        let primary: CPAlertAction
        var secondary: CPAlertAction?
        if navigating {
            primary = CPAlertAction(title: "OK", style: .default) { _ in }
        } else {
            primary = CPAlertAction(title: "Navigate", style: .default) { [weak self] _ in
                self?.onNavigate?()
            }
            secondary = CPAlertAction(title: "Later", style: .cancel) { _ in }
        }
        let alert = CPNavigationAlert(
            titleVariants: [RetargetText.title],
            subtitleVariants: distance.map { [$0] } ?? [],
            image: UIImage(systemName: "scope"),
            primaryAction: primary,
            secondaryAction: secondary,
            duration: 8
        )
        present(alert)
    }

    func alert(_ title: String, subtitle: String?, seconds: TimeInterval = 3) {
        let alert = CPNavigationAlert(
            titleVariants: [title],
            subtitleVariants: subtitle.map { [$0] } ?? [],
            image: nil,
            primaryAction: CPAlertAction(title: "OK", style: .default) { _ in },
            secondaryAction: nil,
            duration: seconds
        )
        present(alert)
    }

    func setOptionsPanel(_ panel: AnyObject?) {
        guard #available(iOS 27.0, *), let session else {
            return
        }
        session.optionsPanel = panel as? CPMapPanel
    }

    func teardown() {
        if session != nil {
            restarting = true
            session?.cancelTrip()
            session = nil
        }
    }

    private func present(_ alert: CPNavigationAlert) {
        guard template.currentNavigationAlert != nil else {
            template.present(navigationAlert: alert, animated: true)
            return
        }
        template.dismissNavigationAlert(animated: false) { [weak self] _ in
            Task { @MainActor in self?.template.present(navigationAlert: alert, animated: true) }
        }
    }

    private func drive(_ route: ActiveRoute) {
        if session == nil || sessionPlanID != route.plan.id {
            begin(route)
        }
        if route.rerouting {
            pause(.rerouting, model.navigation.retryAt == nil ? PromptText.rerouting : "Routing paused")
            return
        }
        if pausedFor != nil {
            pausedFor = nil
            shownSteps = []
        }
        updateManeuvers(route)
        updateEstimates(route)
    }

    private func begin(_ route: ActiveRoute) {
        guard
            let here = model.navigation.lastLocation.map({ LatLon($0.coordinate) }) ?? route.plan.points.first
        else {
            return
        }
        if let session {
            restarting = true
            session.cancelTrip()
            self.session = nil
        }
        template.hideTripPreviews()
        let trip = CarTripFactory.trip(
            origin: here,
            target: route.target,
            plans: [route.plan],
            units: model.settings.unitSystem
        )
        start(trip, plan: route.plan)
    }

    private func start(_ trip: CPTrip, plan: RoutePlan) {
        let session = template.startNavigationSession(for: trip)
        self.session = session
        sessionPlanID = plan.id
        maneuvers = Self.maneuvers(for: plan, units: model.settings.unitSystem)
        session.add(maneuvers.keys.sorted().compactMap { maneuvers[$0] })
        shownSteps = []
        pausedFor = nil
        currentStep = nil
        clearPreviews()
        map.navigating = true
        settleRestart()
        onChange?()
    }

    private func settleRestart() {
        Task { [weak self] in
            do {
                try await Task.sleep(for: .seconds(1))
            } catch {
                return
            }
            self?.restarting = false
        }
    }

    private static func maneuvers(for plan: RoutePlan, units: UnitSystem) -> [Int: CPManeuver] {
        var built: [Int: CPManeuver] = [:]
        for spec in CarManeuvers.specs(for: plan) {
            let maneuver = CPManeuver()
            maneuver.instructionVariants = [spec.instruction]
            maneuver.symbolImage = UIImage(systemName: spec.symbolName, withConfiguration: symbolSize)
            maneuver.maneuverType = spec.kind.carPlayType
            maneuver.initialTravelEstimates = CPTravelEstimates(
                distanceRemaining: CarUnits.measure(spec.distanceM, units),
                timeRemaining: 0
            )
            maneuver.userInfo = spec.step
            built[spec.step] = maneuver
        }
        return built
    }

    private func updateManeuvers(_ route: ActiveRoute) {
        guard let session, let position = route.position else {
            return
        }
        let upcoming = CarManeuvers.upcoming(plan: route.plan, position: position)
        let steps = upcoming.map(\.step)
        if steps != shownSteps {
            shownSteps = steps
            session.upcomingManeuvers = steps.compactMap { maneuvers[$0] }
        }
        if position.nextStep != currentStep {
            currentStep = position.nextStep
            stepSince = Date()
        }
        if let toNext = position.toNextM {
            let state = CarManeuvers.state(toNextM: toNext, stepAgeS: Date().timeIntervalSince(stepSince))
            session.maneuverState = state.carPlayState
        }
    }

    private func updateEstimates(_ route: ActiveRoute) {
        guard let session, let position = route.position else {
            return
        }
        let units = model.settings.unitSystem
        let speed = max(Self.minimumSpeedMps, model.navigation.lastLocation?.speed ?? 0)
        if let step = position.nextStep, let maneuver = maneuvers[step], let toNext = position.toNextM {
            session.updateEstimates(
                CPTravelEstimates(
                    distanceRemaining: CarUnits.measure(toNext, units),
                    timeRemaining: toNext / speed
                ),
                for: maneuver
            )
        }
        let eta =
            route.plan.distanceM > 0 ? route.plan.travelTimeS * position.remainingM / route.plan.distanceM : 0
        template.updateEstimates(
            CPTravelEstimates(
                distanceRemaining: CarUnits.measure(position.remainingM, units),
                timeRemaining: eta
            ),
            for: session.trip
        )
    }

    private func pause(_ reason: CPNavigationSession.PauseReason, _ text: String) {
        guard let session, pausedFor != text else {
            return
        }
        pausedFor = text
        session.pauseTrip(for: reason, description: text)
    }

    private func finishSession() {
        guard let session else {
            return
        }
        restarting = true
        session.finishTrip()
        close()
    }

    private func endSession() {
        guard let session else {
            return
        }
        restarting = true
        session.cancelTrip()
        close()
    }

    private func close() {
        session = nil
        sessionPlanID = nil
        maneuvers = [:]
        shownSteps = []
        pausedFor = nil
        map.navigating = false
        settleRestart()
        onChange?()
    }

    func mapTemplate(
        _ mapTemplate: CPMapTemplate,
        selectedPreviewFor trip: CPTrip,
        using routeChoice: CPRouteChoice
    ) {
        previewPlan = CarTripFactory.planID(routeChoice).flatMap { previews[$0] } ?? previewPlan
        onChange?()
    }

    func mapTemplate(_ mapTemplate: CPMapTemplate, startedTrip trip: CPTrip, using routeChoice: CPRouteChoice)
    {
        mapTemplate.hideTripPreviews()
        guard let target = previewTarget,
            let plan = CarTripFactory.planID(routeChoice).flatMap({ previews[$0] }) ?? previewPlan
        else {
            alert("No route", subtitle: nil)
            return
        }
        model.navigation.start(plan: plan, to: target)
        start(trip, plan: plan)
    }

    private func clearPreviews() {
        previews = [:]
        previewTarget = nil
        previewPlan = nil
    }

    func mapTemplateDidCancelNavigation(_ mapTemplate: CPMapTemplate) {
        guard !restarting else {
            return
        }
        session = nil
        close()
        model.navigation.end()
    }

    func mapTemplateShouldProvideNavigationMetadata(_ mapTemplate: CPMapTemplate) -> Bool {
        true
    }

    func mapTemplate(_ mapTemplate: CPMapTemplate, displayStyleFor maneuver: CPManeuver)
        -> CPManeuverDisplayStyle
    {
        .leadingSymbol
    }

    func mapTemplate(_ mapTemplate: CPMapTemplate, shouldShowNotificationFor maneuver: CPManeuver) -> Bool {
        true
    }

    func mapTemplate(
        _ mapTemplate: CPMapTemplate,
        shouldUpdateNotificationFor maneuver: CPManeuver,
        with travelEstimates: CPTravelEstimates
    ) -> Bool {
        true
    }

    func mapTemplate(
        _ mapTemplate: CPMapTemplate,
        shouldShowNotificationFor navigationAlert: CPNavigationAlert
    ) -> Bool {
        true
    }

    @available(iOS 26.0, *)
    func mapTemplateDidBeginZoomGesture(_ mapTemplate: CPMapTemplate) {
        map.beginZoomGesture()
    }

    @available(iOS 26.0, *)
    func mapTemplate(
        _ mapTemplate: CPMapTemplate,
        didUpdateZoomGestureWithCenter center: CGPoint,
        scale: CGFloat,
        velocity: CGFloat
    ) {
        map.updateZoomGesture(scale: Double(scale))
    }

    @available(iOS 26.0, *)
    func mapTemplate(_ mapTemplate: CPMapTemplate, didEndZoomGestureWithVelocity velocity: CGFloat) {
        map.endZoomGesture()
    }
}
