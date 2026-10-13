/// Later: the picker's quick choices and what the user typed, as mermozd resolves them
/// (later.options / later.parse), and which key picks which choice.
import Foundation

/// A ready-made choice ("Friday evening") with its one-key shortcut in the picker.
public struct LaterChoice: Codable, Sendable, Equatable, Identifiable {
    public var key: String
    public var label: String
    /// When it comes back, Unix milliseconds.
    public var until: Int64?
    /// "Fri 18:00", "tomorrow, 08:00".
    public var when: String
    public var id: String { key }

    public init(key: String, label: String, until: Int64?, when: String) {
        self.key = key; self.label = label; self.until = until; self.when = when
    }
}

/// What the typed words resolve to.
public struct LaterTime: Codable, Sendable, Equatable {
    public var until: Int64?
    public var when: String

    public init(until: Int64?, when: String) { self.until = until; self.when = when }
}

public enum LaterKeys {
    /// "Pick a date & time…".
    public static let pick = "p"

    /// The choice a key picks, if any.
    public static func choice(for key: String, in options: [LaterChoice]) -> LaterChoice? {
        options.first { $0.key == key.lowercased() }
    }

    /// How a choice reads in "You set this for …": "Friday evening" stays, "Tomorrow
    /// morning" becomes "tomorrow morning".
    public static func hint(_ label: String) -> String {
        let weekdays = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday",
                        "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        guard let first = label.split(separator: " ").first, !weekdays.contains(String(first)) else { return label }
        return label.prefix(1).lowercased() + label.dropFirst()
    }

    /// What the done strip says: "Later · Fri 18:00".
    public static func strip(_ when: String) -> String { "Later · \(when)" }
}
