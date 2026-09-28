import Foundation
import SdrmmCore
import Synchronization
import os

nonisolated final class LiveCore: CoreService {
    private static let buffered = 256
    private let core: MobileCore
    private let dropped = Atomic<Int>(0)
    private let eventsTaken = Atomic<Bool>(false)

    init(config: CoreConfig, vault: KeychainVault) throws {
        core = try MobileCore(config: config, vault: vault)
        core.setLogListener(listener: CoreLogBridge())
    }

    deinit {
        core.shutdown()
    }

    func about() -> CoreAbout { core.about() }
    func notices() -> [LicenseEntry] { core.notices() }
    func savedServers() throws -> [SavedServer] { try core.savedServers() }
    func parsePairLink(_ link: String) throws -> PairOffer { try core.parsePairLink(link: link) }

    func offerFromDiscovery(_ server: DiscoveredServer, code: String) throws -> PairOffer {
        try core.offerFromDiscovery(server: server, code: code)
    }

    func offerManual(address: String, code: String) async throws -> PairOffer {
        try await core.offerManual(address: address, code: code)
    }

    func pair(_ offer: PairOffer, phoneName: String) async throws -> SavedServer {
        try await core.pair(offer: offer, phoneName: phoneName)
    }

    func forgetServer(id: String) throws { try core.forgetServer(id: id) }

    func updateHosts(serverID: String, hosts: [String]) throws {
        try core.updateHosts(serverId: serverID, hosts: hosts)
    }

    func connect(serverID: String) async throws { try await core.connect(serverId: serverID) }
    func disconnect() { core.disconnect() }
    func setForeground(_ foreground: Bool) { core.setForeground(foreground: foreground) }

    func events() -> AsyncStream<CoreEvent> {
        guard !eventsTaken.exchange(true, ordering: .acquiringAndReleasing) else {
            Log.core.error("events() called twice")
            return AsyncStream { $0.finish() }
        }
        let (stream, continuation) = AsyncStream.makeStream(
            of: CoreEvent.self,
            bufferingPolicy: .bufferingNewest(Self.buffered)
        )
        let pump = Task.detached { [core, weak self] in
            while !Task.isCancelled, let event = await core.nextEvent() {
                switch continuation.yield(event) {
                case .dropped: self?.dropped.add(1, ordering: .relaxed)
                case .terminated: return
                case .enqueued: continue
                @unknown default: continue
                }
            }
            continuation.finish()
        }
        continuation.onTermination = { _ in pump.cancel() }
        return stream
    }

    func takeDroppedEvents() -> Int { dropped.exchange(0, ordering: .relaxed) }
    func refreshMissions() async throws { try await core.refreshMissions() }
    func switchWorkspace(id: String) async throws { try await core.switchWorkspace(id: id) }
    func openMission(id: String) throws { try core.openMission(id: id) }
    func closeMission() { core.closeMission() }
    func send(_ command: MissionCommand) async throws { try await core.send(command: command) }
    func pushLocation(_ sample: LocationSample) { core.pushLocation(sample: sample) }
    func pushHeading(_ sample: HeadingSample) { core.pushHeading(sample: sample) }
    func pushMotion(_ sample: MotionSample) { core.pushMotion(sample: sample) }
    func setPoseSettings(_ settings: PoseSettings) { core.setPoseSettings(settings: settings) }
    func startAlign() { core.startAlign() }
    func cancelAlign() { core.cancelAlign() }
}

nonisolated final class CoreLogBridge: LogListener {
    func onLog(level: LogLevel, target: String, message: String) {
        switch level {
        case .error:
            Log.core.error("\(target, privacy: .public) \(message, privacy: .private)")
        case .warn:
            Log.core.warning("\(target, privacy: .public) \(message, privacy: .private)")
        case .info:
            Log.core.info("\(target, privacy: .public) \(message, privacy: .private)")
        case .debug:
            Log.core.debug("\(target, privacy: .public) \(message, privacy: .private)")
        }
    }
}
