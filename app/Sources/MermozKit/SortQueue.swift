/// The Sort queue: one email at a time with the AI's suggestion, answered yes, no,
/// change or skip. Suggestions are prepared a few cards ahead, never in bulk, and
/// every answer teaches the queue about that sender.
import Foundation

/// The fields of a thread an answer can change, as they were before it.
public struct ThreadState: Codable, Sendable, Equatable {
    public var id: String
    public var unread: Bool
    public var status: String
    public var priority: Int
    public var projects: [String]
    public var due: String?

    public init(_ t: ThreadSummary) {
        id = t.id; unread = t.unread; status = t.status; priority = t.priority
        projects = t.projects; due = t.due
    }
}

/// What `sort.undo` needs to take an answer back.
public struct SortUndoToken: Codable, Sendable, Equatable {
    public var thread: ThreadState
    public var since: Int
}

/// One email of the queue, with its suggestion and the evidence behind it.
public struct SortCard: Codable, Sendable, Equatable, Identifiable {
    public var thread: ThreadSummary
    /// "You filed 9 of your 12 other threads with these people in Voyage".
    public var why: String?
    public var id: String { thread.id }

    public init(thread: ThreadSummary, why: String? = nil) {
        self.thread = thread; self.why = why
    }
}

/// `sort.next`.
public struct SortBatch: Codable, Sendable {
    public var cards: [SortCard]
    public var paused: String?
    public var providerName: String?
    public var offer: ProviderOffer?
}

/// `sort.answer`.
public struct SortAnswerResult: Codable, Sendable {
    public var undo: SortUndoToken
    /// "Learned: mail from X goes to Voyage", once, when this answer made a rule.
    public var learned: String?
}

/// Something the queue learned from the user's answers.
public struct LearnedItem: Codable, Sendable, Equatable, Identifiable {
    public var sender: String
    public var name: String
    public var status: String?
    public var projects: [String]
    /// "rule": suggested from now on; "avoid": never suggested again for this sender.
    public var kind: String
    public var count: Int

    public init(sender: String, name: String, status: String?, projects: [String], kind: String, count: Int) {
        self.sender = sender; self.name = name; self.status = status
        self.projects = projects; self.kind = kind; self.count = count
    }

    public var id: String { [kind, sender, status ?? "", projects.joined(separator: "\u{1F}")].joined(separator: "|") }

    /// "Mail from Ana goes to Voyage" / "Won't suggest Done · Mariage for Ana again".
    public func sentence(statusName: (String) -> String) -> String {
        let to = SortText.target(status: status, projects: projects, statusName: statusName)
        return kind == "rule" ? "Mail from \(name) goes to \(to)" : "Won't suggest \(to) for \(name) again"
    }
}

/// One open column's waiting count.
public struct SortColumnCount: Codable, Sendable, Equatable {
    public var status: String
    public var count: Int
}

/// `sort.state`: what waits to be sorted, and what was learned.
public struct SortInfo: Codable, Sendable, Equatable {
    public var toSort: Int
    public var byStatus: [SortColumnCount]
    public var configured: Bool
    public var paused: String?
    /// The instant sorting provider in use, by name.
    public var providerName: String?
    public var offer: ProviderOffer?
    public var learned: [LearnedItem]

    public init(toSort: Int = 0, byStatus: [SortColumnCount] = [], configured: Bool = false,
                paused: String? = nil, providerName: String? = nil, offer: ProviderOffer? = nil,
                learned: [LearnedItem] = []) {
        self.toSort = toSort; self.byStatus = byStatus; self.configured = configured
        self.paused = paused; self.providerName = providerName; self.offer = offer
        self.learned = learned
    }

    /// Threads of one column waiting to be sorted.
    public func count(_ status: String) -> Int { byStatus.first { $0.status == status }?.count ?? 0 }
}

/// The cards in hand. The first is on screen; the rest were prepared ahead.
public struct SortQueue: Equatable, Sendable {
    public private(set) var cards: [SortCard] = []
    public private(set) var skipped: [String] = []
    public private(set) var answered: [String] = []

    /// Below this many cards in hand, the next ones are asked for.
    public static let lookahead = 3

    public init() {}

    public var current: SortCard? { cards.first }

    /// Threads the daemon must not hand out again this session.
    public var exclude: [String] { cards.map(\.id) + skipped + answered }

    public var needsMore: Bool { cards.count < Self.lookahead }

    /// Adds newly prepared cards, leaving out any already in hand, skipped or answered.
    public mutating func add(_ new: [SortCard]) {
        let known = Set(exclude)
        var seen = Set<String>()
        cards += new.filter { !known.contains($0.id) && seen.insert($0.id).inserted }
    }

    /// Takes the card on screen off the queue once answered; returns it.
    @discardableResult
    public mutating func answer() -> SortCard? {
        guard !cards.isEmpty else { return nil }
        let c = cards.removeFirst()
        answered.append(c.id)
        return c
    }

    /// Sets the card on screen aside for this session.
    public mutating func skip() {
        guard !cards.isEmpty else { return }
        skipped.append(cards.removeFirst().id)
    }

    /// Brings an answered card back on screen (undo).
    public mutating func putBack(_ card: SortCard) {
        answered.removeAll { $0 == card.id }
        skipped.removeAll { $0 == card.id }
        cards.removeAll { $0.id == card.id }
        cards.insert(card, at: 0)
    }

    public mutating func reset() { self = SortQueue() }
}

/// Plain-language text of the Sort queue.
public enum SortText {
    /// "Archive · Voyage": a status by its display name, nested projects by their leaf.
    public static func target(status: String?, projects: [String], statusName: (String) -> String) -> String {
        var parts: [String] = status.map { [statusName($0)] } ?? []
        parts += projects.map { $0.split(separator: "/").last.map(String.init) ?? $0 }
        return parts.joined(separator: " · ")
    }

    /// Why sorting is on hold, for the user; nil when it is not.
    public static func paused(_ reason: String?, provider: String? = nil) -> String? {
        switch reason {
        case "credits": "Sorting paused: your \(provider ?? "TypeSafe") credits ran out"
        case nil: nil
        default: "Sorting paused: the AI is busy, trying again soon"
        }
    }

    /// "✦ 312 to sort".
    public static func toSort(_ n: Int, locale: Locale = .current) -> String {
        "\(n.formatted(.number.locale(locale))) to sort"
    }
}
