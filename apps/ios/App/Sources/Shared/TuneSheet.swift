import SwiftUI

struct TuneSheet: View {
    let currentHz: Double?
    let apply: @MainActor (String) async -> String?
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var error: String?
    @State private var busy = false

    var body: some View {
        NavigationStack {
            Form {
                TextField("MHz", text: $text)
                    .keyboardType(.decimalPad)
                    .font(.title2.monospacedDigit())
                    .accessibilityIdentifier(A11y.tuneField)
                if let error {
                    Text(error)
                        .foregroundStyle(Palette.danger)
                        .accessibilityIdentifier(A11y.tuneError)
                }
            }
            .navigationTitle("Tune")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Set") { Task { await submit() } }
                        .disabled(busy)
                        .accessibilityIdentifier(A11y.tuneSet)
                }
            }
            .onAppear {
                if let currentHz {
                    text = String(format: "%.3f", currentHz / 1e6)
                }
            }
        }
        .presentationDetents([.medium])
    }

    private func submit() async {
        busy = true
        error = await apply(text)
        busy = false
        if error == nil {
            dismiss()
        }
    }
}

nonisolated enum TuneInput {
    static func hertz(_ megahertz: String) -> Double? {
        let normalized = megahertz.trimmingCharacters(in: .whitespaces).replacingOccurrences(
            of: ",",
            with: "."
        )
        guard let value = Double(normalized) else {
            return nil
        }
        let hz = value * 1e6
        guard hz.isFinite, hz > 0, hz < 1e11 else {
            return nil
        }
        return hz
    }
}
