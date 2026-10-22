/// Suggestions on Feed cards: where the AI is confident a thread goes, shown as one chip
/// ("✦ Archive", "✦ → Voyage"). They are asked for just before a card shows, never in
/// bulk; y (or a click on the chip) accepts, n dismisses, and every answer teaches the
/// sender rules the daemon keeps.
import Foundation
import Observation

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
public struct SuggestionUndo: Codable, Sendable, Equatable {
    public var thread: ThreadState
    public var since: Int
    /// The other open threads of its conversation, which followed the answer.
    public var siblings: [ThreadState]?
}

/// One thread's confident suggestion, and the evidence behind it.
public struct ThreadSuggestion: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var suggestion: Suggestion
    /// "You filed 9 of your 12 other threads with these people in Voyage".
    public var why: String?

    public init(id: String, suggestion: Suggestion, why: String? = nil) {
        self.id = id; self.suggestion = suggestion; self.why = why
    }
}

/// `threads.suggest`.
public struct SuggestBatch: Codable, Sendable {
    public var suggestions: [ThreadSuggestion]
    public var paused: String?
    public var providerName: String?
    public var offer: ProviderOffer?
}

/// `threads.suggest_column`: what ✨ on a column did.
public struct ColumnSuggestions: Codable, Sendable {
    public var asked: Int
    public var suggested: [String]
    public var paused: String?
    public var providerName: String?
}

/// `sort.answer`.
public struct AnswerResult: Codable, Sendable {
    public var undo: SuggestionUndo
    /// "Learned: mail from X goes to Voyage", once, when this answer made a rule.
    public var learned: String?
}

/// Something suggestions learned from the user's answers.
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
        let to = SuggestionText.target(status: status, projects: projects, statusName: statusName)
        return kind == "rule" ? "Mail from \(name) goes to \(to)" : "Won't suggest \(to) for \(name) again"
    }
}

/// Plain-language text of suggestions.
public enum SuggestionText {
    /// "Archive · Voyage": a status by its display name, nested projects by their leaf.
    public static func target(status: String?, projects: [String], statusName: (String) -> String) -> String {
        var parts: [String] = status.map { [statusName($0)] } ?? []
        parts += projects.map { $0.split(separator: "/").last.map(String.init) ?? $0 }
        return parts.joined(separator: " · ")
    }

    /// The chip after its ✦: "Archive", "→ Voyage" for a project alone, "Done · Voyage".
    public static func chip(_ sg: Suggestion, statusName: (String) -> String) -> String {
        let to = target(status: sg.status, projects: sg.projects, statusName: statusName)
        return sg.status == nil ? "→ " + to : to
    }
}

/// The chips the Feed shows, and which threads to ask about next. Pure, so the flow is
/// tested without a daemon.
public struct FeedSuggestions: Equatable, Sendable {
    /// Below this confidence a guess is never shown (the daemon applies the same bar).
    public static let minConfidence = 0.7
    /// Cards after the focused one asked about ahead, so the chip is there on arrival.
    public static let ahead = 3
    /// Threads per request: the daemon serves at most this many.
    public static let perRequest = 6

    /// Chips by thread id.
    public private(set) var shown: [String: ThreadSuggestion] = [:]
    /// The latest mail each chip was made for; new mail retires it.
    private var madeFor: [String: Int64] = [:]
    /// Threads asked about, by "id@lastDate": asked again only after new mail.
    private var asked: Set<String> = []

    public init() {}

    /// Every field shown is confident, and there is something to show.
    public static func confident(_ sg: Suggestion) -> Bool {
        guard !sg.isEmpty else { return false }
        if sg.status != nil, (sg.statusConfidence ?? 0) < minConfidence { return false }
        if !sg.projects.isEmpty, (sg.projectConfidence ?? 0) < minConfidence { return false }
        return true
    }

    private static func key(_ t: ThreadSummary) -> String { "\(t.id)@\(t.lastDate)" }

    /// The threads to ask about now: the focused card and the few after it, then the
    /// cards on screen; single cards only, each once per new mail.
    public func wanted(in feed: Feed, visible: Set<String>, focus: String?) -> [String] {
        let cards = feed.cards.filter { !$0.isBatch }
        let start = cards.firstIndex { $0.id == focus } ?? 0
        let near = cards[start...].prefix(Self.ahead + 1)
        var seen = Set<String>()
        var out: [String] = []
        for c in Array(near) + cards.filter({ visible.contains($0.id) })
        where !asked.contains(Self.key(c.thread)) && seen.insert(c.thread.id).inserted {
            out.append(c.thread.id)
        }
        return Array(out.prefix(Self.perRequest))
    }

    /// Records a request for `ids` so they are not asked again before new mail.
    public mutating func asking(_ ids: [String], in feed: Feed) {
        let wanted = Set(ids)
        for c in feed.cards where wanted.contains(c.thread.id) { asked.insert(Self.key(c.thread)) }
    }

    /// The request failed: these threads may be asked again.
    public mutating func failed(_ ids: [String], in feed: Feed) {
        let failed = Set(ids)
        for c in feed.cards where failed.contains(c.thread.id) { asked.remove(Self.key(c.thread)) }
    }

    /// Keeps the confident suggestions of a reply as chips.
    public mutating func received(_ batch: [ThreadSuggestion], in feed: Feed) {
        for s in batch where Self.confident(s.suggestion) {
            guard let t = feed.cards.first(where: { $0.thread.id == s.id })?.thread else { continue }
            shown[s.id] = s
            madeFor[s.id] = t.lastDate
        }
    }

    /// The chip on a card, if any.
    public func chip(for card: FeedCard) -> ThreadSuggestion? {
        card.isBatch ? nil : shown[card.thread.id]
    }

    /// A reloaded feed: chips of threads gone from it, or with new mail since, retire.
    public mutating func sync(with feed: Feed) {
        let now = Dictionary(feed.cards.filter { !$0.isBatch }.map { ($0.thread.id, $0.thread.lastDate) },
                             uniquingKeysWith: { a, _ in a })
        for id in shown.keys where now[id] == nil || now[id] != madeFor[id] {
            shown[id] = nil
            madeFor[id] = nil
        }
    }

    /// y or a click on the chip: the chip leaves the card, and focus and the reading pane
    /// move on to the next card. Returns the suggestion to accept.
    public mutating func accept(_ card: FeedCard, nav: inout FeedNavigator, in feed: Feed) -> ThreadSuggestion? {
        guard let s = take(card) else { return nil }
        nav.leave(card.id, in: feed)
        return s
    }

    /// n: the chip leaves; the card and the focus stay. Returns the suggestion turned down.
    public mutating func dismiss(_ card: FeedCard) -> ThreadSuggestion? {
        take(card)
    }

    private mutating func take(_ card: FeedCard) -> ThreadSuggestion? {
        guard !card.isBatch, let s = shown.removeValue(forKey: card.thread.id) else { return nil }
        return s
    }

    /// Undo, or an answer that did not land: the chip is back on its card.
    public mutating func putBack(_ s: ThreadSuggestion, lastDate: Int64) {
        shown[s.id] = s
        madeFor[s.id] = lastDate
    }
}

/// The Feed's chips with the daemon behind them: asks for suggestions just in time and
/// sends answers, which the daemon learns from (`sort.answer`).
@MainActor
@Observable
public final class FeedSuggester {
    public private(set) var state = FeedSuggestions()
    /// Why the AI is on hold, from the last reply ("credits", "unavailable", "off", "limit").
    public private(set) var paused: String?

    public init() {}

    /// Asks about the cards that need it now; the chips appear when the reply lands.
    public func refresh(client: DaemonClient, feed: Feed, visible: Set<String>, focus: String?) async throws {
        let ids = state.wanted(in: feed, visible: visible, focus: focus)
        guard !ids.isEmpty else { return }
        state.asking(ids, in: feed)
        do {
            let batch = try await client.suggest(ids)
            state.received(batch.suggestions, in: feed)
            paused = batch.paused
        } catch {
            state.failed(ids, in: feed)
            throw error
        }
    }

    public func sync(with feed: Feed) { state.sync(with: feed) }

    /// y / a click on the chip. The chip goes and `nav` moves to the next card at once;
    /// the yes is sent after. Nil when the card has no chip.
    public func accept(_ card: FeedCard, nav: inout FeedNavigator, in feed: Feed) -> ThreadSuggestion? {
        state.accept(card, nav: &nav, in: feed)
    }

    /// n: the chip goes, the card stays.
    public func dismiss(_ card: FeedCard) -> ThreadSuggestion? {
        state.dismiss(card)
    }

    /// Sends an answer ("yes" or "no") for a chip taken off `card`; if it does not land,
    /// the chip comes back.
    public func send(_ answer: String, _ s: ThreadSuggestion, of card: FeedCard, client: DaemonClient) async throws -> AnswerResult {
        do {
            return try await client.answerSuggestion(s.id, answer: answer)
        } catch {
            state.putBack(s, lastDate: card.thread.lastDate)
            throw error
        }
    }

    /// Undo: the answer is forgotten and the chip is back.
    public func undo(_ token: SuggestionUndo, _ s: ThreadSuggestion, lastDate: Int64, client: DaemonClient) async throws {
        try await client.undoAnswer(token)
        state.putBack(s, lastDate: lastDate)
    }
}
