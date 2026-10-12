/// Which single-key shortcut a key press means, wherever the keyboard focus is.
import Foundation

public enum ShortcutKey {
    /// The shortcut character for a key press, or nil when it isn't one: chords with
    /// ⌘, ⌃ or ⌥ belong to menus, and "R" means "r" (Caps Lock or Shift).
    public static func normalize(_ characters: String, command: Bool, control: Bool, option: Bool, known: String) -> String? {
        guard !command, !control, !option, characters.count == 1 else { return nil }
        let c = characters.first!.isLetter ? characters.lowercased() : characters
        return known.contains(c) ? c : nil
    }
}
