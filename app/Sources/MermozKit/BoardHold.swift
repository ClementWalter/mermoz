/// The open card stays where it was clicked: reading it (or the AI's suggestion being
/// applied) changes its status, but the card only moves once another one is selected.
import Foundation

public struct BoardHold: Equatable, Sendable {
    /// The card as it was placed when selected: its grouping fields are the ones kept.
    public var placed: ThreadSummary
    /// The thread just before it in the list then; nil when it was first.
    public var after: String?

    public var id: String { placed.id }

    /// Holds `id` where it sits in `threads`; nil when it is not on screen.
    public init?(_ id: String, in threads: [ThreadSummary]) {
        guard let i = threads.firstIndex(where: { $0.id == id }) else { return nil }
        placed = threads[i]
        after = i > 0 ? threads[i - 1].id : nil
    }

    /// `threads` with the held card back at its place: the current card (fresh unread dot,
    /// subject, suggestion) with its old grouping fields, right after its old neighbour.
    public func apply(_ threads: [ThreadSummary]) -> [ThreadSummary] {
        guard let i = threads.firstIndex(where: { $0.id == placed.id }) else { return threads }
        var card = threads[i]
        card.status = placed.status
        card.priority = placed.priority
        card.projects = placed.projects
        card.accountId = placed.accountId
        var rest = threads
        rest.remove(at: i)
        // Groups keep relative order, so landing after the old neighbour is landing at the old row.
        let at: Int = if let after { rest.firstIndex { $0.id == after }.map { $0 + 1 } ?? i } else { 0 }
        rest.insert(card, at: min(at, rest.count))
        return rest
    }
}

public enum BoardStep {
    /// The card to select once `ids` leave the board: the next one down the same column,
    /// else the one above, else the same row of the nearest column still holding cards.
    public static func next(after ids: [String], in columns: [[String]]) -> String? {
        let gone = Set(ids)
        guard let ci = columns.firstIndex(where: { $0.contains { gone.contains($0) } }),
              let ri = columns[ci].firstIndex(where: { gone.contains($0) }) else { return nil }
        let left = columns.map { $0.filter { !gone.contains($0) } }
        if !left[ci].isEmpty { return left[ci][min(ri, left[ci].count - 1)] }
        let nearest = left.indices.filter { !left[$0].isEmpty }.min { abs($0 - ci) < abs($1 - ci) }
        return nearest.map { left[$0][min(ri, left[$0].count - 1)] }
    }
}
