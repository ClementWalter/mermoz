/// Single-key shortcuts: either case works, chords are left to menus.
import Testing
@testable import MermozKit

private let known = "jkeay#usmprc?/"

@Test(arguments: [
    ("r", false, "r"),
    ("R", false, "r"),
    ("?", false, "?"),
    ("#", false, "#"),
    ("x", false, nil),
    ("r", true, nil),
] as [(String, Bool, String?)])
func shortcutKeys(_ chars: String, _ command: Bool, _ want: String?) {
    #expect(ShortcutKey.normalize(chars, command: command, control: false, option: false, known: known) == want)
}
