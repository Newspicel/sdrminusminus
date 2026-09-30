import AVFAudio
import SdrmmCore
import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var forgetting: SavedServer?
    @State private var addServer = false

    var body: some View {
        @Bindable var settings = model.settings
        NavigationStack {
            Form {
                servers
                Section("Phone") {
                    TextField("Name", text: $settings.phoneName)
                        .accessibilityIdentifier(A11y.settingsName)
                }
                HeadingSettingsView()
                Section("Units") {
                    Picker("Units", selection: $settings.units) {
                        Text("Auto").tag(UnitsChoice.auto)
                        Text("Metric").tag(UnitsChoice.metric)
                        Text("Imperial").tag(UnitsChoice.imperial)
                    }
                    .accessibilityIdentifier(A11y.settingsUnits)
                }
                VoiceSettings()
                LocationSettings()
                about
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .confirmationDialog(
                "Forget server?",
                isPresented: forgetShown,
                titleVisibility: .visible,
                presenting: forgetting
            ) { server in
                Button("Forget", role: .destructive) { model.forget(server) }
                Button("Cancel", role: .cancel) { forgetting = nil }
            }
            .sheet(isPresented: $addServer) {
                PairView().environment(model)
            }
            .onChange(of: model.servers.map(\.id)) {
                addServer = false
            }
        }
    }

    private var servers: some View {
        Section("Servers") {
            ForEach(model.servers, id: \.id) { server in
                Button {
                    Task { await model.connect(serverID: server.id) }
                } label: {
                    HStack {
                        Text(server.name)
                        Spacer()
                        Text(online(server) ? "Online" : "Offline").foregroundStyle(.secondary)
                    }
                }
                .foregroundStyle(.primary)
                .swipeActions {
                    Button("Forget", role: .destructive) { forgetting = server }
                }
                .accessibilityIdentifier(A11y.server(server.id))
            }
            Button("Add server") { addServer = true }
                .accessibilityIdentifier(A11y.settingsAddServer)
        }
    }

    private var about: some View {
        Section("About") {
            LabeledContent("Version", value: appVersion)
            LabeledContent("Core", value: model.core.about().coreVersion)
            LabeledContent("Protocol", value: String(model.core.about().protocol))
            NavigationLink("Licenses") {
                LicensesView(entries: model.core.notices())
            }
        }
        .accessibilityIdentifier(A11y.settingsAbout)
    }

    private var forgetShown: Binding<Bool> {
        Binding(
            get: { forgetting != nil },
            set: { shown in
                if !shown { forgetting = nil }
            }
        )
    }

    private var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "-"
    }

    private func online(_ server: SavedServer) -> Bool {
        guard server.id == model.activeServer?.id, case .online = model.link else {
            return false
        }
        return true
    }
}

private struct VoiceSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var settings = model.settings
        Section("Voice") {
            Toggle("Voice", isOn: $settings.voiceOn)
                .accessibilityIdentifier(A11y.settingsVoice)
            Picker("Voice", selection: $settings.voiceID) {
                Text("Default").tag(String?.none)
                ForEach(Self.voices(), id: \.identifier) { voice in
                    Text(voice.name).tag(Optional(voice.identifier))
                }
            }
            .accessibilityIdentifier(A11y.settingsVoicePick)
            Button("Test") { model.speech.say("Voice test", urgent: true) }
                .accessibilityIdentifier(A11y.settingsVoiceTest)
        }
    }

    private static func voices() -> [AVSpeechSynthesisVoice] {
        let language = Locale.current.language.languageCode?.identifier ?? "en"
        return AVSpeechSynthesisVoice.speechVoices()
            .filter { $0.language.hasPrefix(language) }
            .sorted { $0.quality.rawValue > $1.quality.rawValue }
    }
}

private struct LocationSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let status = model.sensors.status
        Section("Location") {
            LabeledContent("Access", value: Self.access(status.access))
                .accessibilityIdentifier(A11y.settingsAccess)
            if status.access != .always {
                Button("Allow always") { model.sensors.requestAlways() }
                    .accessibilityIdentifier(A11y.settingsAlways)
            }
            HStack {
                LabeledContent("Precise", value: status.precise ? "On" : "Off")
                if !status.precise {
                    Button("Turn on") { model.sensors.requestPrecise() }
                }
            }
            .accessibilityIdentifier(A11y.settingsPrecise)
        }
    }

    private static func access(_ access: LocationAccess) -> String {
        switch access {
        case .always: "Always"
        case .whenInUse: "While using"
        case .denied, .restricted: "Off"
        case .unknown: "Not asked"
        }
    }
}
