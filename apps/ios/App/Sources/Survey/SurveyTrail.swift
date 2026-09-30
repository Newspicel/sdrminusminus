import Foundation
import MapKit
import SdrmmCore

nonisolated struct TrailRun: Identifiable, Equatable, Sendable {
    let id: Int
    let bin: Int
    var coordinates: [LatLon]
}

nonisolated struct LevelRange: Equatable, Sendable {
    let min: Float
    let max: Float

    func shifted(from other: LevelRange, by limit: Float) -> Bool {
        !(abs(min - other.min) <= limit && abs(max - other.max) <= limit)
    }
}

nonisolated enum SurveyTrail {
    static let bins = 8
    static let maxPoints = 20_000
    static let keptPoints = 18_000
    static let maxRuns = 1_500
    static let rebuildShiftDb: Float = 3
    static let rebuildInterval: TimeInterval = 2

    static func bin(level: Float, min: Float, max: Float) -> Int {
        guard level.isFinite else {
            return 0
        }
        let flat = !(max - min >= 1)
        let low = flat ? level - 0.5 : min
        let high = flat ? level + 0.5 : max
        let scaled = ((level - low) / (high - low) * Float(bins)).rounded(.down)
        return Swift.min(Swift.max(Int(scaled), 0), bins - 1)
    }

    static func runs(from points: [SurveyPoint], min: Float, max: Float) -> [TrailRun] {
        var runs: [TrailRun] = []
        var nextID = 0
        extend(&runs, with: points, min: min, max: max, nextID: &nextID)
        return runs
    }

    static func extend(
        _ runs: inout [TrailRun],
        with points: [SurveyPoint],
        min: Float,
        max: Float,
        nextID: inout Int
    ) {
        for point in points {
            let bin = bin(level: point.levelDb, min: min, max: max)
            if let last = runs.indices.last {
                runs[last].coordinates.append(point.at)
                if runs[last].bin == bin {
                    continue
                }
            }
            runs.append(TrailRun(id: nextID, bin: bin, coordinates: [point.at]))
            nextID += 1
        }
    }

    static func trim(_ runs: inout [TrailRun]) -> Bool {
        guard runs.count > maxRuns else {
            return false
        }
        runs.removeFirst(runs.count - maxRuns)
        return true
    }

    static func range(of points: [SurveyPoint]) -> LevelRange? {
        let levels = points.map(\.levelDb).filter(\.isFinite)
        guard let low = levels.min(), let high = levels.max() else {
            return nil
        }
        return LevelRange(min: low, max: high)
    }

    static func region(_ coordinates: [LatLon]) -> MKCoordinateRegion? {
        guard let first = coordinates.first else {
            return nil
        }
        var south = first.lat
        var north = first.lat
        var west = first.lon
        var east = first.lon
        for point in coordinates.dropFirst() {
            south = Swift.min(south, point.lat)
            north = Swift.max(north, point.lat)
            west = Swift.min(west, point.lon)
            east = Swift.max(east, point.lon)
        }
        let center = CLLocationCoordinate2D(latitude: (south + north) / 2, longitude: (west + east) / 2)
        let span = MKCoordinateSpan(
            latitudeDelta: Swift.max((north - south) * 1.3, 0.002),
            longitudeDelta: Swift.max((east - west) * 1.3, 0.002)
        )
        return MKCoordinateRegion(center: center, span: span)
    }
}
