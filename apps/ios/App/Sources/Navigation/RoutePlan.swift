import Foundation
import MapKit
import SdrmmCore

nonisolated struct RawStep: Equatable, Sendable {
    let instruction: String
    let notice: String?
    let distanceM: Double
    let points: [LatLon]
}

nonisolated struct PlanStep: Equatable, Sendable {
    let instruction: String
    let notice: String?
    let distanceM: Double
    let startIndex: Int
    let startM: Double
    let maneuver: ManeuverKind
}

nonisolated struct RoutePlan: Identifiable, Equatable, Sendable {
    let id: UUID
    let name: String
    let distanceM: Double
    let travelTimeS: Double
    let points: [LatLon]
    let cumulativeM: [Double]
    let steps: [PlanStep]
}

nonisolated enum RoutePlanBuilder {
    private static let junctionM = 0.5
    private static let armM = 20.0

    static func plan(name: String, travelTimeS: Double, steps: [RawStep]) -> RoutePlan {
        var points: [LatLon] = []
        var starts: [Int] = []
        for step in steps {
            var own = step.points[...]
            if let first = own.first, let last = points.last, geoDistanceM(from: last, to: first) <= junctionM
            {
                starts.append(points.count - 1)
                own = own.dropFirst()
            } else {
                starts.append(points.count)
            }
            points.append(contentsOf: own)
        }
        let cumulative = cumulativeM(points)
        let planSteps = steps.indices.map { index in
            let start = min(starts[index], max(0, points.count - 1))
            return PlanStep(
                instruction: steps[index].instruction,
                notice: steps[index].notice,
                distanceM: steps[index].distanceM,
                startIndex: start,
                startM: cumulative.isEmpty ? 0 : cumulative[start],
                maneuver: maneuver(index, of: steps.count, at: start, points: points, cumulative: cumulative)
            )
        }
        return RoutePlan(
            id: UUID(),
            name: name,
            distanceM: cumulative.last ?? 0,
            travelTimeS: travelTimeS,
            points: points,
            cumulativeM: cumulative,
            steps: planSteps
        )
    }

    static func cumulativeM(_ points: [LatLon]) -> [Double] {
        var total = 0.0
        var cumulative: [Double] = points.isEmpty ? [] : [0]
        for index in points.indices.dropFirst() {
            total += geoDistanceM(from: points[index - 1], to: points[index])
            cumulative.append(total)
        }
        return cumulative
    }

    private static func maneuver(
        _ index: Int,
        of count: Int,
        at junction: Int,
        points: [LatLon],
        cumulative: [Double]
    ) -> ManeuverKind {
        if index == 0 {
            return .depart
        }
        if index == count - 1 {
            return .arrive
        }
        guard !points.isEmpty else {
            return .straight
        }
        var back = junction
        while back > 0, cumulative[junction] - cumulative[back] < armM {
            back -= 1
        }
        var ahead = junction
        while ahead < points.count - 1, cumulative[ahead] - cumulative[junction] < armM {
            ahead += 1
        }
        guard back != junction, ahead != junction else {
            return .straight
        }
        let inbound = geoBearingDeg(from: points[back], to: points[junction])
        let outbound = geoBearingDeg(from: points[junction], to: points[ahead])
        return ManeuverKind.classify(turnDeg: wrap180(outbound - inbound))
    }

    static func wrap180(_ degrees: Double) -> Double {
        let shifted = (degrees + 540).truncatingRemainder(dividingBy: 360)
        return (shifted < 0 ? shifted + 360 : shifted) - 180
    }
}

nonisolated extension RoutePlanBuilder {
    static func plan(_ route: MKRoute) -> RoutePlan {
        let steps = route.steps.map { step in
            let polyline = step.polyline
            let points = UnsafeBufferPointer(start: polyline.points(), count: polyline.pointCount).map {
                LatLon($0.coordinate)
            }
            return RawStep(
                instruction: step.instructions,
                notice: step.notice,
                distanceM: step.distance,
                points: points
            )
        }
        return plan(name: route.name, travelTimeS: route.expectedTravelTime, steps: steps)
    }
}
