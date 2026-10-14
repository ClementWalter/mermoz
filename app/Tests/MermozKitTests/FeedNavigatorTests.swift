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

// MARK: conversations

/// A conversation card: keyed by the conversation id (its first thread, `old`), shown
/// through its newest thread `new`.
private func conversationCard(_ conv: String, newest: String, in section: String = "now") -> FeedCard {
    FeedCard(id: conv, kind: .reply, section: section, thread: thread(newest), title: "T")
}

private let conversations = Feed(sections: [FeedSection(key: "now", title: "Now", cards: [
    card("a"), conversationCard("old", newest: "new"), card("c"),
])])

@Test func anArrowOntoAConversationFocusesItsId() {
    var nav = on("a")
    nav.move(by: 1, in: conversations)
    #expect(nav.focus == "old")
}

@Test func anArrowOntoAConversationShowsItsNewestThread() {
    var nav = on("a")
    nav.move(by: 1, in: conversations)
    #expect(nav.pane == "new")
}

@Test func archivingAConversationMovesFocusAndPaneTogether() {
    var nav = FeedNavigator(focus: "old", pane: "new")
    nav.leave("old", in: conversations)
    #expect(nav == on("c"))
}

@Test func undoOfAConversationFindsItFromAnyOfItsThreads() {
    var nav = on("c")
    nav.restore(["old", "mid", "new"], in: conversations)
    #expect(nav == FeedNavigator(focus: "old", pane: "new"))
}

@Test func undoOfAConversationFindsItByItsNewestThread() {
    var nav = on("c")
    nav.restore(["new"], in: conversations)
    #expect(nav.focus == "old")
}

@Test func aConversationsNeighboursAreFetchedByTheirThreads() {
    #expect(FeedNavigator(focus: "old", pane: "new").neighbours(in: conversations) == ["thr_c", "thr_a"])
}

@Test func aConversationThatJoinsAnotherKeepsTheFocus() {
    // The focused thread `t5` joined the conversation `t1`: its card has a new id.
    let after = Feed(sections: [FeedSection(key: "now", title: "Now", cards: [card("a"), conversationCard("t1", newest: "t5")])])
    var nav = FeedNavigator(focus: "t5", pane: "t5")
    nav.refocus(in: after)
    #expect(nav.focus == "t1")
}

@Test func aFocusThatIsGoneFallsBackToTheFirstCard() {
    var nav = FeedNavigator(focus: "gone", pane: nil)
    nav.refocus(in: conversations)
    #expect(nav.focus == "a")
}

@Test func aFocusStillThereIsKept() {
    var nav = FeedNavigator(focus: "old", pane: "new")
    nav.refocus(in: conversations)
    #expect(nav.focus == "old")
}

@Test func readingAFocusedUnreadConversationKeepsItInPlace() {
    let before = Feed(sections: [
        FeedSection(key: "unread", title: "Unread", cards: [conversationCard("old", newest: "new", in: "unread")]),
        FeedSection(key: "today", title: "Today", cards: [card("a", in: "today")]),
    ])
    let after = Feed(sections: [
        FeedSection(key: "today", title: "Today", cards: [card("a", in: "today"), conversationCard("old", newest: "new", in: "today")]),
    ])
    #expect(FeedHold.keep("old", from: before, in: after).cards.map(\.id) == ["old", "a"])
}

@Test func aConversationIsCachedUnderTheThreadInThePane() {
    var cache = DetailCache(capacity: 2)
    let nav = FeedNavigator(focus: "old", pane: "new")
    cache.put(detail("new"), for: nav.pane!)
    #expect(cache.get(FeedSelection.thread(of: conversationCard("old", newest: "new"))) == detail("new"))
}
