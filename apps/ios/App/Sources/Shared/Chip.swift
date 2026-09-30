import SwiftUI

struct Chip: View {
    let text: String
    let color: Color

    var body: some View {
        Text(text)
            .font(.footnote.weight(.semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(Capsule().fill(color.opacity(0.15)))
    }
}
