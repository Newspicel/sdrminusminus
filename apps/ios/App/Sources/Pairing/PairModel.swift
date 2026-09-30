import Foundation
import Observation
import SdrmmCore
import VisionKit
import os

@Observable
final class PairModel {
    enum Step: Equatable {
        case choose
        case scanning
        case code(DiscoveredServer)
        case confirm(PairOffer)
        case pairing
        case done(SavedServer)
    }

    static let codeDigits = 8
    private static let linkPrefix = "sdrmm://pair"
    private(set) var step: Step = .choose
    private(set) var nearby: [DiscoveredServer] = []
    private(set) var error: String?
    private(set) var browseError: String?
    var address = ""
    var code = ""
    @ObservationIgnored private let core: any CoreService
    @ObservationIgnored private let browser: any BonjourBrowsing
    @ObservationIgnored private let settings: SettingsStore
    @ObservationIgnored private let onPaired: @MainActor (SavedServer) async -> Void
    @ObservationIgnored private var visible = false
    @ObservationIgnored private var watchedServer: String?

    init(
        core: any CoreService,
        browser: any BonjourBrowsing,
        settings: SettingsStore,
        onPaired: @escaping @MainActor (SavedServer) async -> Void
    ) {
        self.core = core
        self.browser = browser
        self.settings = settings
        self.onPaired = onPaired
    }

    enum Scanner: Equatable {
        case ready, cameraOff, unsupported
    }

    var scannerAvailable: Bool {
        scanner == .ready
    }

    var scanner: Scanner {
        guard DataScannerViewController.isSupported else {
            return .unsupported
        }
        return DataScannerViewController.isAvailable ? .ready : .cameraOff
    }

    func appear() {
        visible = true
        browse()
    }

    func disappear() {
        visible = false
        if watchedServer == nil {
            browser.stop()
        }
    }

    func watchHosts(of serverID: String) {
        guard watchedServer != serverID else {
            return
        }
        watchedServer = serverID
        browse()
    }

    func stopWatchingHosts() {
        guard watchedServer != nil else {
            return
        }
        watchedServer = nil
        if !visible {
            browser.stop()
        }
    }

    func scan() {
        error = nil
        step = .scanning
    }

    func scanned(_ payload: String) {
        accept(link: payload)
    }

    func open(_ url: URL) {
        accept(link: url.absoluteString)
    }

    func scanFailed(_ reason: String) {
        step = .choose
        error = reason
    }

    func choose(_ server: DiscoveredServer) {
        error = nil
        code = ""
        step = .code(server)
    }

    func submitCode() {
        guard case .code(let server) = step else {
            return
        }
        guard Self.isCode(code) else {
            error = "\(Self.codeDigits) digits"
            return
        }
        do {
            received(try core.offerFromDiscovery(server, code: code))
        } catch {
            fail(error)
        }
    }

    func submitManual() async {
        let host = address.trimmingCharacters(in: .whitespaces)
        guard !host.isEmpty else {
            error = "Address missing"
            return
        }
        guard Self.isCode(code) else {
            error = "\(Self.codeDigits) digits"
            return
        }
        error = nil
        step = .pairing
        do {
            received(try await core.offerManual(address: host, code: code))
        } catch {
            step = .choose
            fail(error)
        }
    }

    func trust() async {
        guard case .confirm(let offer) = step else {
            return
        }
        await pair(offer)
    }

    func cancel() {
        step = .choose
        error = nil
        code = ""
    }

    static func isCode(_ text: String) -> Bool {
        text.count == codeDigits && text.allSatisfy { $0.isASCII && $0.isNumber }
    }

    private func accept(link: String) {
        guard link.lowercased().hasPrefix(Self.linkPrefix) else {
            step = .choose
            error = "Bad QR"
            return
        }
        do {
            received(try core.parsePairLink(link))
        } catch {
            step = .choose
            fail(error)
        }
    }

    private func received(_ offer: PairOffer) {
        error = nil
        guard offer.fingerprintShort == nil else {
            step = .confirm(offer)
            return
        }
        step = .pairing
        Task { await pair(offer) }
    }

    private func pair(_ offer: PairOffer) async {
        error = nil
        step = .pairing
        do {
            let server = try await core.pair(offer, phoneName: settings.phoneName)
            step = .done(server)
            code = ""
            address = ""
            await onPaired(server)
            step = .choose
        } catch {
            step = offer.fingerprintShort != nil && Self.retryable(error) ? .confirm(offer) : .choose
            fail(error)
        }
    }

    private static func retryable(_ error: Error) -> Bool {
        switch error as? CoreError {
        case .WrongCode, .CodeExpired, .KeyMismatch, .InvalidLink, .ProtocolMismatch, .Revoked: false
        default: true
        }
    }

    private func fail(_ error: Error) {
        Log.pair.error("pairing failed: \(CoreErrorText.short(error), privacy: .public)")
        self.error = CoreErrorText.short(error)
    }

    private func browse() {
        browser.start(
            onChange: { [weak self] servers in
                self?.found(servers)
            },
            onError: { [weak self] text in
                self?.browseError = text
            }
        )
    }

    private func found(_ servers: [DiscoveredServer]) {
        browseError = nil
        nearby = servers
        guard let id = watchedServer, let match = servers.first(where: { $0.txt["id"] == id }) else {
            return
        }
        do {
            try core.updateHosts(serverID: id, hosts: match.hosts)
        } catch {
            Log.pair.error("update hosts failed: \(CoreErrorText.detail(error), privacy: .private)")
        }
    }
}
