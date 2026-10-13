/// The Feed's focused card and the reading pane move together: on j/k and the arrows, when
/// an action takes a card away, and when undo brings it back. Kept pure so the pairing is
/// tested; the app applies `focus` and `pane` in one go.
import Foundation

public struct FeedNavigator: Equatable, Sendable {
    /// The focused card.
    public var focus: String?
    /// The thread the reading pane shows.
    public var pane: String?

    public init(focus: String?, pane: String?) {
        self.focus = focus
        self.pane = pane
    }

    private mutating func land(on card: FeedCard?) {
        focus = card?.id
        pane = card.map(FeedSelection.thread)
    }

    /// j/k and the arrows.
    public mutating func move(by delta: Int, in feed: Feed) {
        let id = FeedFocus.step(feed.cards.map(\.id), from: focus, by: delta)
        land(on: feed.card(id))
    }

    /// An action took `cardId` away: the next card (the previous one if it was the
    /// last), and its mail in the pane.
    public mutating func leave(_ cardId: String, in feed: Feed) {
        let ids = feed.cards.map(\.id)
        guard focus == cardId || pane == feed.card(cardId).map(FeedSelection.thread) else { return }
        land(on: feed.card(FeedFocus.after(removing: cardId, from: ids)))
    }

    /// Undo brought these threads back: their card is focused and shown again.
    public mutating func restore(_ threadIds: [String], in feed: Feed) {
        guard let card = feed.cards.first(where: { !Set($0.threadIds).isDisjoint(with: threadIds) }) else { return }
        land(on: card)
    }

    /// The cards around the focus, whose mail is worth fetching ahead.
    public func neighbours(in feed: Feed) -> [String] {
        let cards = feed.cards
        guard let i = cards.firstIndex(where: { $0.id == focus }) else { return [] }
        return [i + 1, i - 1].filter { cards.indices.contains($0) }.map { FeedSelection.thread(of: cards[$0]) }
    }
}

/// The focused card stays where it stands across a refresh that would move it to
/// another section (an Unread card once read), so focus and pane stay put under the
/// user's eyes; it takes its new place at the first refresh after focus moves on.
public enum FeedHold {
    public static func keep(_ focus: String?, from old: Feed?, in new: Feed) -> Feed {
        guard let focus, let old,
              let si = old.sections.firstIndex(where: { $0.cards.contains { $0.id == focus } }),
              let ci = old.sections[si].cards.firstIndex(where: { $0.id == focus }),
              var card = new.card(focus)
        else { return new }
        let key = old.sections[si].key
        guard card.section != key else { return new }
        var next = new
        for i in next.sections.indices { next.sections[i].cards.removeAll { $0.id == focus } }
        next.sections.removeAll { $0.cards.isEmpty }
        card.section = key
        if let s = next.sections.firstIndex(where: { $0.key == key }) {
            next.sections[s].cards.insert(card, at: min(ci, next.sections[s].cards.count))
        } else {
            // Its section is gone: put back after the sections that came before it.
            let before = Set(old.sections[..<si].map(\.key))
            let at = next.sections.firstIndex { !before.contains($0.key) } ?? next.sections.count
            next.sections.insert(FeedSection(key: key, title: old.sections[si].title, cards: [card]), at: at)
        }
        return next
    }
}

/// Recently fetched conversations, so moving to a card shows its mail at once.
public struct DetailCache: Sendable {
    public let capacity: Int
    private var entries: [String: ThreadDetail] = [:]
    private var order: [String] = []

    public init(capacity: Int = 24) { self.capacity = capacity }

    public func get(_ id: String) -> ThreadDetail? { entries[id] }

    /// Keeps `detail` under `id` (the thread asked for), dropping the oldest past capacity.
    public mutating func put(_ detail: ThreadDetail, for id: String) {
        if entries[id] == nil { order.append(id) }
        entries[id] = detail
        while order.count > capacity { entries[order.removeFirst()] = nil }
    }

    public mutating func drop(_ id: String) {
        entries[id] = nil
        order.removeAll { $0 == id }
    }
}
