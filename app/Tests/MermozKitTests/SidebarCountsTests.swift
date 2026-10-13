/// Sidebar counts: only moved badges notify their views, and count refreshes coalesce.
import Foundation
import Observation
import Testing
@testable import MermozKit

private let before = Counts(status: ["unread": 3, "todo": 5], project: ["Billing": 2, "Travel": 1], account: ["acc": 8])
private let afterRead = Counts(status: ["unread": 2, "todo": 6], project: ["Billing": 2, "Travel": 1], account: ["acc": 8])

/// Set from observation's change callback, which is not main-actor isolated.
private final class Flag: @unchecked Sendable { var on = false }

/// Whether reading `key`'s cell, as a badge view does, is invalidated by applying `next`.
@MainActor
private func notifies(_ key: CountKey, from first: Counts, to next: Counts) -> Bool {
    let cells = CountCells()
    cells.apply(first)
    let fired = Flag()
    withObservationTracking { _ = cells.value(key) } onChange: { fired.on = true }
    cells.apply(next)
    return fired.on
}

@MainActor @Test func applyReportsOnlyTheMovedBadges() {
    let cells = CountCells()
    cells.apply(before)
    #expect(cells.apply(afterRead) == [.status("unread"), .status("todo")])
}

@MainActor @Test func aMovedBadgeRedraws() {
    #expect(notifies(.status("unread"), from: before, to: afterRead))
}

@MainActor @Test func anUnchangedProjectBadgeDoesNotRedraw() {
    #expect(!notifies(.project("Billing"), from: before, to: afterRead))
}

@MainActor @Test func anUnchangedAccountBadgeDoesNotRedraw() {
    #expect(!notifies(.account("acc"), from: before, to: afterRead))
}

@MainActor @Test func theSameAnswerTwiceRedrawsNothing() {
    #expect(!notifies(.status("unread"), from: before, to: before))
}

@MainActor @Test func aBadgeAskedForLaterStartsAtTheLastValue() {
    let cells = CountCells()
    cells.apply(before)
    #expect(cells.value(.project("Billing")) == 2)
}

@MainActor @Test func aCountThatDisappearsIsCleared() {
    let cells = CountCells()
    cells.apply(before)
    _ = cells.cell(.project("Travel"))
    cells.apply(Counts(status: before.status, project: ["Billing": 2], account: before.account))
    #expect(cells.value(.project("Travel")) == nil)
}

@MainActor @Test func derivedBadgesAreCellsToo() {
    let cells = CountCells()
    cells.apply(before, extra: [.reviewBadge: 4])
    #expect(cells.value(.reviewBadge) == 4)
}

// MARK: refresh coalescing

/// Lets a refresh's work ask for another refresh of the same coalescer.
@MainActor private final class Holder { var r: RefreshCoalescer! }

@MainActor @Test func aBurstOfRequestsRunsOnce() async {
    var calls = 0
    let r = RefreshCoalescer(delay: .milliseconds(20)) { calls += 1 }
    for _ in 0..<5 { r.request() }
    await r.settle()
    #expect(calls == 1)
}

@MainActor @Test func requestsDuringARunCauseExactlyOneMore() async {
    var calls = 0
    let h = Holder()
    h.r = RefreshCoalescer(delay: .milliseconds(10)) {
        calls += 1
        if calls == 1 {
            // Three requests arrive while the first refresh is in flight.
            for _ in 0..<3 { h.r.request() }
            try? await Task.sleep(for: .milliseconds(30))
        }
    }
    h.r.request()
    await h.r.settle()
    #expect(calls == 2)
}

@MainActor @Test func onlyOneRefreshIsInFlight() async {
    var inFlight = 0
    var most = 0
    let h = Holder()
    h.r = RefreshCoalescer(delay: .milliseconds(5)) {
        inFlight += 1
        most = max(most, inFlight)
        if h.r.runs == 1 { h.r.request() }
        try? await Task.sleep(for: .milliseconds(20))
        inFlight -= 1
    }
    h.r.request()
    await h.r.settle()
    #expect(most == 1)
}

@MainActor @Test func requestsAfterTheDelayRunAgain() async {
    var calls = 0
    let r = RefreshCoalescer(delay: .milliseconds(5)) { calls += 1 }
    r.request()
    await r.settle()
    r.request()
    await r.settle()
    #expect(calls == 2)
}
