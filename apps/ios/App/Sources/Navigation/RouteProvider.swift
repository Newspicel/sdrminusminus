import MapKit
import SdrmmCore

nonisolated enum RouteError: Error, Equatable, Sendable {
    case notFound, throttled, network, cancelled
    case other(String)

    var label: String {
        switch self {
        case .notFound: "No route"
        case .throttled: "Routing paused"
        case .network: "No network"
        case .cancelled: "Cancelled"
        case .other: "Routing failed"
        }
    }

    var detail: String {
        switch self {
        case .other(let text): "Routing failed: \(text)"
        default: label
        }
    }
}

protocol RouteProviding: AnyObject {
    func routes(from: LatLon, to: LatLon, alternatives: Bool) async throws(RouteError) -> [RoutePlan]
    func cancel()
}

final class MapKitRouteProvider: RouteProviding {
    private var pending: [ObjectIdentifier: PendingDirections] = [:]

    func routes(from: LatLon, to: LatLon, alternatives: Bool) async throws(RouteError) -> [RoutePlan] {
        let request = MKDirections.Request()
        request.source = Self.item(from)
        request.destination = Self.item(to)
        request.transportType = .automobile
        request.requestsAlternateRoutes = alternatives
        let job = PendingDirections(MKDirections(request: request))
        let key = ObjectIdentifier(job)
        pending[key] = job
        defer { pending[key] = nil }
        switch await job.run() {
        case .success(let plans): return plans
        case .failure(let error): throw error
        }
    }

    func cancel() {
        let jobs = pending.values
        pending = [:]
        for job in jobs {
            job.cancel()
        }
    }

    private static func item(_ point: LatLon) -> MKMapItem {
        if #available(iOS 26.0, *) {
            return MKMapItem(location: CLLocation(latitude: point.lat, longitude: point.lon), address: nil)
        }
        return MKMapItem(placemark: MKPlacemark(coordinate: point.coordinate))
    }

    nonisolated static func map(_ error: Error) -> RouteError {
        if error is CancellationError {
            return .cancelled
        }
        let ns = error as NSError
        if ns.domain == MKErrorDomain, let code = MKError.Code(rawValue: UInt(ns.code)) {
            switch code {
            case .loadingThrottled: return .throttled
            case .directionsNotFound, .placemarkNotFound: return .notFound
            case .serverFailure: return .network
            default: return .other(ns.localizedDescription)
            }
        }
        if ns.domain == NSURLErrorDomain {
            return ns.code == NSURLErrorCancelled ? .cancelled : .network
        }
        return .other(ns.localizedDescription)
    }
}

private final class PendingDirections {
    private let directions: MKDirections
    private var continuation: CheckedContinuation<Result<[RoutePlan], RouteError>, Never>?
    private var finished = false

    init(_ directions: MKDirections) {
        self.directions = directions
    }

    func run() async -> Result<[RoutePlan], RouteError> {
        await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                start(continuation)
            }
        } onCancel: {
            Task { @MainActor [weak self] in self?.cancel() }
        }
    }

    func cancel() {
        guard !finished else {
            return
        }
        directions.cancel()
        finish(.failure(.cancelled))
    }

    private func start(_ continuation: CheckedContinuation<Result<[RoutePlan], RouteError>, Never>) {
        guard !finished else {
            continuation.resume(returning: .failure(.cancelled))
            return
        }
        self.continuation = continuation
        directions.calculate { [weak self] response, error in
            let result = Self.result(response, error)
            Task { @MainActor in self?.finish(result) }
        }
    }

    nonisolated private static func result(
        _ response: MKDirections.Response?,
        _ error: Error?
    ) -> Result<[RoutePlan], RouteError> {
        if let error {
            return .failure(MapKitRouteProvider.map(error))
        }
        let plans = (response?.routes ?? []).map { RoutePlanBuilder.plan($0) }
        return plans.isEmpty ? .failure(.notFound) : .success(plans)
    }

    private func finish(_ result: Result<[RoutePlan], RouteError>) {
        finished = true
        guard let continuation else {
            return
        }
        self.continuation = nil
        continuation.resume(returning: result)
    }
}
