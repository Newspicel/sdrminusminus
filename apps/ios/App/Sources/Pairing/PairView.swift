import SdrmmCore
import SwiftUI
import UIKit

struct PairView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var pairing = model.pairing
        NavigationStack {
            Form {
                scanSection
                nearbySection
                manualSection
                if let error = pairing.error {
                    Section {
                        Text(error)
                            .foregroundStyle(Palette.danger)
                            .accessibilityIdentifier(A11y.pairError)
                    }
                }
                demoSection
            }
            .navigationTitle("Pair")
            .overlay { progress }
        }
        .onAppear { pairing.appear() }
        .onDisappear { pairing.disappear() }
        .sheet(isPresented: codeSheetShown) { CodeSheet() }
        .sheet(isPresented: trustShown) { TrustSheet() }
        .fullScreenCover(isPresented: scannerShown) { scanner }
    }

    private var scanSection: some View {
        Section("Scan") {
            switch model.pairing.scanner {
            case .ready:
                Button("Scan QR") { model.pairing.scan() }
                    .accessibilityIdentifier(A11y.pairScan)
            case .cameraOff:
                HStack {
                    Text("Camera off")
                    Spacer()
                    Button("Settings") { openSettings() }
                }
                .accessibilityIdentifier(A11y.pairScanHint)
            case .unsupported:
                Text("Use the Camera app")
                    .foregroundStyle(.secondary)
                    .accessibilityHint("The camera opens sdrmm pairing links")
                    .accessibilityIdentifier(A11y.pairScanHint)
            }
        }
    }

    private var nearbySection: some View {
        Section("Nearby") {
            if let browseError = model.pairing.browseError {
                HStack {
                    Text(browseError)
                    Spacer()
                    Button("Settings") { openSettings() }
                }
                .accessibilityIdentifier(A11y.pairNearbyError)
            } else if model.pairing.nearby.isEmpty {
                Text("None found")
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier(A11y.pairNearbyEmpty)
            }
            ForEach(model.pairing.nearby, id: \.name) { server in
                Button {
                    model.pairing.choose(server)
                } label: {
                    HStack {
                        Text(server.name)
                        Spacer()
                        Text("Pair").foregroundStyle(Palette.accent)
                    }
                }
                .accessibilityIdentifier(A11y.pairNearby(server.name))
            }
        }
    }

    private var manualSection: some View {
        @Bindable var pairing = model.pairing
        return Section("Manual") {
            TextField("host:port", text: $pairing.address)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.URL)
                .accessibilityIdentifier(A11y.pairAddress)
            TextField("Code", text: $pairing.code)
                .keyboardType(.numberPad)
                .font(.body.monospacedDigit())
                .accessibilityHint("8 digits from the server")
                .accessibilityIdentifier(A11y.pairCode)
            Button("Pair") { Task { await pairing.submitManual() } }
                .accessibilityIdentifier(A11y.pairSubmit)
        }
    }

    private var demoSection: some View {
        Section("Demo") {
            Button("Try without a server") { AppRuntime.demo.start() }
                .accessibilityIdentifier(A11y.pairDemo)
        }
    }

    @ViewBuilder private var progress: some View {
        if model.pairing.step == .pairing {
            ProgressView("Pairing")
                .padding()
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                .accessibilityIdentifier(A11y.pairProgress)
        }
    }

    private var scanner: some View {
        NavigationStack {
            QRScannerView(
                onPayload: { model.pairing.scanned($0) },
                onFailure: { model.pairing.scanFailed($0) }
            )
            .ignoresSafeArea()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { model.pairing.cancel() }
                }
            }
        }
    }

    private var codeSheetShown: Binding<Bool> {
        Binding(
            get: {
                if case .code = model.pairing.step { return true }
                return false
            },
            set: { shown in
                if !shown, case .code = model.pairing.step { model.pairing.cancel() }
            }
        )
    }

    private var trustShown: Binding<Bool> {
        Binding(
            get: {
                if case .confirm = model.pairing.step { return true }
                return false
            },
            set: { shown in
                if !shown, case .confirm = model.pairing.step { model.pairing.cancel() }
            }
        )
    }

    private var scannerShown: Binding<Bool> {
        Binding(
            get: { model.pairing.step == .scanning },
            set: { shown in
                if !shown, model.pairing.step == .scanning { model.pairing.cancel() }
            }
        )
    }

    private func openSettings() {
        if let url = URL(string: UIApplication.openSettingsURLString) {
            UIApplication.shared.open(url)
        }
    }
}

private struct CodeSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var pairing = model.pairing
        NavigationStack {
            Form {
                TextField("Code", text: $pairing.code)
                    .keyboardType(.numberPad)
                    .font(.title2.monospacedDigit())
                    .accessibilityIdentifier(A11y.pairCodeField)
                if let error = pairing.error {
                    Text(error)
                        .foregroundStyle(Palette.danger)
                        .accessibilityIdentifier(A11y.pairError)
                }
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { pairing.cancel() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Pair") { pairing.submitCode() }
                        .accessibilityIdentifier(A11y.pairCodeSubmit)
                }
            }
        }
        .presentationDetents([.medium])
    }

    private var title: String {
        if case .code(let server) = model.pairing.step {
            return server.name
        }
        return "Pair"
    }
}

private struct TrustSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Key \(fingerprint)")
                        .font(.body.monospaced())
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityHint("Compare with the key check on the server")
                    if let error = model.pairing.error {
                        Text(error)
                            .foregroundStyle(Palette.danger)
                            .accessibilityIdentifier(A11y.pairError)
                    }
                }
            }
            .navigationTitle("Trust server?")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { model.pairing.cancel() }
                        .accessibilityIdentifier(A11y.pairCancel)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Trust") { Task { await model.pairing.trust() } }
                        .accessibilityIdentifier(A11y.pairTrust)
                }
            }
        }
        .presentationDetents([.medium])
        .interactiveDismissDisabled()
    }

    private var fingerprint: String {
        if case .confirm(let offer) = model.pairing.step {
            return offer.fingerprintShort ?? "-"
        }
        return "-"
    }
}
