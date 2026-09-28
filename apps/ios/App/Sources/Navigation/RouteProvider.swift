import MapKit
import SdrmmCore

nonisolated enum RouteError: Error, Equatable, Sendable {
    case notFound, throttled, network, cancelled
    case other(String)
}

protocol RouteProviding: AnyObject {
    func routes(from: LatLon, to: LatLon, alternatives: Bool) async throws(RouteError) -> [RoutePlan]
    func cancel()
}

final class MapKitRouteProvider: RouteProviding {
    func routes(from: LatLon, to: LatLon, alternatives: Bool) async throws(RouteError) -> [RoutePlan] {
        throw .other("Navigation not built yet")
    }

    func cancel() {}
}
