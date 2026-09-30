import MapKit
import SdrmmCore
import SwiftUI

struct NavigationScreen: View {
    @Environment(AppModel.self) private var app
    @State private var camera: MapCameraPosition = .userLocation(followsHeading: true, fallback: .automatic)

    var body: some View {
        let navigation = app.navigation
        Map(position: $camera) {
            if let plan = navigation.activePlan {
                MapPolyline(coordinates: plan.points.map(\.coordinate))
                    .stroke(
                        Palette.accent,
                        style: StrokeStyle(lineWidth: 6, lineCap: .round, lineJoin: .round)
                    )
            }
            DfMapContent(
                overlay: app.df.overlay,
                estimate: nil,
                target: navigation.target ?? app.df.view?.target,
                layers: MapLayers(rays: true, heat: false, ellipse: true),
                lineWidth: 1,
                showsStations: false
            )
        }
        .mapStyle(.standard(pointsOfInterest: .excludingAll))
        .mapControls {
            MapUserLocationButton()
            MapCompass()
        }
        .safeAreaInset(edge: .top) {
            HStack(alignment: .top, spacing: 8) {
                NavTopCard()
                CompassRose(state: app.df.rose)
                    .frame(width: 64, height: 64)
                    .background(.regularMaterial, in: Circle())
                    .accessibilityIdentifier(NavA11y.rose)
            }
            .padding(.horizontal)
        }
        .safeAreaInset(edge: .bottom) {
            NavBottomBar(end: end)
        }
        .toolbar(.hidden, for: .navigationBar)
        .onChange(of: navigation.isNavigating) { _, navigating in
            if !navigating {
                app.path.removeAll { $0 == .navigation }
            }
        }
        .sensoryFeedback(.impact(weight: .medium), trigger: navigation.nearTick) { _, _ in
            app.settings.hapticsOn
        }
    }

    private func end() {
        app.navigation.end()
        app.path.removeAll { $0 == .navigation }
    }
}

private struct NavTopCard: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        let navigation = app.navigation
        switch navigation.phase {
        case .idle:
            NavStatusBar(text: "No route", identifier: NavA11y.noRoute)
        case .routing:
            NavStatusBar(text: "Routing", identifier: NavA11y.routing)
        case .active:
            activeCard(navigation)
        case .noRoute(_, let reason):
            noRouteCard(navigation, reason: reason)
        case .arrived:
            NavStatusBar(text: PromptText.arrived, identifier: NavA11y.arrived)
        }
    }

    @ViewBuilder private func activeCard(_ navigation: NavigationModel) -> some View {
        if navigation.isRerouting, let retry = navigation.retryAt {
            PausedCard(retryAt: retry)
        } else if navigation.isRerouting {
            NavStatusBar(text: PromptText.rerouting, identifier: NavA11y.rerouting)
        } else if let banner = navigation.banner {
            ManeuverBanner(state: banner)
        }
    }

    @ViewBuilder private func noRouteCard(_ navigation: NavigationModel, reason: NoRouteReason) -> some View {
        if case .throttled(let retry) = reason {
            PausedCard(retryAt: retry)
        } else {
            NavStatusBar(text: "No route", detail: navigation.directGuidance, identifier: NavA11y.noRoute)
        }
    }
}

private struct PausedCard: View {
    let retryAt: Date

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let seconds = max(0, Int(retryAt.timeIntervalSince(context.date).rounded(.up)))
            NavStatusBar(text: "Routing paused", detail: "\(seconds) s", identifier: NavA11y.throttled)
        }
    }
}

private struct NavBottomBar: View {
    @Environment(AppModel.self) private var app
    let end: () -> Void

    var body: some View {
        let navigation = app.navigation
        VStack(spacing: 8) {
            if case .arrived = navigation.phase, let pending = navigation.pendingTarget {
                HStack {
                    Text(pendingText(pending, navigation))
                        .font(.headline)
                    Spacer()
                    Button("Go") { navigation.goToPending() }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier(NavA11y.go)
                }
            }
            HStack {
                if let summary = navigation.summary {
                    Text("\(summary.duration) \u{00B7} \(summary.remaining) \u{00B7} \(summary.arrival)")
                        .font(.headline.monospacedDigit())
                        .accessibilityIdentifier(NavA11y.summary)
                }
                Spacer()
                Button("End", role: .destructive, action: end)
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier(NavA11y.end)
            }
        }
        .padding()
        .background(.regularMaterial)
    }

    private func pendingText(_ target: NavPoint, _ navigation: NavigationModel) -> String {
        guard let distance = navigation.distance(to: target) else {
            return "New target"
        }
        return "New target \(DistanceText.short(distance, navigation.units))"
    }
}
