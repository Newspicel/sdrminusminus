import MapKit
import SdrmmCore
import UIKit

nonisolated final class RayLine: MKPolyline {
    var weight: Float = 1

    static func make(_ points: [LatLon]) -> RayLine {
        let coordinates = points.map(\.coordinate)
        return RayLine(coordinates: coordinates, count: coordinates.count)
    }
}

nonisolated final class RouteLine: MKPolyline {
    static func make(_ points: [LatLon]) -> RouteLine {
        let coordinates = points.map(\.coordinate)
        return RouteLine(coordinates: coordinates, count: coordinates.count)
    }
}

nonisolated final class RouteCasing: MKPolyline {
    static func make(_ points: [LatLon]) -> RouteCasing {
        let coordinates = points.map(\.coordinate)
        return RouteCasing(coordinates: coordinates, count: coordinates.count)
    }
}

nonisolated final class BearingRay: MKPolyline {
    static func make(_ points: [LatLon]) -> BearingRay {
        let coordinates = points.map(\.coordinate)
        return BearingRay(coordinates: coordinates, count: coordinates.count)
    }
}

nonisolated final class EllipseShape: MKPolygon {
    static func make(_ points: [LatLon]) -> EllipseShape {
        let coordinates = points.map(\.coordinate)
        return EllipseShape(coordinates: coordinates, count: coordinates.count)
    }
}

nonisolated final class HeatShape: MKPolygon {
    var level: Float = 0.5

    static func make(_ points: [LatLon]) -> HeatShape {
        let coordinates = points.map(\.coordinate)
        return HeatShape(coordinates: coordinates, count: coordinates.count)
    }
}

nonisolated final class TargetMark: MKPointAnnotation {}

nonisolated final class StationMark: MKPointAnnotation {}

nonisolated final class EstimateMark: MKPointAnnotation {}

enum CarZoom {
    static let minimumM = 300.0
    static let maximumM = 200_000.0

    static func clamp(_ distance: Double) -> Double {
        min(maximumM, max(minimumM, distance))
    }
}

final class CarMapViewController: UIViewController, MKMapViewDelegate {
    private let map = MKMapView()
    private var scene: CarMapScene?
    private var bearing: CarBearingRay?
    private var groups: [String: [any MKOverlay]] = [:]
    private var marks: [any MKAnnotation] = []
    private var gestureStartM: Double?
    var navigating = false

    override func loadView() {
        map.delegate = self
        map.pointOfInterestFilter = .excludingAll
        map.showsUserLocation = true
        map.showsCompass = false
        map.userTrackingMode = .follow
        view = map
    }

    func render(_ next: CarMapScene) {
        guard next != scene else {
            return
        }
        let previous = scene ?? .empty
        scene = next
        if previous.heat != next.heat || previous.rays != next.rays || previous.ellipse != next.ellipse
            || previous.route != next.route
        {
            replaceOverlays(next, previous: previous)
        }
        if previous.stations != next.stations || previous.estimate != next.estimate
            || previous.target != next.target
        {
            replaceMarks(next)
        }
        applyFollow(next.follow)
    }

    func recentre() {
        map.setUserTrackingMode(navigating ? .followWithHeading : .follow, animated: true)
    }

    func zoom(by factor: Double) {
        let camera = map.camera
        camera.centerCoordinateDistance = CarZoom.clamp(camera.centerCoordinateDistance * factor)
        if !navigating {
            map.userTrackingMode = .none
        }
        map.setCamera(camera, animated: true)
    }

    func beginZoomGesture() {
        gestureStartM = map.camera.centerCoordinateDistance
    }

    func updateZoomGesture(scale: Double) {
        guard let start = gestureStartM, scale > 0 else {
            return
        }
        let camera = map.camera
        camera.centerCoordinateDistance = CarZoom.clamp(start / scale)
        map.setCamera(camera, animated: false)
    }

    func endZoomGesture() {
        gestureStartM = nil
    }

    func showBearingRay(_ ray: CarBearingRay?) {
        guard ray != bearing else {
            return
        }
        bearing = ray
        replace("bearing", with: ray.map { [BearingRay.make([$0.from, $0.end])] } ?? [], level: .aboveLabels)
    }

    func styleChanged(_ style: UIUserInterfaceStyle) {
        overrideUserInterfaceStyle = style
        map.overrideUserInterfaceStyle = style
    }

    private func applyFollow(_ follow: CarFollow) {
        let mode: MKUserTrackingMode
        switch follow {
        case .free: return
        case .user: mode = .follow
        case .userHeading: mode = .followWithHeading
        }
        if map.userTrackingMode != mode {
            map.setUserTrackingMode(mode, animated: true)
        }
    }

    private func replaceOverlays(_ next: CarMapScene, previous: CarMapScene) {
        if previous.heat != next.heat || groups["heat"] == nil {
            replace("heat", with: heatShapes(next.heat), level: .aboveRoads)
        }
        if previous.ellipse != next.ellipse || groups["ellipse"] == nil {
            let shape = next.ellipse.isEmpty ? [] : [EllipseShape.make(next.ellipse)]
            replace("ellipse", with: shape, level: .aboveRoads)
        }
        if previous.route != next.route || groups["route"] == nil {
            let route: [any MKOverlay] =
                next.route.count < 2
                ? [] : [RouteCasing.make(next.route), RouteLine.make(next.route)]
            replace("route", with: route, level: .aboveRoads)
        }
        if previous.rays != next.rays || groups["rays"] == nil {
            replace("rays", with: rayLines(next.rays), level: .aboveLabels)
        }
    }

    private func replace(_ group: String, with overlays: [any MKOverlay], level: MKOverlayLevel) {
        if let old = groups[group] {
            map.removeOverlays(old)
        }
        groups[group] = overlays
        map.addOverlays(overlays, level: level)
    }

    private func heatShapes(_ heat: [HeatBand]) -> [any MKOverlay] {
        heat.sorted { $0.level > $1.level }.flatMap { band in
            band.rings.filter { $0.count >= 3 }.map { ring in
                let shape = HeatShape.make(ring)
                shape.level = band.level
                return shape
            }
        }
    }

    private func rayLines(_ rays: [Ray]) -> [any MKOverlay] {
        rays.map { ray in
            let line = RayLine.make([ray.from, ray.to])
            line.weight = ray.weight
            return line
        }
    }

    private func replaceMarks(_ next: CarMapScene) {
        map.removeAnnotations(marks)
        var added: [any MKAnnotation] = next.stations.map { station in
            let mark = StationMark()
            mark.coordinate = station.at.coordinate
            mark.title = station.id
            return mark
        }
        if let estimate = next.estimate {
            let mark = EstimateMark()
            mark.coordinate = estimate.coordinate
            added.append(mark)
        }
        if let target = next.target {
            let mark = TargetMark()
            mark.coordinate = target.at.coordinate
            mark.title = DfMapStyle.targetLabel(target.kind)
            added.append(mark)
        }
        marks = added
        map.addAnnotations(added)
    }

    func mapView(_ mapView: MKMapView, rendererFor overlay: any MKOverlay) -> MKOverlayRenderer {
        CarMapStyle.renderer(for: overlay)
    }

    func mapView(_ mapView: MKMapView, viewFor annotation: any MKAnnotation) -> MKAnnotationView? {
        CarMapStyle.view(for: annotation, in: mapView)
    }
}

enum CarMapStyle {
    static let accent = UIColor(named: "AccentColor") ?? .systemTeal
    static let warn = UIColor(named: "Warn") ?? .systemYellow
    static let heat = UIColor(named: "Heat") ?? .systemOrange

    static func renderer(for overlay: any MKOverlay) -> MKOverlayRenderer {
        switch overlay {
        case let ray as RayLine:
            return stroke(
                ray,
                accent.withAlphaComponent(DfMapStyle.rayOpacity(weight: ray.weight, floor: 0.15)),
                3
            )
        case let casing as RouteCasing:
            return stroke(casing, UIColor.black.withAlphaComponent(0.6), 10)
        case let route as RouteLine:
            return stroke(route, .systemBlue, 8)
        case let bearing as BearingRay:
            return stroke(bearing, accent, 4)
        case let ellipse as EllipseShape:
            let renderer = MKPolygonRenderer(polygon: ellipse)
            renderer.fillColor = accent.withAlphaComponent(0.15)
            renderer.strokeColor = accent
            renderer.lineWidth = 2
            return renderer
        case let shape as HeatShape:
            let renderer = MKPolygonRenderer(polygon: shape)
            renderer.fillColor = heat.withAlphaComponent(DfMapStyle.heatOpacity(level: shape.level))
            return renderer
        default:
            return MKOverlayRenderer(overlay: overlay)
        }
    }

    static func view(for annotation: any MKAnnotation, in map: MKMapView) -> MKAnnotationView? {
        switch annotation {
        case is TargetMark:
            let view = MKMarkerAnnotationView(annotation: annotation, reuseIdentifier: "target")
            view.markerTintColor = warn
            view.glyphImage = UIImage(systemName: "scope")
            view.displayPriority = .required
            return view
        case is StationMark:
            let view = MKMarkerAnnotationView(annotation: annotation, reuseIdentifier: "station")
            view.markerTintColor = .secondaryLabel
            view.glyphImage = UIImage(systemName: "antenna.radiowaves.left.and.right")
            return view
        case is EstimateMark:
            let view = MKAnnotationView(annotation: annotation, reuseIdentifier: "estimate")
            view.frame = CGRect(x: 0, y: 0, width: 10, height: 10)
            view.backgroundColor = accent
            view.layer.cornerRadius = 5
            return view
        default:
            return nil
        }
    }

    private static func stroke(_ line: MKPolyline, _ color: UIColor, _ width: CGFloat) -> MKPolylineRenderer {
        let renderer = MKPolylineRenderer(polyline: line)
        renderer.strokeColor = color
        renderer.lineWidth = width
        renderer.lineCap = .round
        return renderer
    }
}
