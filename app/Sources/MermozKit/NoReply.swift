/// Addresses nobody reads (no-reply, notifications, bounces), and the composer's warning
/// about them. Same rules as the daemon's `reply::is_no_reply`.
import Foundation

public enum NoReply {
    /// Local parts (separators removed) that mark an address nobody reads.
    static let marks = ["noreply", "donotreply", "nepasrepondre", "nepasrépondre"]
    /// Whole local parts that are robots, not people.
    static let robots: Set<String> = ["notifications", "notification", "mailerdaemon"]

    public static func isNoReply(_ email: String) -> Bool {
        let e = email.trimmingCharacters(in: .whitespaces)
        guard let at = e.lastIndex(of: "@") else { return false }
        let squashed = String(e[..<at].lowercased().filter { !"-_.+".contains($0) })
        return marks.contains { squashed.contains($0) } || robots.contains(squashed)
    }
}

/// Shown above a reply whose To has an address that doesn't accept replies.
public struct NoReplyWarning: Equatable, Sendable {
    public static let title = "This address doesn't accept replies"

    /// The To addresses nobody reads.
    public var addresses: [String]
    /// The person to write to instead, when the mail names one.
    public var suggestion: Address?

    /// Nil when every To address accepts replies.
    public init?(to: [String], suggestion: Address?) {
        let dead = to.filter(NoReply.isNoReply)
        guard !dead.isEmpty else { return nil }
        addresses = dead
        let taken = to.contains { $0.caseInsensitiveCompare(suggestion?.email ?? "") == .orderedSame }
        self.suggestion = taken ? nil : suggestion
    }

    /// "no-reply@x.example won't reach anyone."
    public var detail: String {
        "\(addresses.joined(separator: ", ")) won't reach anyone."
    }

    /// "Did you mean franck@plomberie.example?"
    public var question: String? {
        suggestion.map { "Did you mean \($0.email)?" }
    }

    /// The To line with the no-reply addresses swapped for the suggestion.
    public func applied(to: [String]) -> [String] {
        guard let s = suggestion else { return to }
        let kept = to.filter { !NoReply.isNoReply($0) }
        return kept + [s.email]
    }
}
