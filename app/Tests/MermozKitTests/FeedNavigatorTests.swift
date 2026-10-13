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
