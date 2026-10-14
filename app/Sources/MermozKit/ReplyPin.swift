/// Slide to reply: a horizontal swipe on a bubble pins the reply to that message once it
/// has travelled far enough; the composer then quotes what it answers.
import Foundation

/// A two-finger horizontal swipe on a bubble, from its first move to the fingers lifting.
public struct SwipeToReply: Equatable, Sendable {
    /// Travel that pins the reply when the fingers lift.
    public static let threshold: CGFloat = 56
    /// The bubble follows the fingers this far at most.
    public static let maxOffset: CGFloat = 80

    /// How far the fingers have gone (signed, points).
    public private(set) var travel: CGFloat = 0
    public private(set) var active = false

    public init() {}

    /// Whether a scroll with these deltas is this swipe: already started, or clearly
    /// sideways (a vertical scroll stays with the chat).
    public func takes(dx: CGFloat, dy: CGFloat) -> Bool {
        active || (abs(dx) > 2 && abs(dx) > 1.5 * abs(dy))
    }

    /// The fingers moved by `dx`: how far the bubble is drawn aside (0...maxOffset).
    @discardableResult
    public mutating func move(_ dx: CGFloat) -> CGFloat {
        active = true
        travel += dx
        return offset
    }

    public var offset: CGFloat { min(abs(travel), Self.maxOffset) }

    /// How close the swipe is to pinning (0...1), for the ↩ hint.
    public var progress: CGFloat { min(abs(travel) / Self.threshold, 1) }

    /// The fingers lifted: whether the reply is pinned. The swipe starts over.
    public mutating func end() -> Bool {
        let pinned = active && abs(travel) >= Self.threshold
        self = SwipeToReply()
        return pinned
    }
}

/// The line over the composer that says what a pinned reply answers.
public enum ReplyQuote {
    /// "Replying to Mme Adam: “Cordialement…”": the sender (You for the user) and the
    /// start of the message's text on one line.
    public static func preview(_ m: Message, sender: String, limit: Int = 60) -> String {
        let text = BubbleParts.compact(m).text
            .split(whereSeparator: \.isNewline).map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }.joined(separator: " ")
        let shown = text.count > limit ? String(text.prefix(limit)).trimmingCharacters(in: .whitespaces) + "…" : text
        return "Replying to \(m.isFromMe ? "You" : sender): “\(shown)”"
    }
}
