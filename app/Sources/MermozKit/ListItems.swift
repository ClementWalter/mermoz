/// The List as table rows: group headers, sub-group headers, threads and the "loading
/// more" row, flattened in display order, plus what a new page changes in an old one.
import Foundation

public enum ListItem: Equatable, Sendable {
    case group(ThreadGroup)
    case subgroup(ThreadGroup)
    /// `key` is unique per row: a thread in two project groups shows in both.
    case thread(ThreadSummary, key: String)
    case loadMore(loaded: Int, total: Int)

    public var key: String {
        switch self {
        case let .group(g): "g|" + g.id
        case let .subgroup(g): "s|" + g.id
        case let .thread(_, key): key
        case .loadMore: "more"
        }
    }

    public var threadId: String? {
        if case let .thread(t, _) = self { t.id } else { nil }
    }
}

public enum ListItems {
    public static func flatten(_ groups: [ThreadGroup], loaded: Int, total: Int) -> [ListItem] {
        var out: [ListItem] = []
        for g in groups {
            if g.field != .none { out.append(.group(g)) }
            if g.subgroups.isEmpty {
                out += g.threads.map { .thread($0, key: g.id + "|" + $0.id) }
            } else {
                for sub in g.subgroups {
                    out.append(.subgroup(sub))
                    out += sub.threads.map { .thread($0, key: g.id + "/" + sub.id + "|" + $0.id) }
                }
            }
        }
        if total > loaded { out.append(.loadMore(loaded: loaded, total: total)) }
        return out
    }

    /// The first row showing this thread.
    public static func row(of threadId: String, in items: [ListItem]) -> Int? {
        items.firstIndex { $0.threadId == threadId }
    }

    /// How a table showing `old` catches up with `new`.
    public enum Change: Equatable, Sendable {
        case none
        /// Same rows in the same places; only these changed content.
        case rows(IndexSet)
        /// Rows came, went or moved.
        case all
    }

    public static func change(from old: [ListItem], to new: [ListItem]) -> Change {
        guard old.count == new.count, zip(old, new).allSatisfy({ $0.key == $1.key }) else { return .all }
        let changed = IndexSet(old.indices.filter { old[$0] != new[$0] })
        return changed.isEmpty ? .none : .rows(changed)
    }
}
