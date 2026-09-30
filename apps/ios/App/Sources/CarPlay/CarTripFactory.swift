import CarPlay
import MapKit
import SdrmmCore

enum CarTripFactory {
    static func trip(origin: LatLon, target: NavPoint, plans: [RoutePlan], units: UnitSystem) -> CPTrip {
        let choices = plans.enumerated().map { index, plan in
            choice(plan, index: index, units: units)
        }
        let name = DfMapStyle.targetLabel(target.kind)
        let trip: CPTrip
        if #available(iOS 26.4, *) {
            trip = CPTrip(
                originWaypoint: CPNavigationWaypoint(
                    mapItem: item(origin, name: nil),
                    locationThreshold: nil,
                    entryPoints: []
                ),
                destinationWaypoint: CPNavigationWaypoint(
                    mapItem: item(target.at, name: name),
                    locationThreshold: nil,
                    entryPoints: []
                ),
                routeChoices: choices
            )
        } else {
            trip = CPTrip(
                origin: item(origin, name: nil),
                destination: item(target.at, name: name),
                routeChoices: choices
            )
        }
        trip.userInfo = plans.first?.id.uuidString
        return trip
    }

    static func summary(_ plan: RoutePlan, units: UnitSystem) -> String {
        "\(DurationText.text(plan.travelTimeS)) \u{00B7} \(DistanceText.short(plan.distanceM, units))"
    }

    static func planID(_ choice: CPRouteChoice) -> UUID? {
        (choice.userInfo as? String).flatMap(UUID.init(uuidString:))
    }

    private static func choice(_ plan: RoutePlan, index: Int, units: UnitSystem) -> CPRouteChoice {
        let name = plan.name.trimmingCharacters(in: .whitespaces).isEmpty ? "Route \(index + 1)" : plan.name
        let choice = CPRouteChoice(
            summaryVariants: [name],
            additionalInformationVariants: [summary(plan, units: units)],
            selectionSummaryVariants: [DurationText.text(plan.travelTimeS)]
        )
        choice.userInfo = plan.id.uuidString
        return choice
    }

    private static func item(_ point: LatLon, name: String?) -> MKMapItem {
        let item: MKMapItem
        if #available(iOS 26.0, *) {
            item = MKMapItem(location: CLLocation(latitude: point.lat, longitude: point.lon), address: nil)
        } else {
            item = MKMapItem(placemark: MKPlacemark(coordinate: point.coordinate))
        }
        item.name = name
        return item
    }
}
