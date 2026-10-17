/// Desktop palette and paper surfaces adapted to a touch-first reading layout.
import SwiftUI

enum Theme {
    static let ground = Color(hex: MermozPalette.ground)
    static let text = Color(hex: MermozPalette.text)
    static let text2 = Color(hex: MermozPalette.text2)
    static let text3 = Color(hex: MermozPalette.text3)
    static let accent = Color(hex: MermozPalette.accent)
    static let accentSoft = Color(hex: MermozPalette.accentSoft)
    static let line = Color(hex: MermozPalette.line)
    static let paper = Color.white.opacity(0.92)
}

extension Color {
    init(hex: String) {
        let value = UInt64(hex.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16) ?? 0x888888
        self.init(.sRGB, red: Double((value >> 16) & 255) / 255,
                  green: Double((value >> 8) & 255) / 255, blue: Double(value & 255) / 255, opacity: 1)
    }
}

extension View {
    func paperPanel() -> some View {
        background(Theme.paper, in: RoundedRectangle(cornerRadius: 16))
            .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(Theme.line))
    }

    func mermozSurface() -> some View {
        scrollContentBackground(.hidden).background(Theme.ground)
            .foregroundStyle(Theme.text).tint(Theme.accent)
    }
}
