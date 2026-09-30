import MapKit
import SdrmmCore
import SwiftUI

enum DfA11y {
    static let bearing = "df.bearing"
    static let confidence = "df.confidence"
    static let state = "df.state"
    static let northUp = "df.northup"
    static let rose = "df.rose"
    static let guidance = "df.guidance"
    static let mode = "df.mode"
    static let navigate = "df.navigate"
    static let calibrate = "df.calibrate"
    static let clear = "df.clear"
    static let tune = "df.tune"
    static let layers = "df.layers"
    static let fit = "df.fit"
}

struct DfDriveView: View {
    @Environment(AppModel.self) private var app
    @State private var tuning = false
    @State private var notice = false

    var body: some View {
        @Bindable var model = app.df
        GeometryReader { geometry in
            if geometry.size.width > geometry.size.height {
                landscape(geometry.size)
            } else {
                portrait(geometry.size)
            }
        }
        .sheet(isPresented: $tuning) {
            TuneSheet(currentHz: model.view?.freqHz) { await model.tune(megahertz: $0) }
        }
        .sheet(isPresented: $notice) {
            RouteNoticeSheet {
                app.settings.routeNoticeAccepted = true
                notice = false
                model.navigate()
            }
        }
        .confirmationDialog("Clear fusion?", isPresented: $model.confirmClear, titleVisibility: .visible) {
            Button("Clear", role: .destructive) { Task { await model.clearFusion() } }
            Button("Cancel", role: .cancel) {}
        }
        .sensoryFeedback(.warning, trigger: model.retargetTick) { _, _ in app.settings.hapticsOn }
    }

    private func portrait(_ size: CGSize) -> some View {
        VStack(spacing: 8) {
            DfHeader()
            DfRose().frame(maxHeight: size.height * 0.45)
            DfGuidanceLine()
            DfActions(tune: { tuning = true }, navigate: navigate)
            DfMapPanel()
        }
        .padding(.top, 8)
    }

    private func landscape(_ size: CGSize) -> some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                VStack(spacing: 8) {
                    DfHeader()
                    DfRose()
                    DfGuidanceLine()
                }
                .frame(width: size.width * 0.38)
                DfMapPanel()
            }
            DfActions(tune: { tuning = true }, navigate: navigate)
        }
    }

    private func navigate() {
        if app.settings.routeNoticeAccepted {
            app.df.navigate()
        } else {
            notice = true
        }
    }
}

private struct DfHeader: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        let model = app.df
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(model.bearingText)
                .font(.system(size: 44, weight: .semibold, design: .rounded).monospacedDigit())
                .accessibilityIdentifier(DfA11y.bearing)
            Text(model.confidenceText)
                .font(.title3.monospacedDigit())
                .foregroundStyle(.secondary)
                .accessibilityIdentifier(DfA11y.confidence)
            Spacer(minLength: 0)
            VStack(alignment: .trailing, spacing: 4) {
                if let label = model.stateLabel {
                    DriveChip(text: label, tint: Palette.warn).accessibilityIdentifier(DfA11y.state)
                }
                if !model.rose.headingUp {
                    DriveChip(text: "North up", tint: .secondary).accessibilityIdentifier(DfA11y.northUp)
                }
                ForEach(DriveSensorChips.texts(app.sensors.status), id: \.self) { text in
                    DriveChip(text: text, tint: Palette.danger)
                }
            }
        }
        .padding(.horizontal)
    }
}

enum DriveSensorChips {
    static func texts(_ status: SensorStatus) -> [String] {
        var texts: [String] = []
        if status.access == .denied || status.access == .restricted {
            texts.append("Location off")
        } else if !status.precise {
            texts.append("Approx. location")
        }
        if status.running, !status.headingAvailable {
            texts.append("No compass")
        }
        return texts
    }
}

struct DriveChip: View {
    let text: String
    let tint: Color

    var body: some View {
        Text(text)
            .font(.caption.weight(.semibold))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(tint.opacity(0.15), in: Capsule())
            .foregroundStyle(tint)
    }
}

private struct DfRose: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        let model = app.df
        CompassRose(state: model.rose, trueBearingDeg: model.view?.bearingTrueDeg.map(Double.init))
            .padding(.horizontal)
            .accessibilityIdentifier(DfA11y.rose)
    }
}

private struct DfGuidanceLine: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Text(app.df.guidanceText)
            .font(.headline.monospacedDigit())
            .foregroundStyle(app.df.view?.guidance == nil ? Color.secondary : Palette.warn)
            .accessibilityIdentifier(DfA11y.guidance)
    }
}

private struct DfActions: View {
    @Environment(AppModel.self) private var app
    let tune: () -> Void
    let navigate: () -> Void

    var body: some View {
        let model = app.df
        let controls = app.openMission?.controls ?? []
        VStack(spacing: 8) {
            if controls.contains(.targetMode) {
                Picker("Target", selection: modeBinding) {
                    Text("Auto").tag(TargetMode.auto)
                    Text("Direct").tag(TargetMode.direct)
                }
                .pickerStyle(.segmented)
                .accessibilityIdentifier(DfA11y.mode)
            }
            HStack(spacing: 8) {
                Button("Navigate", systemImage: "car.fill", action: navigate)
                    .buttonStyle(.borderedProminent)
                    .disabled(!model.canNavigate)
                    .accessibilityIdentifier(DfA11y.navigate)
                if controls.contains(.calibrate) {
                    Button("Calibrate") { Task { await model.calibrate() } }
                        .accessibilityIdentifier(DfA11y.calibrate)
                }
                if controls.contains(.clearFusion) {
                    Button("Clear") { model.askClear() }
                        .accessibilityIdentifier(DfA11y.clear)
                }
                if controls.contains(.tune) {
                    Button("Tune", action: tune)
                        .accessibilityIdentifier(DfA11y.tune)
                }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(model.busy)
        }
        .padding(.horizontal)
    }

    private var modeBinding: Binding<TargetMode> {
        let model = app.df
        return Binding(
            get: { model.view?.targetMode ?? .auto },
            set: { mode in Task { await model.setTargetMode(mode) } }
        )
    }
}

private struct DfMapPanel: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        @Bindable var model = app.df
        Map(position: $model.camera) {
            DfMapContent(
                overlay: model.overlay,
                estimate: model.view?.estimate,
                target: model.view?.target,
                layers: visibleLayers
            )
        }
        .mapStyle(.standard(pointsOfInterest: .excludingAll))
        .mapControls {
            MapUserLocationButton()
            MapCompass()
            MapScaleView()
        }
        .overlay(alignment: .topLeading) {
            VStack(spacing: 8) {
                layersMenu
                Button("Fit", systemImage: "arrow.up.left.and.arrow.down.right") { model.fitAll() }
                    .labelStyle(.iconOnly)
                    .accessibilityIdentifier(DfA11y.fit)
            }
            .buttonStyle(.bordered)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
            .padding(8)
        }
    }

    private var visibleLayers: MapLayers {
        let model = app.df
        var layers = model.layers
        layers.heat = layers.heat && model.heatAvailable
        return layers
    }

    private var layersMenu: some View {
        @Bindable var model = app.df
        return Menu {
            Toggle("Rays", isOn: $model.layers.rays)
            Toggle("Heat", isOn: $model.layers.heat).disabled(!model.heatAvailable)
            Toggle("Ellipse", isOn: $model.layers.ellipse)
        } label: {
            Label("Layers", systemImage: "square.3.layers.3d").labelStyle(.iconOnly)
        }
        .accessibilityIdentifier(DfA11y.layers)
    }
}
