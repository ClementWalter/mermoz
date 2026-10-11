/// Mermoz visual language: very light blue ground, Liquid Glass surfaces, Linear-style status marks.
import SwiftUI
import MermozKit

enum Theme {
    static let ground = Color(hex: "#EAF3FB")
    static let text = Color(hex: "#0D2436")
    static let text2 = Color(hex: "#4B6477")
    static let text3 = Color(hex: "#5A7386")
    static let accent = Color(hex: "#1F6FB5")
    static let accentSoft = Color(hex: "#DCEBFA")
    static let line = Color(hex: "#DDE9F3")
    static let paper = Color.white.opacity(0.86)
    static let danger = Color(hex: "#C93A3A")
    static let ok = Color(hex: "#2E9A6B")

    /// Fallback colors when the daemon sends none, keyed by default status key.
    static let statusColors: [String: String] = [
        "triage": "#8A6FD1", "backlog": "#8AA0B2", "todo": "#5A7386", "in_progress": "#D9902F",
        "waiting": "#2F8FCF", "review": "#C2568C", "done": "#2E9A6B", "canceled": "#8AA0B2",
    ]

    static let accountPalette = ["#1F6FB5", "#8A6FD1", "#2E9A6B", "#E0782F", "#C2568C", "#2F8FCF"]
    static let projectColors = ["#5B8DEF", "#2F8FCF", "#2E9A6B", "#E0782F", "#D9534F", "#C2568C", "#8A6FD1", "#5A7386"]

    static func colorName(_ hex: String) -> String {
        ["#5B8DEF": "Blue", "#2F8FCF": "Sky", "#2E9A6B": "Green", "#E0782F": "Orange", "#D9534F": "Red",
         "#C2568C": "Pink", "#8A6FD1": "Purple", "#5A7386": "Slate"][hex] ?? hex
    }

    static func accountColor(_ id: String, among accounts: [Account]) -> Color {
        let i = accounts.firstIndex { $0.id == id } ?? 0
        return Color(hex: accountPalette[i % accountPalette.count])
    }

    static let priorityNames = [0: "No priority", 1: "Urgent", 2: "High", 3: "Normal", 4: "Low"]
}

extension Color {
    init(hex: String) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        let v = UInt64(s, radix: 16) ?? 0x888888
        self.init(.sRGB, red: Double((v >> 16) & 0xFF) / 255, green: Double((v >> 8) & 0xFF) / 255,
                  blue: Double(v & 0xFF) / 255, opacity: 1)
    }
}

/// Linear-style status glyph: dashed ring for triage, filling ring for progress, solid for done.
struct StatusIcon: View {
    let key: String
    var color: Color? = nil
    var size: CGFloat = 12

    var body: some View {
        let c = color ?? Color(hex: Theme.statusColors[key] ?? "#5A7386")
        ZStack {
            switch key {
            case "triage":
                Circle().strokeBorder(c, style: StrokeStyle(lineWidth: 1.5, dash: [2, 1.6]))
            case "done":
                Circle().fill(c)
                Image(systemName: "checkmark").font(.system(size: size * 0.5, weight: .heavy)).foregroundStyle(.white)
            case "canceled":
                Circle().fill(c.opacity(0.8))
                Image(systemName: "xmark").font(.system(size: size * 0.45, weight: .heavy)).foregroundStyle(.white)
            default:
                Circle().strokeBorder(c, lineWidth: 1.5)
                Circle().trim(from: 0, to: fill).stroke(c, style: StrokeStyle(lineWidth: size / 2 - 2))
                    .rotationEffect(.degrees(-90)).padding(size / 4 + 0.5)
            }
        }
        .frame(width: size, height: size)
        .accessibilityLabel(Text(key))
    }

    private var fill: CGFloat {
        switch key {
        case "in_progress": 0.5
        case "waiting": 0.25
        case "review": 0.75
        default: 0
        }
    }
}

/// Signal-bar priority mark; urgent gets an orange square like Linear.
struct PriorityIcon: View {
    let priority: Int

    var body: some View {
        Group {
            if priority == 1 {
                RoundedRectangle(cornerRadius: 3).fill(Color(hex: "#E0782F"))
                    .overlay(Text("!").font(.system(size: 9, weight: .black)).foregroundStyle(.white))
                    .frame(width: 12, height: 12)
            } else {
                HStack(alignment: .bottom, spacing: 1.5) {
                    ForEach(0..<3) { i in
                        RoundedRectangle(cornerRadius: 1)
                            .fill(filled(i) ? Theme.text.opacity(0.85) : Color(hex: "#C6D7E6"))
                            .frame(width: 3, height: 4 + CGFloat(i) * 3.5)
                    }
                }
                .frame(width: 12, height: 12, alignment: .bottom)
            }
        }
        .accessibilityLabel(Text(Theme.priorityNames[priority] ?? ""))
    }

    private func filled(_ i: Int) -> Bool {
        switch priority {
        case 2: true
        case 3: i < 2
        case 4: i < 1
        default: false
        }
    }
}

/// Small rounded chip used in toolbars and rows.
struct Chip<Label: View>: View {
    var small = false
    @ViewBuilder var label: () -> Label

    var body: some View {
        HStack(spacing: 5) { label() }
            .font(.system(size: small ? 11 : 12))
            .foregroundStyle(Theme.text.opacity(0.85))
            .padding(.horizontal, small ? 6 : 8)
            .frame(height: small ? 18 : 24)
            .background(Color.white.opacity(0.9), in: RoundedRectangle(cornerRadius: small ? 5 : 7))
            .overlay(RoundedRectangle(cornerRadius: small ? 5 : 7).strokeBorder(Theme.line))
    }
}

/// Glass panel: frosted, rounded, used for the list and floating cards.
struct GlassPanel: ViewModifier {
    var radius: CGFloat = 14

    func body(content: Content) -> some View {
        content.glassEffect(.regular.tint(Color.white.opacity(0.35)), in: RoundedRectangle(cornerRadius: radius))
    }
}

/// Near-opaque reading surface for thread content.
struct PaperPanel: ViewModifier {
    var radius: CGFloat = 14

    func body(content: Content) -> some View {
        // The shadow belongs to the background shape only: a shadow on the content would
        // re-render the whole scrolling subtree offscreen on every frame.
        content
            .background {
                RoundedRectangle(cornerRadius: radius).fill(Theme.paper)
                    .shadow(color: Theme.accent.opacity(0.07), radius: 15, y: 8)
            }
            .overlay(RoundedRectangle(cornerRadius: radius).strokeBorder(Color.white))
    }
}

extension View {
    func glassPanel(radius: CGFloat = 14) -> some View { modifier(GlassPanel(radius: radius)) }
    func paperPanel(radius: CGFloat = 14) -> some View { modifier(PaperPanel(radius: radius)) }
    /// Scrolling content sits on a flat, light surface; live glass is reserved for static chrome.
    func listPanel(radius: CGFloat = 14) -> some View {
        background(Color(hex: "#F6FAFE").opacity(0.92), in: RoundedRectangle(cornerRadius: radius))
            .overlay(RoundedRectangle(cornerRadius: radius).strokeBorder(Color.white))
    }
}

enum Format {
    private static let time: DateFormatter = { let f = DateFormatter(); f.dateFormat = "H:mm"; return f }()
    private static let weekday: DateFormatter = { let f = DateFormatter(); f.dateFormat = "EEE"; return f }()
    private static let day: DateFormatter = { let f = DateFormatter(); f.dateFormat = "MMM d"; return f }()
    private static let full: DateFormatter = { let f = DateFormatter(); f.dateStyle = .medium; f.timeStyle = .short; return f }()

    /// Mail-list style relative time: "9:41", "Yesterday", "Fri", "Oct 3".
    static func short(_ ms: Int64) -> String {
        let d = Date(timeIntervalSince1970: Double(ms) / 1000)
        let cal = Calendar.current
        if cal.isDateInToday(d) { return time.string(from: d) }
        if cal.isDateInYesterday(d) { return "Yesterday" }
        if let days = cal.dateComponents([.day], from: d, to: Date()).day, days < 7 { return weekday.string(from: d) }
        return day.string(from: d)
    }

    static func long(_ ms: Int64) -> String { full.string(from: Date(timeIntervalSince1970: Double(ms) / 1000)) }

    /// "2026-10-16" -> "Fri, Oct 16".
    static func due(_ s: String) -> String {
        let p = DateFormatter()
        p.dateFormat = "yyyy-MM-dd"
        guard let d = p.date(from: s) else { return s }
        let o = DateFormatter()
        o.dateFormat = "EEE, MMM d"
        return o.string(from: d)
    }

    static func size(_ bytes: Int64) -> String { ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file) }
}
