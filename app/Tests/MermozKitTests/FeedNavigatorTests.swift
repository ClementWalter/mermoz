/// The Feed's focus and reading pane move together: arrows, actions that take a card
/// away, undo; and the cache that lets the pane show a card at once.
import Foundation
import Testing
@testable import MermozKit

private func thread(_ id: String) -> ThreadSummary {
    ThreadSummary(id: id, number: "M-1", accountId: "acc", subject: "S", snippet: "", participants: [],
                  lastFrom: "Marie", lastDate: 0, unread: false, messageCount: 1, hasAttachments: false,
                  status: "todo", priority: 0)
}

private func card(_ id: String, members: [ThreadSummary] = []) -> FeedCard {
    FeedCard(id: id, kind: members.isEmpty ? .reply : .cleanup, section: "now", thread: thread("thr_\(id)"),
             members: members, title: "T")
}

private let feed = Feed(sections: [FeedSection(key: "now", title: "Now", cards: [card("a"), card("b"), card("c")])])

private func on(_ id: String) -> FeedNavigator { FeedNavigator(focus: id, pane: "thr_\(id)") }

// MARK: arrows and j/k

@Test func anArrowShowsTheFocusedCardInThePane() {
    var nav = on("a")
    nav.move(by: 1, in: feed)
    #expect(nav.pane == "thr_b")
}

@Test func anArrowMovesTheFocus() {
    var nav = on("a")
    nav.move(by: 1, in: feed)
    #expect(nav.focus == "b")
}

@Test func theFirstArrowLandsOnTheFirstCard() {
    var nav = FeedNavigator(focus: nil, pane: nil)
    nav.move(by: 1, in: feed)
    #expect(nav == on("a"))
}

@Test func anArrowOntoABatchShowsItsLatestMail() {
    let f = Feed(sections: [FeedSection(key: "later", title: "Can wait", cards: [card("a"), card("batch:cleanup", members: [thread("x"), thread("y")])])])
    var nav = on("a")
    nav.move(by: 1, in: f)
    #expect(nav.pane == "x")
}

// MARK: actions

@Test func anActionMovesFocusAndPaneToTheNextCard() {
    var nav = on("b")
    nav.leave("b", in: feed)
    #expect(nav == on("c"))
}

@Test func anActionOnTheLastCardGoesBackOne() {
    var nav = on("c")
    nav.leave("c", in: feed)
    #expect(nav == on("b"))
}

@Test func aCardLeavingElsewhereKeepsTheFocus() {
    var nav = on("a")
    nav.leave("c", in: feed)
    #expect(nav == on("a"))
}

@Test func theOnlyCardLeavingEmptiesThePane() {
    let f = Feed(sections: [FeedSection(key: "now", title: "Now", cards: [card("a")])])
    var nav = on("a")
    nav.leave("a", in: f)
    #expect(nav == FeedNavigator(focus: nil, pane: nil))
}

// MARK: undo

@Test func undoRestoresTheCardAndThePane() {
    var nav = on("b")
    nav.leave("b", in: feed)
    nav.restore(["thr_b"], in: feed)
    #expect(nav == on("b"))
}

@Test func undoOfABatchRestoresTheBatch() {
    let f = Feed(sections: [FeedSection(key: "later", title: "Can wait", cards: [card("a"), card("batch:cleanup", members: [thread("x"), thread("y")])])])
    var nav = on("a")
    nav.restore(["y"], in: f)
    #expect(nav.focus == "batch:cleanup")
}

// MARK: fetching ahead

@Test func theNeighboursAreFetchedAhead() {
    #expect(on("b").neighbours(in: feed) == ["thr_c", "thr_a"])
}

@Test func theFirstCardHasOneNeighbour() {
    #expect(on("a").neighbours(in: feed) == ["thr_b"])
}

private func detail(_ id: String) -> ThreadDetail { .placeholder(thread(id)) }

@Test func aCachedConversationIsShownAtOnce() {
    var cache = DetailCache(capacity: 2)
    cache.put(detail("a"), for: "a")
    #expect(cache.get("a") == detail("a"))
}

@Test func theOldestConversationIsDroppedPastCapacity() {
    var cache = DetailCache(capacity: 2)
    for id in ["a", "b", "c"] { cache.put(detail(id), for: id) }
    #expect(cache.get("a") == nil)
}

@Test func aChangedThreadIsDropped() {
    var cache = DetailCache(capacity: 2)
    cache.put(detail("a"), for: "a")
    cache.drop("a")
    #expect(cache.get("a") == nil)
}

@Test func aPlaceholderShowsTheSubjectWithoutMessages() {
    #expect(ThreadDetail.placeholder(thread("a")).messages.isEmpty)
}

// MARK: reading an Unread card

private func card(_ id: String, in section: String) -> FeedCard {
    FeedCard(id: id, kind: .reply, section: section, thread: thread("thr_\(id)"), title: "T")
}

/// Before: two Unread cards on top. After reading `u`, mermozd puts it in Today.
private let beforeReading = Feed(sections: [
    FeedSection(key: "unread", title: "Unread", cards: [card("u", in: "unread"), card("v", in: "unread")]),
    FeedSection(key: "today", title: "Today", cards: [card("a", in: "today")]),
])
private let afterReading = Feed(sections: [
    FeedSection(key: "unread", title: "Unread", cards: [card("v", in: "unread")]),
    FeedSection(key: "today", title: "Today", cards: [card("a", in: "today"), card("u", in: "today")]),
])

@Test func readingTheFocusedUnreadCardKeepsItInPlace() {
    let next = FeedHold.keep("u", from: beforeReading, in: afterReading)
    #expect(next.cards.map(\.id) == ["u", "v", "a"])
}

@Test func readingTheFocusedUnreadCardDoesNotMoveFocus() {
    let next = FeedHold.keep("u", from: beforeReading, in: afterReading)
    var nav = on("u")
    nav.move(by: 0, in: next)
    #expect(nav == on("u"))
}

@Test func theNextCardAfterAReadOneIsTheNextUnread() {
    let next = FeedHold.keep("u", from: beforeReading, in: afterReading)
    var nav = on("u")
    nav.move(by: 1, in: next)
    #expect(nav == on("v"))
}

@Test func aHeldCardStillSaysUnread() {
    let next = FeedHold.keep("u", from: beforeReading, in: afterReading)
    #expect(next.card("u")?.section == "unread")
}

@Test func onceFocusMovedOnTheReadCardTakesItsPlace() {
    let held = FeedHold.keep("u", from: beforeReading, in: afterReading)
    let next = FeedHold.keep("v", from: held, in: afterReading)
    #expect(next.cards.map(\.id) == ["v", "a", "u"])
}

@Test func theLastUnreadCardKeepsItsSectionWhileFocused() {
    let before = Feed(sections: [
        FeedSection(key: "unread", title: "Unread", cards: [card("u", in: "unread")]),
        FeedSection(key: "today", title: "Today", cards: [card("a", in: "today")]),
    ])
    let after = Feed(sections: [
        FeedSection(key: "today", title: "Today", cards: [card("u", in: "today"), card("a", in: "today")]),
    ])
    #expect(FeedHold.keep("u", from: before, in: after).sections.map(\.key) == ["unread", "today"])
}

@Test func aCardThatStaysInItsSectionIsNotHeld() {
    let next = FeedHold.keep("v", from: beforeReading, in: afterReading)
    #expect(next == afterReading)
}
