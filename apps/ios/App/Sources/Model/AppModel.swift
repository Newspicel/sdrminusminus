import CoreLocation
import Observation
import SdrmmCore
import SwiftUI
import os

enum Screen: Hashable {
    case mission(String)
    case navigation
}

@Observable
final class AppModel {
    private static let kinds: [MissionKind] = [.hunt, .dfDrive, .radarWatch, .survey]

    let core: any CoreService
    let settings: SettingsStore
    let sensors: SensorHub
    let pairing: PairModel
    let hunt: HuntModel
    let df: DfDriveModel
    let navigation: NavigationModel
    let radar: RadarModel
    let survey: SurveyModel
    let audio: AudioSessionController
    let screen: ScreenAwake
    @ObservationIgnored let speech: any SpeechPrompting
    private(set) var link: LinkState = .offline
    private(set) var servers: [SavedServer] = []
    private(set) var missions: MissionsView?
    private(set) var openMission: Mission?
    private(set) var pose: PoseView?
    private(set) var banner: Banner?
    private(set) var carPlayConnected = false
    var showSettings = false
    var showPairSheet = false
    var confirmWorkspace: WorkspaceRef?
    @ObservationIgnored var demoExit: (@MainActor () -> Void)?
    private var storedPath: [Screen] = []
    @ObservationIgnored private let notifier: any RetargetNotifying
    @ObservationIgnored private let network: any NetworkWatching
    @ObservationIgnored private var started = false
    @ObservationIgnored private var running: Task<Void, Never>?
    @ObservationIgnored private var bannerCount = 0
    @ObservationIgnored private var phase: ScenePhase = .active
    @ObservationIgnored private var askedAlerts = false
    @ObservationIgnored private var sceneForeground = true
    @ObservationIgnored private var refusalShown: RefusalKind?

    init(
        core: any CoreService,
        settings: SettingsStore,
        routes: any RouteProviding,
        speech: any SpeechPrompting,
        clicks: any ClickPlaying,
        browser: any BonjourBrowsing,
        sensors: SensorHub,
        notifier: any RetargetNotifying,
        audio: AudioSessionController = AudioSessionController(),
        network: any NetworkWatching = PathWatch()
    ) {
        let relay = ModelRelay()
        let report: @MainActor (Error) -> Void = { relay.model?.report($0) }
        let navigation = NavigationModel(
            routes: routes,
            speech: speech,
            settings: settings,
            clock: SystemNavClock(),
            report: report
        )
        self.core = core
        self.settings = settings
        self.sensors = sensors
        self.notifier = notifier
        self.audio = audio
        self.network = network
        self.navigation = navigation
        self.speech = speech
        pairing = PairModel(core: core, browser: browser, settings: settings) { server in
            await relay.model?.paired(server)
        }
        hunt = HuntModel(core: core, settings: settings, clicks: clicks, report: report)
        df = DfDriveModel(core: core, navigation: navigation, report: report)
        radar = RadarModel()
        survey = SurveyModel(core: core, report: report)
        screen = ScreenAwake()
        relay.model = self
        settings.onPoseChange = { core.setPoseSettings($0) }
        sensors.onFix = { navigation.update(location: $0) }
        df.showNavigation = { relay.model?.path.append(.navigation) }
        audio.onInterruptionEnded = { relay.model?.interruptionEnded() }
    }

    var path: [Screen] {
        get { storedPath }
        set {
            storedPath = newValue
            if let open = openMission, !newValue.contains(.mission(open.id)) {
                closeMission()
            }
        }
    }

    var needsPairing: Bool { servers.isEmpty }

    var groupedMissions: [(MissionKind, [Mission])] {
        let all = missions?.missions ?? []
        return Self.kinds.compactMap { kind in
            let group = all.filter { $0.kind == kind }
            return group.isEmpty ? nil : (kind, group)
        }
    }

    var activeServer: SavedServer? {
        servers.first { $0.id == settings.activeServerID } ?? servers.first
    }

    func start() {
        guard running == nil else {
            return
        }
        running = Task { await run() }
    }

    func run() async {
        guard !started else {
            return
        }
        started = true
        let events = core.events()
        reloadServers()
        core.setPoseSettings(settings.poseSettings)
        network.start { [core] in core.networkChanged() }
        async let connecting: Void = reconnect()
        for await event in events {
            apply(event)
            surfaceDrops()
        }
        network.stop()
        await connecting
        surfaceDrops()
    }

    func connect(serverID: String) async {
        settings.activeServerID = serverID
        refusalShown = nil
        do {
            try await core.connect(serverID: serverID)
        } catch {
            report(error)
        }
    }

    func reconnect() async {
        guard let id = activeServer?.id else {
            return
        }
        await connect(serverID: id)
    }

    func forget(_ server: SavedServer) {
        do {
            try core.forgetServer(id: server.id)
        } catch {
            report(error)
            return
        }
        if settings.activeServerID == server.id {
            closeMission()
            core.disconnect()
            settings.activeServerID = nil
            missions = nil
            storedPath = []
        }
        reloadServers()
        if servers.isEmpty {
            showSettings = false
        }
    }

    func startAlign() {
        core.startAlign()
    }

    func cancelAlign() {
        core.cancelAlign()
    }

    func paired(_ server: SavedServer) async {
        reloadServers()
        showPairSheet = false
        await connect(serverID: server.id)
    }

    func refresh() async {
        do {
            try await core.refreshMissions()
        } catch {
            report(error)
        }
    }

    func switchWorkspace(_ workspace: WorkspaceRef) async {
        confirmWorkspace = nil
        closeMission()
        do {
            try await core.switchWorkspace(id: workspace.id)
        } catch {
            report(error)
        }
    }

    func open(_ mission: Mission) {
        guard mission.ready else {
            show(level: .warn, text: mission.blocker ?? "Not ready", detail: nil)
            return
        }
        closeMission()
        do {
            try core.openMission(id: mission.id)
        } catch {
            report(error)
            return
        }
        openMission = mission
        sensors.start(profile: mission.kind == .dfDrive ? .drive : .walk)
        activateAudio { [weak self] in self?.hunt.resumeClicks() }
        screen.update(missionOpen: true, phase: phase)
        missionOpened(mission)
        storedPath = [.mission(mission.id)]
    }

    func open(missionID: String) {
        guard let mission = missions?.missions.first(where: { $0.id == missionID }) else {
            show(level: .warn, text: "Mission gone", detail: nil)
            return
        }
        open(mission)
    }

    func closeMission() {
        guard openMission != nil else {
            return
        }
        navigation.end()
        core.closeMission()
        sensors.stop()
        hunt.missionClosed()
        radar.missionClosed()
        survey.missionClosed()
        audio.deactivate()
        openMission = nil
        storedPath = []
        screen.update(missionOpen: false, phase: phase)
    }

    func openLink(_ url: URL) {
        pairing.open(url)
        if !needsPairing {
            showPairSheet = true
        }
    }

    func scene(_ phase: ScenePhase) {
        self.phase = phase
        switch phase {
        case .active:
            sceneForeground = true
            syncForeground()
            surfaceAlertFailure()
        case .background:
            sceneForeground = false
            syncForeground()
        default:
            break
        }
        screen.update(missionOpen: openMission != nil, phase: phase)
    }

    func setCarPlay(connected: Bool) {
        carPlayConnected = connected
        syncForeground()
    }

    private func syncForeground() {
        core.setForeground(sceneForeground || carPlayConnected)
    }

    func report(_ error: Error) {
        Log.core.error("\(CoreErrorText.detail(error), privacy: .private)")
        show(level: .error, text: CoreErrorText.short(error), detail: CoreErrorText.detail(error))
    }

    func show(_ banner: Banner) {
        self.banner = banner
    }

    func dismissBanner() {
        banner = nil
    }

    func show(level: NoticeLevel, text: String, detail: String?) {
        bannerCount += 1
        show(Banner(id: bannerCount, level: level, text: text, detail: detail))
    }

    private func reloadServers() {
        do {
            servers = try core.savedServers()
        } catch {
            report(error)
        }
    }

    private func activateAudio(then resume: @escaping @MainActor @Sendable () -> Void) {
        audio.activate { [weak self] failure in
            guard let failure else {
                resume()
                return
            }
            Log.audio.error("audio session: \(failure.reason, privacy: .public)")
            self?.show(level: .warn, text: "Audio off", detail: failure.reason)
        }
    }

    private func missionOpened(_ mission: Mission) {
        switch mission.kind {
        case .hunt:
            hunt.missionOpened()
        case .dfDrive:
            df.missionOpened()
            askForAlertsOnce()
        case .radarWatch:
            radar.missionOpened()
        case .survey:
            survey.missionOpened()
        }
    }

    private func interruptionEnded() {
        guard openMission != nil else {
            return
        }
        activateAudio { [weak self] in self?.hunt.resumeClicks() }
    }

    private func askForAlertsOnce() {
        guard !askedAlerts else {
            return
        }
        askedAlerts = true
        Task {
            if await !notifier.requestAuthorization() {
                show(level: .info, text: "Alerts off", detail: nil)
            }
        }
    }

    private func surfaceAlertFailure() {
        if let failure = notifier.takeFailure() {
            show(level: .warn, text: "Alert failed", detail: failure)
        }
    }

    private func surfaceDrops() {
        let dropped = core.takeDroppedEvents()
        if dropped > 0 {
            show(level: .warn, text: "Missed \(dropped) updates", detail: nil)
        }
    }
}

extension AppModel {
    func apply(_ event: CoreEvent) {
        switch event {
        case .link(let state): applyLink(state)
        case .missions(let view): applyMissions(view)
        case .pose(let view): applyPose(view)
        case .hunt(let view):
            if isOpen(view.mission) { hunt.apply(view) }
        case .df(let view):
            if isOpen(view.mission) { df.apply(view) }
        case .radar(let view):
            if isOpen(view.mission) { radar.apply(view) }
        case .radarImage(let image): radar.apply(image: image)
        case .survey(let view):
            if isOpen(view.mission) { survey.apply(view) }
        case .surveyPoints(let points): survey.append(points)
        case .retarget(let notice): applyRetarget(notice)
        case .notice(let notice): show(level: notice.level, text: notice.text, detail: nil)
        }
    }

    private func isOpen(_ mission: String) -> Bool {
        guard mission == openMission?.id else {
            Log.core.debug("update for \(mission, privacy: .public) while it is not open")
            return false
        }
        return true
    }

    private func applyLink(_ state: LinkState) {
        link = state
        switch state {
        case .connecting(let attempt, _) where attempt >= 2:
            if let id = activeServer?.id {
                pairing.watchHosts(of: id)
            }
        case .refused(.revoked, let text):
            pairing.stopWatchingHosts()
            revoked(text)
        case .refused(let reason, let text):
            pairing.stopWatchingHosts()
            if refusalShown != reason {
                refusalShown = reason
                show(level: .error, text: Self.label(reason), detail: text)
            }
        case .online:
            refusalShown = nil
            pairing.stopWatchingHosts()
        case .offline, .connecting:
            pairing.stopWatchingHosts()
        }
    }

    private func revoked(_ text: String) {
        if let server = activeServer {
            forget(server)
        }
        storedPath = []
        show(level: .error, text: "Phone removed", detail: text)
    }

    private static func label(_ reason: RefusalKind) -> String {
        switch reason {
        case .serverTooOld: "Server too old"
        case .appTooOld: "App too old"
        case .revoked: "Phone removed"
        case .keyMismatch: "Key mismatch"
        }
    }

    private func applyMissions(_ view: MissionsView) {
        missions = view
        guard let open = openMission else {
            return
        }
        if let fresh = view.missions.first(where: { $0.id == open.id }) {
            openMission = fresh
        } else {
            closeMission()
            show(level: .warn, text: "Mission gone", detail: open.title)
        }
    }

    private func applyPose(_ view: PoseView) {
        pose = view
        df.apply(pose: view)
        if case .done(let offset) = view.align,
            abs(SettingsStore.wrap(offset - settings.mountOffsetDeg)) > 0.05
        {
            settings.mountOffsetDeg = offset
        }
    }

    private func applyRetarget(_ notice: RetargetNotice) {
        navigation.retarget(notice)
        df.retargeted(notice)
        let distance = navigation.distance(to: notice.target).map {
            DistanceText.short($0, settings.unitSystem)
        }
        guard phase != .active else {
            let text = distance.map { "New target \($0)" } ?? "New target"
            show(level: .info, text: text, detail: nil)
            return
        }
        notifier.post(notice, distance: distance ?? "-")
    }
}

private final class ModelRelay {
    weak var model: AppModel?
}
