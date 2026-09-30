import SwiftUI

enum Palette {
    static let accent = Color("AccentColor")
    static let warn = Color("Warn")
    static let danger = Color("Danger")
    static let heat = Color("Heat")
    static let survey: [Color] = [
        0x440154, 0x46327E, 0x365C8D, 0x277F8E, 0x1FA187, 0x4AC16D, 0xA0DA39, 0xFDE725,
    ].map { rgb in
        Color(
            red: Double((rgb >> 16) & 0xFF) / 255,
            green: Double((rgb >> 8) & 0xFF) / 255,
            blue: Double(rgb & 0xFF) / 255
        )
    }
}
