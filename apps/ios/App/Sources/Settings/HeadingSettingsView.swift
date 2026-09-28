import SdrmmCore
import SwiftUI

struct HeadingSettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var failure: String?

    var body: some View {
        @Bindable var settings = model.settings
        Section("Heading") {
            LabeledContent("Heading", value: Self.headingStatus(model.pose))
                .accessibilityIdentifier(A11y.settingsHeading)
            Picker("Source", selection: $settings.headingMode) {
                Text("Auto").tag(HeadingMode.auto)
                Text("Compass").tag(HeadingMode.compass)
                Text("GPS course").tag(HeadingMode.course)
            }
            .accessibilityIdentifier(A11y.settingsSource)
            Picker("Mount", selection: $settings.mount) {
                Text("Flat").tag(Mount.flat)
                Text("Upright").tag(Mount.upright)
            }
            .accessibilityIdentifier(A11y.settingsMount)
            Stepper(value: $settings.mountOffsetDeg, in: -180...179, step: 1) {
                LabeledContent("Offset", value: Self.offset(settings.mountOffsetDeg))
            }
            .accessibilityIdentifier(A11y.settingsOffset)
            Button("Align with car") { model.startAlign() }
                .accessibilityHint("Drive straight above 20 km/h for about 20 s")
                .accessibilityIdentifier(A11y.settingsAlign)
            alignStatus
        }
        .sheet(item: failureBinding) { reason in
            NavigationStack {
                Text(reason.text)
                    .padding()
                    .navigationTitle("Failed")
                    .navigationBarTitleDisplayMode(.inline)
            }
            .presentationDetents([.medium])
        }
    }

    @ViewBuilder private var alignStatus: some View {
        switch model.pose?.align ?? .idle {
        case .idle:
            EmptyView()
        case .collecting(let progress, let hint):
            VStack(alignment: .leading) {
                ProgressView(value: Double(progress))
                HStack {
                    Text(Self.hint(hint))
                    Spacer()
                    Button("Cancel") { model.cancelAlign() }
                        .buttonStyle(.borderless)
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier(A11y.settingsAlignStatus)
        case .done(let offset):
            Text("Aligned \(Self.offset(offset))")
                .accessibilityIdentifier(A11y.settingsAlignStatus)
        case .failed(let reason):
            Button("Failed") { failure = reason }
                .foregroundStyle(Palette.danger)
                .accessibilityIdentifier(A11y.settingsAlignStatus)
        }
    }

    private var failureBinding: Binding<FailureText?> {
        Binding(
            get: { failure.map(FailureText.init) },
            set: { failure = $0?.text }
        )
    }

    static func hint(_ hint: AlignHint) -> String {
        switch hint {
        case .driveFaster: "Drive faster"
        case .driveStraight: "Drive straight"
        case .hold: "Hold"
        }
    }

    static func offset(_ degrees: Double) -> String {
        "\(Int(degrees.rounded()))\u{00B0}"
    }

    static func headingStatus(_ pose: PoseView?) -> String {
        guard let pose else {
            return "None"
        }
        let accuracy = pose.accuracyDeg.map { " \u{00B1}\(Int($0.rounded()))\u{00B0}" } ?? ""
        switch pose.source {
        case .fused: return "Fused" + accuracy
        case .compass: return "Compass" + accuracy
        case .course: return "GPS course"
        case .none: return "None"
        }
    }
}

private struct FailureText: Identifiable {
    let text: String
    var id: String { text }
}
