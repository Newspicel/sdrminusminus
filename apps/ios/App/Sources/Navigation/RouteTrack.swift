import Foundation
import SdrmmCore

nonisolated struct RoutePosition: Equatable, Sendable {
    let alongM: Double
    let offRouteM: Double
    let segment: Int
    let nextStep: Int?
    let toNextM: Double?
    let remainingM: Double
}

nonisolated struct RouteTrack {
    private static let earthRadiusM = 6_371_000.0
    private static let windowBehind = 2
    private static let windowAhead = 40
    private static let recoverM = 50.0
    private static let passedM = 5.0

    let plan: RoutePlan
    private(set) var segment = 0

    init(plan: RoutePlan) {
        self.plan = plan
    }

    mutating func locate(_ point: LatLon) -> RoutePosition {
        let points = plan.points
        guard points.count >= 2 else {
            let off = points.first.map { geoDistanceM(from: point, to: $0) } ?? 0
            return RoutePosition(
                alongM: 0,
                offRouteM: off,
                segment: 0,
                nextStep: nil,
                toNextM: nil,
                remainingM: 0
            )
        }
        let last = points.count - 2
        let low = max(0, min(segment, last) - Self.windowBehind)
        let high = min(last, segment + Self.windowAhead)
        var best = nearest(to: point, in: low...high)
        if best.distance > Self.recoverM {
            best = nearest(to: point, in: 0...last)
        }
        segment = best.segment
        let next = nextStep(after: best.along)
        return RoutePosition(
            alongM: best.along,
            offRouteM: best.distance,
            segment: best.segment,
            nextStep: next,
            toNextM: next.map { plan.steps[$0].startM - best.along },
            remainingM: max(0, (plan.cumulativeM.last ?? 0) - best.along)
        )
    }

    private struct Candidate {
        let segment: Int
        let distance: Double
        let along: Double
    }

    private func nearest(to point: LatLon, in range: ClosedRange<Int>) -> Candidate {
        var best = Candidate(segment: range.lowerBound, distance: .infinity, along: 0)
        for index in range {
            let candidate = project(point, onto: index)
            if candidate.distance < best.distance {
                best = candidate
            }
        }
        return best
    }

    private func project(_ point: LatLon, onto index: Int) -> Candidate {
        let start = local(plan.points[index], around: point)
        let end = local(plan.points[index + 1], around: point)
        let dx = end.x - start.x
        let dy = end.y - start.y
        let length = dx * dx + dy * dy
        let t = length > 0 ? min(1, max(0, -(start.x * dx + start.y * dy) / length)) : 0
        let closestX = start.x + t * dx
        let closestY = start.y + t * dy
        let from = plan.cumulativeM[index]
        let to = plan.cumulativeM[index + 1]
        return Candidate(
            segment: index,
            distance: (closestX * closestX + closestY * closestY).squareRoot(),
            along: from + t * (to - from)
        )
    }

    private func local(_ point: LatLon, around origin: LatLon) -> (x: Double, y: Double) {
        let radians = Double.pi / 180
        let x = Self.earthRadiusM * cos(origin.lat * radians) * (point.lon - origin.lon) * radians
        let y = Self.earthRadiusM * (point.lat - origin.lat) * radians
        return (x, y)
    }

    private func nextStep(after along: Double) -> Int? {
        plan.steps.indices.dropFirst().first { plan.steps[$0].startM > along + Self.passedM }
    }
}
