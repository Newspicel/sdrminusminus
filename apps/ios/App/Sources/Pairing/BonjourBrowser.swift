import Foundation
import Network
import SdrmmCore
import dnssd
import os

protocol BonjourBrowsing: AnyObject {
    func start(
        onChange: @escaping @MainActor ([DiscoveredServer]) -> Void,
        onError: @escaping @MainActor (String) -> Void
    )
    func stop()
}

nonisolated enum BonjourTxt {
    static func decode(_ record: NWTXTRecord) -> [String: String] {
        var decoded: [String: String] = [:]
        for (key, entry) in record {
            switch entry {
            case .string(let value): decoded[key] = value
            case .empty: decoded[key] = ""
            case .data, .none: continue
            @unknown default: continue
            }
        }
        return decoded
    }

    static func hostPort(_ endpoint: NWEndpoint?) -> String? {
        guard case .hostPort(let host, let port) = endpoint else {
            return nil
        }
        switch host {
        case .ipv4(let address):
            return "\(address):\(port.rawValue)"
        case .ipv6(let address):
            guard !address.isLinkLocal, !address.isLoopback else {
                return nil
            }
            let text = "\(address)".split(separator: "%").first.map(String.init) ?? "\(address)"
            return "[\(text)]:\(port.rawValue)"
        case .name(let name, _):
            return "\(name):\(port.rawValue)"
        @unknown default:
            return nil
        }
    }
}

final class BonjourBrowser: BonjourBrowsing {
    private nonisolated struct Found: Sendable, Equatable {
        let name: String
        let endpoint: NWEndpoint
        let txt: [String: String]
    }

    private nonisolated static let timeout: DispatchTimeInterval = .seconds(3)
    private let type: String
    private let queue = DispatchQueue(label: "dev.newspicel.sdrmm.bonjour")
    private var browser: NWBrowser?
    private var current: [String: Found] = [:]
    private var resolved: [String: DiscoveredServer] = [:]
    private var resolving: Set<String> = []
    private var onChange: (@MainActor ([DiscoveredServer]) -> Void)?
    private var onError: (@MainActor (String) -> Void)?

    init(type: String = "_sdrmm._tcp") {
        self.type = type
    }

    func start(
        onChange: @escaping @MainActor ([DiscoveredServer]) -> Void,
        onError: @escaping @MainActor (String) -> Void
    ) {
        self.onChange = onChange
        self.onError = onError
        guard browser == nil else {
            publish()
            return
        }
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: type, domain: nil), using: .tcp)
        browser.stateUpdateHandler = { [weak self] state in
            Task { @MainActor in self?.changed(state) }
        }
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            let found = results.compactMap(Self.found)
            Task { @MainActor in self?.update(found) }
        }
        browser.start(queue: queue)
        self.browser = browser
    }

    func stop() {
        browser?.cancel()
        browser = nil
        current = [:]
        resolved = [:]
        resolving = []
    }

    private nonisolated static func found(_ result: NWBrowser.Result) -> Found? {
        guard case .service(let name, _, _, _) = result.endpoint else {
            return nil
        }
        var txt: [String: String] = [:]
        if case .bonjour(let record) = result.metadata {
            txt = BonjourTxt.decode(record)
        }
        return Found(name: name, endpoint: result.endpoint, txt: txt)
    }

    private func changed(_ state: NWBrowser.State) {
        switch state {
        case .ready:
            publish()
        case .failed(let error):
            Log.pair.error("bonjour browse failed: \(error.debugDescription, privacy: .public)")
            stop()
            onError?(Self.denied(error) ? "Local network off" : "Browse failed")
        case .waiting(let error):
            Log.pair.error("bonjour browse waiting: \(error.debugDescription, privacy: .public)")
            onError?(Self.denied(error) ? "Local network off" : "Browse failed")
        default:
            break
        }
    }

    private nonisolated static func denied(_ error: NWError) -> Bool {
        if case .dns(let code) = error {
            return code == DNSServiceErrorType(kDNSServiceErr_PolicyDenied)
        }
        return false
    }

    private func update(_ found: [Found]) {
        let names = Set(found.map(\.name))
        current = Dictionary(found.map { ($0.name, $0) }, uniquingKeysWith: { first, _ in first })
        resolved = resolved.filter { names.contains($0.key) }
        publish()
        for item in found where resolved[item.name] == nil && !resolving.contains(item.name) {
            resolve(item)
        }
    }

    private func resolve(_ item: Found) {
        let queue = queue
        resolving.insert(item.name)
        Task {
            async let v4 = Self.address(item.endpoint, version: .v4, queue: queue)
            async let v6 = Self.address(item.endpoint, version: .v6, queue: queue)
            let hosts = await [v4, v6].compactMap { $0 }
            resolving.remove(item.name)
            guard let latest = current[item.name] else {
                return
            }
            guard latest == item else {
                resolve(latest)
                return
            }
            guard !hosts.isEmpty else {
                Log.pair.error("bonjour: \(item.name, privacy: .private) gave no address")
                return
            }
            resolved[item.name] = DiscoveredServer(
                name: item.txt["n"] ?? item.name,
                hosts: hosts,
                txt: item.txt
            )
            publish()
        }
    }

    private func publish() {
        onChange?(resolved.values.sorted { $0.name < $1.name })
    }

    private nonisolated static func address(
        _ endpoint: NWEndpoint,
        version: NWProtocolIP.Options.Version,
        queue: DispatchQueue
    ) async -> String? {
        let parameters = NWParameters.tcp
        if let ip = parameters.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options {
            ip.version = version
        }
        let connection = NWConnection(to: endpoint, using: parameters)
        return await withCheckedContinuation { continuation in
            let once = FirstTime()
            let finish: @Sendable (String?) -> Void = { value in
                guard once.claim() else {
                    return
                }
                connection.cancel()
                continuation.resume(returning: value)
            }
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready: finish(BonjourTxt.hostPort(connection.currentPath?.remoteEndpoint))
                case .failed, .cancelled: finish(nil)
                default: break
                }
            }
            connection.start(queue: queue)
            queue.asyncAfter(deadline: .now() + timeout) { finish(nil) }
        }
    }
}
