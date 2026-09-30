import SdrmmCore
import SwiftUI

struct HuntScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.verticalSizeClass) private var vertical
    @State private var tuning = false

    var body: some View {
        let hunt = model.hunt
        let controls = model.openMission?.controls ?? []
        layout(controls: controls)
            .sensoryFeedback(trigger: hunt.cue) { _, event in
                guard model.settings.hapticsOn, let event else {
                    return nil
                }
                return Self.feedback(event.cue)
            }
            .sheet(isPresented: $tuning) {
                TuneSheet(currentHz: hunt.view?.freqHz) { await hunt.tune(megahertz: $0) }
            }
    }

    @ViewBuilder private func layout(controls: [MissionControl]) -> some View {
        if vertical == .compact {
            HStack(alignment: .top, spacing: 24) {
                HuntReadout(tuning: $tuning, canTune: controls.contains(.tune))
                ScrollView {
                    HuntControls(tuning: $tuning, controls: controls)
                }
            }
            .padding()
        } else {
            ScrollView {
                VStack(spacing: 20) {
                    HuntReadout(tuning: $tuning, canTune: controls.contains(.tune))
                    HuntControls(tuning: $tuning, controls: controls)
                }
                .padding()
            }
        }
    }

    static func feedback(_ cue: HapticCue) -> SensoryFeedback {
        switch cue {
        case .increase: .increase
        case .decrease: .decrease
        case .success: .success
        }
    }
}

private struct HuntReadout: View {
    @Environment(AppModel.self) private var model
    @Binding var tuning: Bool
    let canTune: Bool

    var body: some View {
        let hunt = model.hunt
        let view = hunt.view
        VStack(spacing: 12) {
            Button {
                tuning = true
            } label: {
                Text(view.map { FrequencyText.text($0.freqHz) } ?? "-")
                    .font(.system(size: 40, weight: .semibold, design: .rounded).monospacedDigit())
                    .minimumScaleFactor(0.5)
                    .lineLimit(1)
            }
            .foregroundStyle(.primary)
            .disabled(!canTune)
            .accessibilityIdentifier(A11y.huntFreq)
            Text(LevelText.db(view?.smoothDb))
                .font(.title3.monospacedDigit())
                .foregroundStyle(.secondary)
                .accessibilityIdentifier(A11y.huntLevel)
            HStack(spacing: 8) {
                Image(systemName: hunt.trendSymbol)
                    .accessibilityHidden(true)
                Text(hunt.trendLabel)
                    .accessibilityIdentifier(A11y.huntTrend)
            }
            .font(.title2.weight(.semibold))
            HuntMeter(strength: view?.strength ?? 0, floor: view?.floorDb, best: view?.bestDb)
            if let refusal = view?.refusal {
                Text(refusal)
                    .foregroundStyle(Palette.danger)
                    .accessibilityIdentifier(A11y.huntRefusal)
            }
        }
        .frame(maxWidth: .infinity)
    }
}

private struct HuntMeter: View {
    let strength: Float
    let floor: Float?
    let best: Float?

    var body: some View {
        let fill = strength.isFinite ? Double(min(max(strength, 0), 1)) : 0
        VStack(spacing: 4) {
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 10).fill(.quaternary)
                    RoundedRectangle(cornerRadius: 10)
                        .fill(Palette.accent)
                        .frame(width: geometry.size.width * fill)
                }
            }
            .frame(height: 64)
            HStack {
                Text(LevelText.db(floor))
                Spacer()
                Text(LevelText.db(best))
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Strength")
        .accessibilityValue("\(Int((fill * 100).rounded())) %")
        .accessibilityIdentifier(A11y.huntMeter)
    }
}

private struct HuntControls: View {
    @Environment(AppModel.self) private var model
    @Binding var tuning: Bool
    let controls: [MissionControl]

    var body: some View {
        @Bindable var settings = model.settings
        let hunt = model.hunt
        let running = hunt.view?.running == true
        VStack(spacing: 16) {
            Button {
                Task { await hunt.toggleRun() }
            } label: {
                Text(running ? "Stop" : "Start")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .tint(running ? Palette.danger : Palette.accent)
            .disabled(hunt.busy || !controls.contains(.huntRun))
            .accessibilityIdentifier(A11y.huntRun)
            if controls.contains(.sweep) || controls.contains(.mark) {
                SweepPanel(controls: controls)
            }
            Toggle("Clicks", isOn: Binding(get: { settings.clicksOn }, set: { hunt.setClicks($0) }))
                .accessibilityIdentifier(A11y.huntClicks)
            Toggle("Haptics", isOn: $settings.hapticsOn)
                .accessibilityIdentifier(A11y.huntHaptics)
            if controls.contains(.tune) {
                Button("Tune") { tuning = true }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier(A11y.huntTune)
            }
        }
    }
}
