import Foundation
import Network

protocol NetworkWatching: AnyObject {
    func start(onChange: @escaping @MainActor () -> Void)
    func stop()
}

nonisolated struct PathSummary: Equatable, Sendable {
    let usable: Bool
    let interfaces: [String]
    let gateways: [String]

    init(usable: Bool, interfaces: [String], gateways: [String]) {
        self.usable = usable
        self.interfaces = interfaces
        self.gateways = gateways
    }

    init(_ path: NWPath) {
        self.init(
            usable: path.status == .satisfied,
            interfaces: path.availableInterfaces.map(\.name),
            gateways: path.gateways.map(\.debugDescription).sorted()
        )
    }

    static func matters(from previous: PathSummary?, to next: PathSummary) -> Bool {
        guard let previous else {
            return false
        }
        return next.usable && previous != next
    }
}

final class PathWatch: NetworkWatching {
    private let queue = DispatchQueue(label: "dev.newspicel.sdrmm.path")
    private var monitor: NWPathMonitor?
    private var last: PathSummary?
    private var onChange: (@MainActor () -> Void)?

    func start(onChange: @escaping @MainActor () -> Void) {
        self.onChange = onChange
        guard monitor == nil else {
            return
        }
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            let summary = PathSummary(path)
            Task { @MainActor in self?.update(summary) }
        }
        monitor.start(queue: queue)
        self.monitor = monitor
    }

    func stop() {
        monitor?.cancel()
        monitor = nil
        last = nil
        onChange = nil
    }

    private func update(_ summary: PathSummary) {
        let previous = last
        last = summary
        if PathSummary.matters(from: previous, to: summary) {
            onChange?()
        }
    }
}
