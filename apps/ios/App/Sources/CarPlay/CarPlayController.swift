import CarPlay
import SdrmmCore
import os

private enum CarBar {
    case none, navigate, cancelPreview, end
}

final class CarPlayController {
    private static let renderInterval: TimeInterval = 0.5
    private static let tickInterval: TimeInterval = 1

    private let model: AppModel
    private let interface: CPInterfaceController
    private let window: CPWindow
    private let map = CarMapViewController()
    private let template = CPMapTemplate()
    private let driver: CarNavigationDriver
    private var loops: [ObservationLoop] = []
    private var ticker: Task<Void, Never>?
    private var renderTask: Task<Void, Never>?
    private var pendingScene: CarMapScene?
    private var pendingBearing: CarBearingRay?
    private var lastRender = Date.distantPast
    private var follow: CarFollow = .user
    private var shownAlert: String?
    private var alert: CPAlertTemplate?
    private var retargetSeen = 0
    private var panelBox: AnyObject?
    private var panel: AnyObject?
    private var info: CPInformationTemplate?
    private var lastContent: CarPanelContent?
    private var bar: CarBar?

    init(model: AppModel, interface: CPInterfaceController, window: CPWindow) {
        self.model = model
        self.interface = interface
        self.window = window
        driver = CarNavigationDriver(model: model, template: template, map: map)
    }

    func start() async {
        window.rootViewController = map
        configure()
        do {
            _ = try await interface.setRootTemplate(template, animated: false)
        } catch {
            failed("root template", error)
        }
        retargetSeen = model.df.retargetTick
        driver.onNavigate = { [weak self] in self?.navigate() }
        driver.onChange = { [weak self] in self?.sessionChanged() }
        loops = [
            ObservationLoop { [weak self] in self?.observeLink() },
            ObservationLoop { [weak self] in self?.observeScene() },
            ObservationLoop { [weak self] in self?.observeBar() },
            ObservationLoop { [weak self] in self?.observeRetarget() },
        ]
        ticker = Task { [weak self] in
            while !Task.isCancelled, self != nil {
                self?.tick()
                do {
                    try await Task.sleep(for: .seconds(Self.tickInterval))
                } catch {
                    return
                }
            }
        }
    }

    func teardown() {
        loops.forEach { $0.cancel() }
        loops = []
        ticker?.cancel()
        renderTask?.cancel()
        driver.teardown()
    }

    func styleChanged(_ style: UIUserInterfaceStyle) {
        map.styleChanged(style)
    }

    private func configure() {
        template.automaticallyHidesNavigationBar = false
        template.hidesButtonsWithNavigationBar = false
        template.tripEstimateStyle = .dark
        template.mapDelegate = driver
        template.mapButtons = [
            button("location.fill") { [weak self] in self?.recentre() },
            button("plus.magnifyingglass") { [weak self] in self?.zoom(0.5) },
            button("minus.magnifyingglass") { [weak self] in self?.zoom(2) },
            button("scope") { [weak self] in self?.toggleDf() },
        ]
        template.leadingNavigationBarButtons = [
            CPBarButton(title: "Missions") { [weak self] _ in self?.showMissions() }
        ]
    }

    private func button(_ symbol: String, action: @escaping @MainActor () -> Void) -> CPMapButton {
        let button = CPMapButton { _ in action() }
        button.image = UIImage(systemName: symbol)
        return button
    }

    private func tick() {
        driver.sync()
        refreshDf()
    }

    private func observeLink() {
        let text: String?
        if model.needsPairing {
            text = "Not paired"
        } else {
            switch model.link {
            case .online: text = nil
            case .connecting(let attempt, _): text = attempt >= 2 ? "Offline" : nil
            case .offline, .refused: text = "Offline"
            }
        }
        setAlert(text)
    }

    private func observeScene() {
        let navigating = model.navigation.isNavigating
        if navigating {
            follow = .userHeading
        } else if follow == .userHeading {
            follow = .user
        }
        let scene = CarMapScene.make(
            df: model.df.view,
            plan: driver.previewPlan ?? model.navigation.activePlan,
            layers: model.df.layers,
            follow: follow
        )
        let here = model.navigation.lastLocation.map { LatLon($0.coordinate) }
        requestRender(scene, bearing: CarBearingRay.make(df: model.df.view, here: here))
    }

    private func sessionChanged() {
        refreshBar()
        observeScene()
        if #available(iOS 27.0, *), driver.navigating {
            driver.setOptionsPanel(panel ?? makePanel())
        }
    }

    private func observeBar() {
        _ = model.navigation.phase
        _ = model.openMission
        _ = model.df.view?.target
        refreshBar()
    }

    private func observeRetarget() {
        let tick = model.df.retargetTick
        guard tick != retargetSeen else {
            return
        }
        retargetSeen = tick
        if let notice = model.navigation.lastRetarget {
            driver.presentRetarget(notice)
        }
    }

    private func refreshBar() {
        let next: CarBar
        if model.navigation.isNavigating {
            next = .end
        } else if driver.previewing {
            next = .cancelPreview
        } else if model.openMission?.kind == .dfDrive, model.df.view?.target != nil {
            next = .navigate
        } else {
            next = .none
        }
        guard next != bar else {
            return
        }
        bar = next
        switch next {
        case .end:
            template.trailingNavigationBarButtons = [
                CPBarButton(title: "End") { [weak self] _ in self?.model.navigation.end() }
            ]
        case .navigate:
            template.trailingNavigationBarButtons = [
                CPBarButton(title: "Navigate") { [weak self] _ in self?.navigate() }
            ]
        case .cancelPreview:
            template.trailingNavigationBarButtons = [
                CPBarButton(title: "Cancel") { [weak self] _ in self?.driver.cancelPreviews() }
            ]
        case .none:
            template.trailingNavigationBarButtons = []
        }
    }

    private func requestRender(_ scene: CarMapScene, bearing: CarBearingRay?) {
        let now = Date()
        guard now.timeIntervalSince(lastRender) < Self.renderInterval else {
            lastRender = now
            pendingScene = nil
            pendingBearing = nil
            map.render(scene)
            map.showBearingRay(bearing)
            return
        }
        pendingScene = scene
        pendingBearing = bearing
        guard renderTask == nil else {
            return
        }
        let wait = Self.renderInterval - now.timeIntervalSince(lastRender)
        renderTask = Task { [weak self] in
            do {
                try await Task.sleep(for: .seconds(wait))
            } catch {
                return
            }
            self?.flushRender()
        }
    }

    private func flushRender() {
        renderTask = nil
        guard let scene = pendingScene else {
            return
        }
        pendingScene = nil
        lastRender = Date()
        map.render(scene)
        map.showBearingRay(pendingBearing)
        pendingBearing = nil
    }

    private func recentre() {
        follow = model.navigation.isNavigating ? .userHeading : .user
        map.recentre()
        observeScene()
    }

    private func zoom(_ factor: Double) {
        if !model.navigation.isNavigating {
            follow = .free
        }
        map.zoom(by: factor)
    }

    private func navigate() {
        guard model.settings.routeNoticeAccepted else {
            presentNotice()
            return
        }
        if let info, interface.topTemplate === info {
            pop()
        }
        if model.navigation.isNavigating, let target = model.df.view?.target {
            model.navigation.start(to: target)
            return
        }
        Task { await driver.showPreviews() }
    }

    private func presentNotice() {
        let notice = CPAlertTemplate(
            titleVariants: [RouteNotice.text],
            actions: [
                CPAlertAction(title: "OK", style: .default) { [weak self] _ in
                    self?.model.settings.routeNoticeAccepted = true
                    self?.dismiss("dismiss notice")
                    self?.navigate()
                }
            ]
        )
        present(notice)
    }

    private func setAlert(_ text: String?) {
        guard text != shownAlert else {
            return
        }
        shownAlert = text
        if alert != nil {
            alert = nil
            dismiss("dismiss alert")
        }
        guard let text else {
            return
        }
        let next = CPAlertTemplate(
            titleVariants: [text],
            actions: [CPAlertAction(title: "OK", style: .cancel) { [weak self] _ in self?.dismissAlert() }]
        )
        alert = next
        present(next)
    }

    private func dismissAlert() {
        alert = nil
        dismiss("dismiss alert")
    }

    private func dismiss(_ what: String) {
        interface.dismissTemplate(animated: true, completion: done(what, emptyIsFine: true))
    }

    private func present(_ template: CPTemplate) {
        interface.presentTemplate(template, animated: true, completion: done("present"))
    }

    private func done(_ what: String, emptyIsFine: Bool = false) -> (Bool, (any Error)?) -> Void {
        { [weak self] ok, error in
            guard error != nil || !(ok || emptyIsFine) else {
                return
            }
            Task { @MainActor in self?.failed(what, error) }
        }
    }

    private func failed(_ what: String, _ error: Error?) {
        let text = error?.localizedDescription ?? "rejected"
        Log.carplay.error("\(what, privacy: .public): \(text, privacy: .public)")
        model.show(level: .error, text: "CarPlay error", detail: "\(what): \(text)")
    }
}

extension CarPlayController {
    private var content: CarPanelContent {
        CarPanelContent.make(
            df: model.df.view,
            pose: model.pose,
            here: model.navigation.lastLocation.map { LatLon($0.coordinate) },
            units: model.settings.unitSystem,
            controls: model.openMission?.controls ?? []
        )
    }

    private func actions() -> (
        navigate: @MainActor () -> Void, calibrate: @MainActor () -> Void, clear: @MainActor () -> Void
    ) {
        (
            { [weak self] in self?.navigate() },
            { [weak self] in self?.calibrate() },
            { [weak self] in self?.clear() }
        )
    }

    private func calibrate() {
        Task {
            if await !model.df.calibrate() {
                driver.alert("Calibrate failed", subtitle: nil)
            }
        }
    }

    private func clear() {
        Task {
            let cleared = await model.df.clearFusion()
            driver.alert(cleared ? "Fusion cleared" : "Clear failed", subtitle: nil)
        }
    }

    private func toggleDf() {
        if #available(iOS 27.0, *) {
            togglePanel()
        } else {
            pushInfo()
        }
    }

    @available(iOS 27.0, *)
    private func togglePanel() {
        if bearingPanel().visible, panel != nil {
            template.hidePanel(completion: done("hide panel"))
            return
        }
        let shown = makePanel()
        driver.setOptionsPanel(shown)
        template.showPanel(shown, completion: done("show panel"))
    }

    @available(iOS 27.0, *)
    private func makePanel() -> CPMapPanel {
        let shown = bearingPanel().panel(content, units: model.settings.unitSystem)
        panel = shown
        lastContent = content
        return shown
    }

    @available(iOS 27.0, *)
    private func bearingPanel() -> CarBearingPanel {
        if let existing = panelBox as? CarBearingPanel {
            return existing
        }
        let made = actions()
        let builder = CarBearingPanel(
            onNavigate: made.navigate,
            onCalibrate: made.calibrate,
            onClear: made.clear
        )
        panelBox = builder
        return builder
    }

    private func pushInfo() {
        let made = actions()
        let builder = CarDfInfo(onNavigate: made.navigate, onCalibrate: made.calibrate, onClear: made.clear)
        panelBox = builder
        let shown = builder.template(content)
        info = shown
        lastContent = content
        interface.pushTemplate(shown, animated: true, completion: done("push DF"))
    }

    private func refreshDf() {
        let next = content
        guard next != lastContent else {
            return
        }
        lastContent = next
        if #available(iOS 27.0, *), let shown = panel as? CPMapPanel,
            let builder = panelBox as? CarBearingPanel
        {
            builder.update(shown, with: next, units: model.settings.unitSystem)
            driver.setOptionsPanel(shown)
        }
        if let info, interface.templates.contains(where: { $0 === info }),
            let builder = panelBox as? CarDfInfo
        {
            builder.update(info, with: next)
        }
    }

    private func showMissions() {
        let missions = model.missions?.missions.filter { $0.kind == .dfDrive } ?? []
        let items = missions.map { mission in
            let item = CPListItem(
                text: mission.title,
                detailText: mission.ready ? mission.detail : mission.blocker
            )
            item.isEnabled = mission.ready
            item.handler = { [weak self] _, completion in
                self?.model.open(mission)
                self?.pop()
                completion()
            }
            return item
        }
        let empty = CPListItem(text: "No DF missions", detailText: nil)
        empty.isEnabled = false
        let list = CPListTemplate(
            title: "Missions",
            sections: [CPListSection(items: items.isEmpty ? [empty] : items)]
        )
        interface.pushTemplate(list, animated: true, completion: done("missions"))
    }

    private func pop() {
        interface.popTemplate(animated: true, completion: done("pop", emptyIsFine: true))
    }
}
