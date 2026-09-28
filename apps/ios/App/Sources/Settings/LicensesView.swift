import SdrmmCore
import SwiftUI

struct LicensesView: View {
    let entries: [LicenseEntry]

    var body: some View {
        List(entries, id: \.self) { entry in
            NavigationLink {
                ScrollView {
                    Text(entry.text)
                        .font(.footnote.monospaced())
                        .textSelection(.enabled)
                        .padding()
                }
                .navigationTitle(entry.name)
            } label: {
                VStack(alignment: .leading) {
                    Text(entry.version.map { "\(entry.name) \($0)" } ?? entry.name)
                    Text(entry.license).font(.footnote).foregroundStyle(.secondary)
                }
            }
        }
        .navigationTitle("Licenses")
    }
}
