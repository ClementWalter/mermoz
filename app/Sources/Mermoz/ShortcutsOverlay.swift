/// Keyboard help (press ?): every shortcut on one card, closed with Esc or a click.
import SwiftUI

struct ShortcutsOverlay: View {
    @Environment(AppModel.self) private var model

    private let groups: [(String, [(String, String)])] = [
        ("Navigate", [("j / ↓", "Next thread"), ("k / ↑", "Previous thread"), ("⇧↑ / ⇧↓", "Select several"), ("⇧ / ⌘ click", "Select a range / one more"), ("Esc", "Close message or panel"), ("⌘← / ⌘→", "Back / forward"), ("⌘K or /", "Search or run a command")]),
        ("Organize", [("y", "Accept the AI's suggestion"), ("x", "Dismiss the AI's suggestion"), ("e", "Done"), ("a", "Archive"), ("⌫ / ⌘⌫ / #", "Delete"), ("u", "Mark unread"), ("⌘Z", "Undo"),
                  ("s", "Set status"), ("p", "Set priority"), ("m", "Move to project"), ("⇧M", "Join selected into one conversation")]),
        ("Write", [("r", "Reply"), ("c", "New message"), ("t", "Chat about this"), ("⌘↩", "Send / approve"), ("Esc", "Discard draft")]),
        ("Help", [("? or ⌘/", "Show this help")]),
        ("Feed", [("↩", "Do what the card says"), ("1 2 3", "Pick an option"), ("y / n", "Accept / dismiss the ✦ suggestion"), ("l", "Later…"),
                  ("⇧L", "Later: type when"), ("d / ⌫", "Delete"), ("o", "Open the mail"), ("t", "Ask about this")]),
    ]

    var body: some View {
        ZStack {
            Color(hex: "#D6E6F4").opacity(0.45).ignoresSafeArea()
                .onTapGesture { model.showShortcuts = false }
            VStack(alignment: .leading, spacing: 18) {
                Text("Keyboard shortcuts").font(.system(size: 17, weight: .semibold))
                // Top-aligned so both columns start at the same height.
                Grid(alignment: .topLeading, horizontalSpacing: 40, verticalSpacing: 22) {
                    GridRow {
                        group(groups[0]); group(groups[1])
                    }
                    GridRow {
                        group(groups[2]); group(groups[3])
                    }
                    GridRow {
                        group(groups[4]); Color.clear.frame(height: 0)
                    }
                }
            }
            .foregroundStyle(Theme.text)
            .padding(28)
            .frame(width: 700, alignment: .leading)
            .glassEffect(.regular.tint(Color.white.opacity(0.6)), in: RoundedRectangle(cornerRadius: 18))
        }
    }

    private func group(_ g: (String, [(String, String)])) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(g.0).font(.system(size: 11, weight: .semibold)).kerning(0.6).textCase(.uppercase).foregroundStyle(Theme.text3)
            // A grid sizes the key column to the widest key, so every description lines up.
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 8) {
                ForEach(g.1, id: \.1) { key, label in
                    GridRow {
                        KeyHint(key)
                        Text(label).font(.system(size: 13))
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }
}
