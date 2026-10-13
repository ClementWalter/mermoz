/// Which rows are selected, as one observable flag per row.
///
/// A row reading its own flag is redrawn only when it gains or loses the selection, so
/// stepping with j/k redraws two rows instead of the whole list.
import Foundation
import Observation

@MainActor
@Observable
public final class SelectionFlag {
    public fileprivate(set) var on: Bool

    init(_ on: Bool) { self.on = on }
}

@MainActor
public final class SelectionCells {
    private var flags: [String: SelectionFlag] = [:]
    private var selected: Set<String> = []

    public init() {}

    public func flag(_ id: String) -> SelectionFlag {
        if let f = flags[id] { return f }
        let f = SelectionFlag(selected.contains(id))
        flags[id] = f
        return f
    }

    /// Sets the selected ids; returns the ids whose flag flipped.
    @discardableResult
    public func select(_ ids: Set<String>) -> Set<String> {
        let flipped = selected.symmetricDifference(ids)
        selected = ids
        // Only flipped flags are written: an Observable write notifies even when equal.
        for id in flipped { flags[id]?.on = ids.contains(id) }
        return flipped
    }
}
