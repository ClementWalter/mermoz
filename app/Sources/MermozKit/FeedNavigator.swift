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
