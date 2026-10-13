/// Row selection flags: a j/k step notifies only the rows that gain or lose the selection.
import Foundation
import Observation
import Testing
@testable import MermozKit

/// Set from observation's change callback, which is not main-actor isolated.
private final class Fired: @unchecked Sendable { var on = false }

/// Whether row `id`'s view, reading its flag, is invalidated by moving from `a` to `b`.
@MainActor
private func notifies(_ id: String, from a: Set<String>, to b: Set<String>) -> Bool {
    let cells = SelectionCells()
    cells.select(a)
    let fired = Fired()
    withObservationTracking { _ = cells.flag(id).on } onChange: { fired.on = true }
    cells.select(b)
    return fired.on
}

@MainActor @Test func aStepFlipsTheTwoRows() {
    let cells = SelectionCells()
    cells.select(["t1"])
    #expect(cells.select(["t2"]) == ["t1", "t2"])
}

@MainActor @Test func theRowLeftBehindRedraws() {
    #expect(notifies("t1", from: ["t1"], to: ["t2"]))
}

@MainActor @Test func theRowReachedRedraws() {
    #expect(notifies("t2", from: ["t1"], to: ["t2"]))
}

@MainActor @Test func anotherRowDoesNotRedraw() {
    #expect(!notifies("t3", from: ["t1"], to: ["t2"]))
}

@MainActor @Test func selectingTheSameRowAgainRedrawsNothing() {
    #expect(!notifies("t1", from: ["t1"], to: ["t1"]))
}

@MainActor @Test func aRowShownLaterStartsSelected() {
    let cells = SelectionCells()
    cells.select(["t9"])
    #expect(cells.flag("t9").on)
}
