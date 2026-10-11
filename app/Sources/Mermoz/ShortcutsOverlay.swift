/// Keyboard help (press ?): every shortcut on one card, closed with Esc or a click.
import SwiftUI

struct ShortcutsOverlay: View {
    @Environment(AppModel.self) private var model

    private let groups: [(String, [(String, String)])] = [
        ("Navigate", [("j / ↓", "Next thread"), ("k / ↑", "Previous thread"), ("⇧↑ / ⇧↓", "Select several"), ("⇧ / ⌘ click", "Select a range / one more"), ("Esc", "Close message or panel"), ("⌘← / ⌘→", "Back / forward"), ("⌘K or /", "Search or run a command")]),
        ("Sort", [("y", "Accept the AI's suggestion"), ("e", "Done"), ("a", "Archive"), ("⌫ / ⌘⌫ / #", "Delete"), ("u", "Mark unread"), ("⌘Z", "Undo"),
                  ("s", "Set status"), ("p", "Set priority"), ("m", "Move to project")]),
        ("Write", [("r", "Reply"), ("c", "New message"), ("⌘↩", "Send / approve"), ("Esc", "Discard draft")]),
        ("Help", [("? or ⌘/", "Show this help")]),
    ]

    var body: some View {
        ZStack {
            Color(hex: "#D6E6F4").opacity(0.45).ignoresSafeArea()
                .onTapGesture { model.showShortcuts = false }
            VStack(alignment: .leading, spacing: 18) {
                Text("Keyboard shortcuts").font(.system(size: 17, weight: .semibold))
                Grid(alignment: .leading, horizontalSpacing: 36, verticalSpacing: 18) {
                    GridRow {
                        group(groups[0]); group(groups[1])
                    }
                    GridRow {
                        group(groups[2]); group(groups[3])
                    }
                }
            }
            .foregroundStyle(Theme.text)
            .padding(28)
            .frame(width: 600, alignment: .leading)
            .glassEffect(.regular.tint(Color.white.opacity(0.6)), in: RoundedRectangle(cornerRadius: 18))
        }
    }

    private func group(_ g: (String, [(String, String)])) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(g.0).font(.system(size: 11, weight: .semibold)).kerning(0.6).textCase(.uppercase).foregroundStyle(Theme.text3)
            ForEach(g.1, id: \.1) { key, label in
                HStack(spacing: 10) {
                    KeyHint(key).frame(minWidth: 64, alignment: .leading)
                    Text(label).font(.system(size: 13))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }
}
