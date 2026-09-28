import SdrmmCore
import SwiftUI

struct BannerView: View {
    let banner: Banner
    let onTap: () -> Void

    var body: some View {
        Button(action: onTap) {
            Label(banner.text, systemImage: symbol)
                .font(.callout.weight(.medium))
                .foregroundStyle(tint)
                .lineLimit(1)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
        .accessibilityHint("Shows details")
        .accessibilityIdentifier(A11y.banner)
    }

    private var symbol: String {
        switch banner.level {
        case .info: "info.circle"
        case .warn: "exclamationmark.triangle"
        case .error: "xmark.octagon"
        }
    }

    private var tint: Color {
        switch banner.level {
        case .info: .primary
        case .warn: Palette.warn
        case .error: Palette.danger
        }
    }
}
