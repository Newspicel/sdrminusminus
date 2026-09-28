import SdrmmCore

nonisolated protocol CoreService: AnyObject, Sendable {
    func about() -> CoreAbout
    func notices() -> [LicenseEntry]
    func savedServers() throws -> [SavedServer]
    func parsePairLink(_ link: String) throws -> PairOffer
    func offerFromDiscovery(_ server: DiscoveredServer, code: String) throws -> PairOffer
    func offerManual(address: String, code: String) async throws -> PairOffer
    func pair(_ offer: PairOffer, phoneName: String) async throws -> SavedServer
    func forgetServer(id: String) throws
    func updateHosts(serverID: String, hosts: [String]) throws
    func connect(serverID: String) async throws
    func disconnect()
    func setForeground(_ foreground: Bool)
    func events() -> AsyncStream<CoreEvent>
    func takeDroppedEvents() -> Int
    func refreshMissions() async throws
    func switchWorkspace(id: String) async throws
    func openMission(id: String) throws
    func closeMission()
    func send(_ command: MissionCommand) async throws
    func pushLocation(_ sample: LocationSample)
    func pushHeading(_ sample: HeadingSample)
    func pushMotion(_ sample: MotionSample)
    func setPoseSettings(_ settings: PoseSettings)
    func startAlign()
    func cancelAlign()
}
