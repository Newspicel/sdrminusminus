import SwiftUI

enum NavA11y {
    static let banner = "nav.banner"
    static let routing = "nav.routing"
    static let rerouting = "nav.rerouting"
    static let noRoute = "nav.noroute"
    static let throttled = "nav.throttled"
    static let arrived = "nav.arrived"
    static let summary = "nav.summary"
    static let end = "nav.end"
    static let rose = "nav.rose"
    static let go = "nav.go"
    static let noticeOK = "nav.notice.ok"
}

struct ManeuverBanner: View {
    let state: ManeuverBannerState

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: state.symbol)
                .font(.system(size: 40, weight: .bold))
                .frame(width: 52)
            VStack(alignment: .leading, spacing: 2) {
                Text(state.distance)
                    .font(.title.bold().monospacedDigit())
                Text(state.instruction)
                    .font(.headline)
                    .lineLimit(2)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .foregroundStyle(.white)
        .background(Palette.accent, in: RoundedRectangle(cornerRadius: 14))
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(NavA11y.banner)
    }
}

struct NavStatusBar: View {
    let text: String
    var detail: String?
    let identifier: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(text).font(.headline)
            if let detail {
                Text(detail).font(.subheadline.monospacedDigit())
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
    }
}

enum RouteNotice {
    static let text =
        "YOUR USE OF THIS REAL TIME ROUTE GUIDANCE APPLICATION IS AT YOUR SOLE RISK. LOCATION DATA MAY NOT BE ACCURATE."
}

struct RouteNoticeSheet: View {
    let accept: () -> Void

    var body: some View {
        VStack(spacing: 20) {
            Text(RouteNotice.text)
                .font(.footnote.weight(.semibold))
                .multilineTextAlignment(.center)
            Button("OK", action: accept)
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier(NavA11y.noticeOK)
        }
        .padding(24)
        .presentationDetents([.height(220)])
        .interactiveDismissDisabled()
    }
}
