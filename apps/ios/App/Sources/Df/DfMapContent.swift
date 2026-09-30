import MapKit
import SdrmmCore
import SwiftUI

nonisolated struct IdentifiedRay: Identifiable, Equatable, Sendable {
    let id: Int
    let ray: Ray
}

nonisolated struct IdentifiedRing: Identifiable, Equatable, Sendable {
    let id: String
    let level: Float
    let points: [LatLon]
}

nonisolated enum DfMapStyle {
    static let paddingFraction = 0.15
    static let minimumSpanM = 500.0

    static func heatOpacity(level: Float) -> Double {
        switch level {
        case 0.95: 0.12
        case 0.8: 0.25
        case 0.5: 0.40
        default: 0.5 * (1 - Double(level)) + 0.1
        }
    }

    static func rayOpacity(weight: Float, floor: Double) -> Double {
        max(floor, Double(weight))
    }

    static func rays(_ overlay: DfOverlay) -> [IdentifiedRay] {
        overlay.rays.enumerated().map { IdentifiedRay(id: $0.offset, ray: $0.element) }
    }

    static func rings(_ heat: [HeatBand]) -> [IdentifiedRing] {
        let bands = heat.enumerated().sorted { $0.element.level > $1.element.level }
        return bands.flatMap { band in
            band.element.rings.enumerated().map { ring in
                IdentifiedRing(
                    id: "\(band.offset).\(ring.offset)",
                    level: band.element.level,
                    points: ring.element
                )
            }
        }
    }

    static func targetLabel(_ kind: GuidanceKind) -> String {
        switch kind {
        case .probe: "Cross"
        case .estimate: "Target"
        }
    }

    static func points(view: DfView?) -> [LatLon] {
        guard let view else {
            return []
        }
        let rays = view.overlay.rays.flatMap { [$0.from, $0.to] }
        let stations = view.overlay.stations.map(\.at)
        return rays + stations + [view.estimate?.at, view.target?.at].compactMap { $0 }
    }

    static func region(_ points: [LatLon]) -> MKMapRect? {
        guard let first = points.first else {
            return nil
        }
        let start = MKMapRect(origin: MKMapPoint(first.coordinate), size: MKMapSize(width: 0, height: 0))
        let rect = points.dropFirst().reduce(start) { rect, point in
            rect.union(MKMapRect(origin: MKMapPoint(point.coordinate), size: MKMapSize(width: 0, height: 0)))
        }
        let minimum = minimumSpanM * MKMapPointsPerMeterAtLatitude(first.lat)
        let width = max(rect.size.width, minimum)
        let height = max(rect.size.height, minimum)
        let grown = MKMapRect(
            x: rect.midX - width / 2,
            y: rect.midY - height / 2,
            width: width,
            height: height
        )
        return grown.insetBy(dx: -width * paddingFraction, dy: -height * paddingFraction)
    }
}

struct DfMapContent: MapContent {
    let overlay: DfOverlay
    let estimate: EstimateView?
    let target: NavPoint?
    let layers: MapLayers
    var lineWidth: CGFloat = 2
    var showsStations = true

    var body: some MapContent {
        UserAnnotation()
        if layers.heat {
            ForEach(DfMapStyle.rings(overlay.heat)) { ring in
                MapPolygon(coordinates: ring.points.map(\.coordinate))
                    .foregroundStyle(Palette.heat.opacity(DfMapStyle.heatOpacity(level: ring.level)))
            }
        }
        if layers.rays {
            ForEach(DfMapStyle.rays(overlay)) { item in
                MapPolyline(coordinates: [item.ray.from.coordinate, item.ray.to.coordinate])
                    .stroke(
                        Palette.accent.opacity(DfMapStyle.rayOpacity(weight: item.ray.weight, floor: 0.1)),
                        lineWidth: lineWidth
                    )
            }
        }
        if layers.ellipse, !overlay.ellipse.isEmpty {
            MapPolygon(coordinates: overlay.ellipse.map(\.coordinate))
                .foregroundStyle(Palette.accent.opacity(0.15))
                .stroke(Palette.accent, lineWidth: lineWidth)
        }
        if showsStations {
            ForEach(overlay.stations, id: \.id) { station in
                Annotation(station.id, coordinate: station.at.coordinate) {
                    Image(systemName: "antenna.radiowaves.left.and.right")
                        .foregroundStyle(.secondary)
                }
            }
        }
        if let estimate {
            Annotation("Estimate", coordinate: estimate.at.coordinate) {
                EstimateDot(converged: estimate.converged)
            }
        }
        if let target {
            Marker(
                DfMapStyle.targetLabel(target.kind),
                systemImage: "scope",
                coordinate: target.at.coordinate
            )
            .tint(Palette.warn)
        }
    }
}

private struct EstimateDot: View {
    let converged: Bool

    var body: some View {
        Group {
            if converged {
                Circle().fill(Palette.accent)
            } else {
                Circle().stroke(Palette.accent, lineWidth: 2)
            }
        }
        .frame(width: 10, height: 10)
    }
}
