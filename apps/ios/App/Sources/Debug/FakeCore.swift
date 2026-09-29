import Foundation
import SdrmmCore
import Synchronization

nonisolated final class FakeCore: CoreService {
    enum Scenario: String, CaseIterable {
        case fresh, paired, hunt, df, radar, survey, demo

        var streams: [MissionKind] {
            switch self {
            case .fresh, .paired: []
            case .hunt: [.hunt]
            case .df: [.dfDrive]
            case .radar: [.radarWatch]
            case .survey: [.survey]
            case .demo: [.hunt, .dfDrive, .radarWatch, .survey]
            }
        }
    }

    enum Call: Equatable {
        case connect(String)
        case disconnect
        case parse(String)
        case offer(String)
        case pair(PairOffer, String)
        case forget(String)
        case updateHosts(String, [String])
        case refresh
        case switchWorkspace(String)
        case open(String)
        case close
        case send(MissionCommand)
        case location
        case heading
        case motion
        case pose(PoseSettings)
        case startAlign
        case cancelAlign
        case foreground(Bool)
    }

    private struct State {
        var calls: [Call] = []
        var servers: [SavedServer]
        var failNext: CoreError?
        var workspace = FakeScenarios.field
        var openMission: Mission?
        var strength: Float = 0
        var sweeping = false
        var dropped = 0
        var ticker: Task<Void, Never>?
    }

    static let tick: Duration = .milliseconds(200)
    let scenario: Scenario
    private let ticking: Bool
    private let serverName: String
    private let state: Mutex<State>
    private let stream: AsyncStream<CoreEvent>
    private let continuation: AsyncStream<CoreEvent>.Continuation

    init(scenario: Scenario, ticking: Bool = true) {
        self.scenario = scenario
        self.ticking = ticking
        serverName = scenario == .demo ? "Demo" : FakeScenarios.server().name
        let servers = scenario == .fresh ? [] : [FakeScenarios.server(name: serverName)]
        state = Mutex(State(servers: servers))
        (stream, continuation) = AsyncStream.makeStream(
            of: CoreEvent.self,
            bufferingPolicy: .bufferingNewest(256)
        )
    }

    var calls: [Call] { state.withLock { $0.calls } }

    func emit(_ event: CoreEvent) {
        continuation.yield(event)
    }

    func fail(next: CoreError) {
        state.withLock { $0.failNext = next }
    }

    func drop(_ count: Int) {
        state.withLock { $0.dropped += count }
    }

    func finish() {
        state.withLock { $0.ticker?.cancel() }
        continuation.finish()
    }

    private func record(_ call: Call) {
        state.withLock { $0.calls.append(call) }
    }

    private func failure() throws {
        if let error = state.withLock({ state in
            defer { state.failNext = nil }
            return state.failNext
        }) {
            throw error
        }
    }

    func about() -> CoreAbout { CoreAbout(coreVersion: "demo", protocol: 1) }

    func notices() -> [LicenseEntry] {
        [LicenseEntry(name: "SDR--", version: nil, license: "AGPL-3.0-or-later", text: "Demo data")]
    }

    func savedServers() throws -> [SavedServer] {
        try failure()
        return state.withLock { $0.servers }
    }

    func parsePairLink(_ link: String) throws -> PairOffer {
        record(.parse(link))
        try failure()
        guard let components = URLComponents(string: link), components.scheme == "sdrmm",
            components.host == "pair"
        else {
            throw CoreError.InvalidLink(reason: "not a pairing link")
        }
        let items = components.queryItems ?? []
        let code = items.first { $0.name == "c" }?.value ?? ""
        let hosts = items.filter { $0.name == "h" }.compactMap(\.value)
        return try accepted(code: code, hosts: hosts.isEmpty ? ["10.0.0.2:8443"] : hosts, name: "Lab Pi")
    }

    func offerFromDiscovery(_ server: DiscoveredServer, code: String) throws -> PairOffer {
        record(.offer(code))
        try failure()
        return try accepted(code: code, hosts: server.hosts, name: server.name)
    }

    func offerManual(address: String, code: String) async throws -> PairOffer {
        record(.offer(code))
        try failure()
        return try accepted(code: code, hosts: [address], name: nil)
    }

    private func accepted(code: String, hosts: [String], name: String?) throws -> PairOffer {
        guard code == FakeScenarios.code else {
            throw CoreError.WrongCode
        }
        return FakeScenarios.offer(hosts: hosts, code: code, name: name)
    }

    func pair(_ offer: PairOffer, phoneName: String) async throws -> SavedServer {
        record(.pair(offer, phoneName))
        try failure()
        let server = FakeScenarios.server()
        state.withLock { $0.servers = [server] }
        return server
    }

    func forgetServer(id: String) throws {
        record(.forget(id))
        try failure()
        state.withLock { $0.servers.removeAll { $0.id == id } }
    }

    func updateHosts(serverID: String, hosts: [String]) throws {
        record(.updateHosts(serverID, hosts))
        try failure()
    }

    func connect(serverID: String) async throws {
        record(.connect(serverID))
        try failure()
        guard let server = state.withLock({ $0.servers.first { $0.id == serverID } }) else {
            throw CoreError.Internal(message: "Unknown server \(serverID)")
        }
        emit(.link(state: .connecting(attempt: 1, host: server.hosts.first ?? "")))
        emit(.link(state: .online(server: server.name)))
        emit(.missions(view: FakeScenarios.missions(workspace: state.withLock { $0.workspace })))
    }

    func disconnect() {
        record(.disconnect)
        emit(.link(state: .offline))
    }

    func setForeground(_ foreground: Bool) {
        record(.foreground(foreground))
    }

    func events() -> AsyncStream<CoreEvent> { stream }

    func takeDroppedEvents() -> Int {
        state.withLock { state in
            defer { state.dropped = 0 }
            return state.dropped
        }
    }

    func refreshMissions() async throws {
        record(.refresh)
        try failure()
        emit(.missions(view: FakeScenarios.missions(workspace: state.withLock { $0.workspace })))
    }

    func switchWorkspace(id: String) async throws {
        record(.switchWorkspace(id))
        try failure()
        let workspace = [FakeScenarios.field, FakeScenarios.lab].first { $0.id == id } ?? FakeScenarios.field
        state.withLock { $0.workspace = workspace }
        emit(.missions(view: FakeScenarios.missions(workspace: workspace)))
    }

    func openMission(id: String) throws {
        record(.open(id))
        try failure()
        guard let mission = FakeScenarios.missions().missions.first(where: { $0.id == id }) else {
            throw CoreError.NoMission
        }
        state.withLock { state in
            state.ticker?.cancel()
            state.ticker = nil
            state.openMission = mission
            state.strength = 0
            state.sweeping = false
        }
        if scenario.streams.contains(mission.kind) {
            stream(mission)
        }
    }

    func closeMission() {
        record(.close)
        state.withLock { state in
            state.ticker?.cancel()
            state.ticker = nil
            state.openMission = nil
        }
    }

    func send(_ command: MissionCommand) async throws {
        record(.send(command))
        try failure()
        switch command {
        case .startHunt:
            state.withLock { $0.sweeping = false }
            startHunt()
        case .stopHunt: stopHunt()
        case .sweep(let on): sweep(on)
        case .startSurvey: setRecording(true)
        case .stopSurvey: setRecording(false)
        default: break
        }
    }

    func pushLocation(_ sample: LocationSample) { record(.location) }
    func pushHeading(_ sample: HeadingSample) { record(.heading) }
    func pushMotion(_ sample: MotionSample) { record(.motion) }
    func setPoseSettings(_ settings: PoseSettings) { record(.pose(settings)) }

    func startAlign() {
        record(.startAlign)
        var view = FakeScenarios.pose(heading: nil)
        view = PoseView(
            headingDeg: view.headingDeg,
            accuracyDeg: view.accuracyDeg,
            source: .course,
            align: .collecting(progress: 0.3, hint: .driveStraight),
            sending: view.sending,
            fixAgeMs: view.fixAgeMs
        )
        emit(.pose(view: view))
    }

    func cancelAlign() {
        record(.cancelAlign)
        emit(.pose(view: FakeScenarios.pose(heading: nil)))
    }

    private func stream(_ mission: Mission) {
        switch mission.kind {
        case .hunt:
            emit(.hunt(view: huntView(strength: 0, trend: .waiting, running: false)))
        case .dfDrive:
            emit(.pose(view: FakeScenarios.pose(heading: 90)))
            emit(.df(view: FakeScenarios.df(bearing: 137, heading: 90)))
            if ticking {
                schedule { core in
                    try await Task.sleep(for: .seconds(3))
                    core.emit(.retarget(notice: FakeScenarios.retarget()))
                }
            }
        case .radarWatch:
            emit(.radar(view: FakeScenarios.radar()))
            emit(.radarImage(image: FakeScenarios.radarImage()))
        case .survey:
            emit(.survey(view: FakeScenarios.survey()))
            emit(.surveyPoints(points: FakeScenarios.surveyPoints()))
        }
    }

    private func startHunt() {
        guard streaming(.hunt) else {
            return
        }
        guard ticking else {
            emit(.hunt(view: huntView(strength: 0.6, trend: .warmer, running: true)))
            return
        }
        schedule { core in
            while !Task.isCancelled {
                let strength = core.state.withLock { state in
                    state.strength = min(1, state.strength + 0.04)
                    return state.strength
                }
                let trend: Trend = strength >= 1 ? .onTop : .warmer
                core.emit(.hunt(view: core.huntView(strength: strength, trend: trend, running: true)))
                try await Task.sleep(for: FakeCore.tick)
            }
        }
    }

    private func stopHunt() {
        let strength = state.withLock { state in
            state.ticker?.cancel()
            state.ticker = nil
            return state.strength
        }
        guard scenario.streams.contains(.hunt) else {
            return
        }
        emit(.hunt(view: huntView(strength: strength, trend: .waiting, running: false)))
    }

    private func sweep(_ on: Bool) {
        state.withLock { $0.sweeping = on }
        if on || ticking {
            startHunt()
        } else if streaming(.hunt) {
            emit(.hunt(view: huntView(strength: 0.6, trend: .warmer, running: true)))
        }
    }

    private func setRecording(_ on: Bool) {
        if streaming(.survey) {
            emit(.survey(view: FakeScenarios.survey(recording: on)))
        }
    }

    private func streaming(_ kind: MissionKind) -> Bool {
        state.withLock { $0.openMission?.kind == kind } && scenario.streams.contains(kind)
    }

    private func huntView(strength: Float, trend: Trend, running: Bool) -> HuntView {
        let sweeping = state.withLock { $0.sweeping }
        return FakeScenarios.hunt(
            strength: strength,
            trend: trend,
            running: running,
            sweep: sweeping ? FakeScenarios.sweep() : nil
        )
    }

    private func schedule(_ work: @escaping @Sendable (FakeCore) async throws -> Void) {
        let task = Task { [weak self] in
            guard let self else {
                return
            }
            try? await work(self)
        }
        state.withLock { state in
            state.ticker?.cancel()
            state.ticker = task
        }
    }
}
