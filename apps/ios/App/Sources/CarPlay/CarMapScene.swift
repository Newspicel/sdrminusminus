import Foundation
import SdrmmCore

nonisolated enum CarFollow: Equatable, Sendable {
    case free, user, userHeading
}

nonisolated struct CarMapScene: Equatable, Sendable {
    var rays: [Ray]
    var ellipse: [LatLon]
    var heat: [HeatBand]
    var stations: [Station]
    var estimate: LatLon?
    var target: NavPoint?
    var route: [LatLon]
    var follow: CarFollow

    static let empty = CarMapScene(
        rays: [],
        ellipse: [],
        heat: [],
        stations: [],
        estimate: nil,
        target: nil,
        route: [],
        follow: .user
    )

    static func make(df: DfView?, plan: RoutePlan?, layers: MapLayers, follow: CarFollow) -> CarMapScene {
        let overlay = df?.overlay
        return CarMapScene(
            rays: layers.rays ? overlay?.rays ?? [] : [],
            ellipse: layers.ellipse ? overlay?.ellipse ?? [] : [],
            heat: layers.heat ? overlay?.heat ?? [] : [],
            stations: overlay?.stations ?? [],
            estimate: df?.estimate?.at,
            target: df?.target,
            route: plan?.points ?? [],
            follow: follow
        )
    }
}

nonisolated struct CarBearingRay: Equatable, Sendable {
    static let defaultLengthM = 5_000.0

    let from: LatLon
    let bearingDeg: Double
    let lengthM: Double

    static func make(df: DfView?, here: LatLon?) -> CarBearingRay? {
        guard let df, df.state == .live, let bearing = df.bearingTrueDeg, let here else {
            return nil
        }
        let length = df.estimate.map { geoDistanceM(from: here, to: $0.at) } ?? defaultLengthM
        return CarBearingRay(from: here, bearingDeg: Double(bearing), lengthM: max(100, length))
    }

    var end: LatLon {
        let radius = 6_371_000.0
        let angular = lengthM / radius
        let theta = bearingDeg * .pi / 180
        let lat1 = from.lat * .pi / 180
        let lon1 = from.lon * .pi / 180
        let lat2 = asin(sin(lat1) * cos(angular) + cos(lat1) * sin(angular) * cos(theta))
        let lon2 = lon1 + atan2(sin(theta) * sin(angular) * cos(lat1), cos(angular) - sin(lat1) * sin(lat2))
        return LatLon(lat: lat2 * 180 / .pi, lon: lon2 * 180 / .pi)
    }
}
