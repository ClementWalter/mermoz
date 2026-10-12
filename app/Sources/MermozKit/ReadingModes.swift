/// Per-message choice between the email as sent and the AI's clean version.
import Foundation

/// How a message is shown: as sent (rendered HTML), or the AI's clean version.
public enum ReadingMode: String, Codable, Sendable {
    case original, optimized
}

/// Each message keeps its own reading mode; messages never toggled show `fallback`.
/// Only choices that differ from `fallback` are stored, oldest dropped past `limit`,
/// so the saved list stays small however much mail is read.
public struct ReadingModes: Codable, Equatable, Sendable {
    public private(set) var fallback: ReadingMode
    /// Message ids set away from `fallback`, oldest first.
    private var chosen: [String: ReadingMode] = [:]
    private var order: [String] = []
    private var limit: Int

    public init(fallback: ReadingMode = .original, limit: Int = 500) {
        self.fallback = fallback
        self.limit = limit
    }

    public subscript(message id: String) -> ReadingMode {
        get { chosen[id] ?? fallback }
        set {
            order.removeAll { $0 == id }
            if newValue == fallback {
                chosen[id] = nil
                return
            }
            chosen[id] = newValue
            order.append(id)
            while order.count > limit { chosen[order.removeFirst()] = nil }
        }
    }
}
