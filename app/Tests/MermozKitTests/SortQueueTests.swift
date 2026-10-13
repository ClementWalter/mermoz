/// The Sort queue's card flow: cards in hand, answers, skips, undo and plain-language text.
import Foundation
import Testing
@testable import MermozKit

private let en = Locale(identifier: "en_US")
private let names = ["archive": "Archive", "done": "Done"]

private func card(_ id: String) -> SortCard {
    SortCard(thread: ThreadSummary(id: id, number: "M-\(id)", accountId: "acc", subject: "Trip", snippet: "",
                                   participants: [], lastFrom: "Ana", lastDate: 0, unread: false, messageCount: 1,
                                   hasAttachments: false, status: "backlog", priority: 0,
                                   suggestion: Suggestion(status: "archive", projects: ["Voyage"])))
}

private func queue(_ ids: String...) -> SortQueue {
    var q = SortQueue()
    q.add(ids.map(card))
    return q
}

@Test func firstCardIsOnScreen() {
    #expect(queue("a", "b").current?.id == "a")
}

@Test func answeringShowsTheNextCard() {
    var q = queue("a", "b")
    q.answer()
    #expect(q.current?.id == "b")
}

@Test func skippingShowsTheNextCard() {
    var q = queue("a", "b")
    q.skip()
    #expect(q.current?.id == "b")
}

@Test func skippedCardIsNotHandedOutAgain() {
    var q = queue("a")
    q.skip()
    q.add([card("a")])
    #expect(q.current == nil)
}

@Test func answeredCardIsNotHandedOutAgain() {
    var q = queue("a")
    q.answer()
    q.add([card("a")])
    #expect(q.current == nil)
}

@Test func cardsInHandAreNotAddedTwice() {
    var q = queue("a")
    q.add([card("a"), card("b")])
    #expect(q.cards.map(\.id) == ["a", "b"])
}

@Test func excludeCoversHeldSkippedAndAnsweredCards() {
    var q = queue("a", "b", "c")
    q.answer()
    q.skip()
    #expect(Set(q.exclude) == ["a", "b", "c"])
}

@Test func undoPutsTheCardBackOnScreen() {
    var q = queue("a", "b")
    let a = q.answer()!
    q.putBack(a)
    #expect(q.current?.id == "a")
}

@Test func undoneCardCanBeAnsweredAgain() {
    var q = queue("a")
    let a = q.answer()!
    q.putBack(a)
    #expect(!q.answered.contains("a"))
}

@Test(arguments: [(2, true), (3, false)])
func moreCardsAreAskedForBelowTheLookahead(_ held: Int, _ want: Bool) {
    var q = SortQueue()
    q.add((0..<held).map { card(String($0)) })
    #expect(q.needsMore == want)
}

@Test func targetReadsInPlainWords() {
    #expect(SortText.target(status: "archive", projects: ["Travel/Voyage"], statusName: { names[$0] ?? $0 }) == "Archive · Voyage")
}

@Test(arguments: [
    ("credits", "Sorting paused: your TypeSafe credits ran out"),
    ("unavailable", "Sorting paused: the AI is busy, trying again soon"),
])
func pauseIsSaidInPlainWords(_ reason: String, _ want: String) {
    #expect(SortText.paused(reason) == want)
}

@Test func spentCreditsNameTheProvider() {
    #expect(SortText.paused("credits", provider: "SubQ") == "Sorting paused: your SubQ credits ran out")
}

@Test func routerStateReadsTheProviders() throws {
    let json = #"{"configured": true, "model": "subq-snap-preview", "provider": "subq", "provider_name": "SubQ", "providers": [{"id": "subq", "name": "SubQ", "model": "subq-snap-preview", "configured": true, "calls_today": 4}], "offer": null}"#
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    let r = try d.decode(RouterState.self, from: Data(json.utf8))
    #expect(r.providers?.first?.callsToday == 4)
}

@Test func routerStateFromAnOlderDaemonHasNoProviders() throws {
    let json = #"{"configured": true, "model": "jev-latest"}"#
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    let r = try d.decode(RouterState.self, from: Data(json.utf8))
    #expect(r.providers == nil)
}

@Test func noPauseSaysNothing() {
    #expect(SortText.paused(nil) == nil)
}

@Test func toSortCountsInTheUsersLocale() {
    #expect(SortText.toSort(1347, locale: en) == "1,347 to sort")
}

@Test(arguments: [
    (LearnedItem(sender: "ana@x.example", name: "Ana", status: nil, projects: ["Voyage"], kind: "rule", count: 3),
     "Mail from Ana goes to Voyage"),
    (LearnedItem(sender: "jl@x.example", name: "Jean", status: "done", projects: ["Mariage"], kind: "avoid", count: 1),
     "Won't suggest Done · Mariage for Jean again"),
])
func learnedReadsAsASentence(_ item: LearnedItem, _ want: String) {
    #expect(item.sentence { names[$0] ?? $0 } == want)
}

@Test func sortStateDecodesColumnCounts() throws {
    let json = Data(#"""
    {"to_sort":3,"by_status":[{"status":"in_progress","count":2},{"status":"backlog","count":1}],
     "configured":true,"paused":"credits","learned":[]}
    """#.utf8)
    #expect(try RPCCoding.decoder().decode(SortInfo.self, from: json).count("in_progress") == 2)
}

@Test func sortAnswerDecodesItsUndo() throws {
    let json = Data(#"""
    {"undo":{"thread":{"id":"thr_1","unread":false,"status":"backlog","priority":0,"projects":[],"due":null},"since":1791700000000},
     "learned":"Learned: mail from Ana goes to Voyage"}
    """#.utf8)
    #expect(try RPCCoding.decoder().decode(SortAnswerResult.self, from: json).undo.thread.status == "backlog")
}

@Test func undoTokenEncodesWhatTheDaemonReads() throws {
    let token = SortUndoToken(thread: ThreadState(card("a").thread), since: 5)
    let obj = try JSONSerialization.jsonObject(with: JSONEncoder().encode(token)) as? [String: Any]
    #expect(Set(obj?.keys.map { $0 } ?? []) == ["thread", "since"])
}
