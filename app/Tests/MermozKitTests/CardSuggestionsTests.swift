/// Suggestion chips on Feed cards: which are shown, which threads are asked about, and
/// what y and n do to the chip, the focus, the reading pane and the daemon's learning.
import Foundation
import Testing
@testable import MermozKit

private let names = ["archive": "Archive", "done": "Done"]

private func thread(_ id: String, lastDate: Int64 = 0) -> ThreadSummary {
    ThreadSummary(id: id, number: "M-1", accountId: "acc", subject: "Trip", snippet: "", participants: [],
                  lastFrom: "Ana", lastDate: lastDate, unread: false, messageCount: 1, hasAttachments: false,
                  status: "todo", priority: 0)
}

private func card(_ id: String, lastDate: Int64 = 0) -> FeedCard {
    FeedCard(id: id, kind: .reply, section: "now", thread: thread("thr_\(id)", lastDate: lastDate), title: "T")
}

private let batch = FeedCard(id: "batch:cleanup", kind: .cleanup, section: "later", thread: thread("thr_m1"),
                             members: [thread("thr_m1"), thread("thr_m2")], title: "Automatic mail")

private func feed(_ cards: [FeedCard]) -> Feed {
    Feed(sections: [FeedSection(key: "now", title: "Now", cards: cards)])
}

private let three = feed([card("a"), card("b"), card("c")])
private let ten = feed((0..<10).map { card("\($0)") })

private func sg(_ status: String? = "archive", _ statusConf: Double? = 0.9, projects: [String] = [],
                _ projectConf: Double? = nil) -> Suggestion {
    var s = Suggestion(status: status, projects: projects)
    s.statusConfidence = statusConf
    s.projectConfidence = projectConf
    return s
}

private func archive(_ thread: String) -> ThreadSuggestion { ThreadSuggestion(id: thread, suggestion: sg()) }

/// Chips for every card of `f` that has one in `batch`.
private func showing(_ batch: [ThreadSuggestion], in f: Feed = three) -> FeedSuggestions {
    var s = FeedSuggestions()
    s.received(batch, in: f)
    return s
}

private func on(_ id: String) -> FeedNavigator { FeedNavigator(focus: id, pane: "thr_\(id)") }

// MARK: which chips show

@Test func aConfidentSuggestionShowsAsAChip() {
    #expect(showing([archive("thr_a")]).chip(for: card("a")) == archive("thr_a"))
}

@Test(arguments: [
    sg("archive", 0.5),
    sg(nil, nil, projects: ["Voyage"], 0.6),
    sg("done", 0.9, projects: ["Voyage"], 0.4),
    sg(nil, nil),
])
func aDoubtfulOrEmptyGuessShowsNoChip(_ guess: Suggestion) {
    #expect(showing([ThreadSuggestion(id: "thr_a", suggestion: guess)]).chip(for: card("a")) == nil)
}

@Test func aCardWithoutASuggestionHasNoChip() {
    #expect(showing([archive("thr_a")]).chip(for: card("b")) == nil)
}

@Test func aBatchCardHasNoChip() {
    #expect(showing([archive("thr_m1")], in: feed([batch])).chip(for: batch) == nil)
}

@Test(arguments: [
    (sg("archive"), "Archive"),
    (sg(nil, nil, projects: ["Travel/Voyage"], 0.9), "→ Voyage"),
    (sg("done", 0.9, projects: ["Voyage"], 0.9), "Done · Voyage"),
])
func theChipSaysWhereItGoes(_ s: Suggestion, _ want: String) {
    #expect(SuggestionText.chip(s) { names[$0] ?? $0 } == want)
}

// MARK: asked just in time, never in bulk

@Test func theFocusedCardAndTheNextThreeAreAskedAbout() {
    #expect(FeedSuggestions().wanted(in: ten, visible: [], focus: "4") == ["thr_4", "thr_5", "thr_6", "thr_7"])
}

@Test func cardsOnScreenAreAskedAboutToo() {
    #expect(FeedSuggestions().wanted(in: ten, visible: ["9"], focus: "0") == ["thr_0", "thr_1", "thr_2", "thr_3", "thr_9"])
}

@Test func oneRequestCoversAFewThreadsOnly() {
    let all = Set((0..<10).map { "\($0)" })
    #expect(FeedSuggestions().wanted(in: ten, visible: all, focus: "0").count == FeedSuggestions.perRequest)
}

@Test func batchCardsAreNotAskedAbout() {
    #expect(FeedSuggestions().wanted(in: feed([batch]), visible: ["batch:cleanup"], focus: nil).isEmpty)
}

@Test func anAskedThreadIsNotAskedAgain() {
    var s = FeedSuggestions()
    s.asking(["thr_a"], in: three)
    #expect(s.wanted(in: three, visible: [], focus: "a") == ["thr_b", "thr_c"])
}

@Test func newMailAsksAgain() {
    var s = FeedSuggestions()
    s.asking(["thr_a"], in: three)
    #expect(s.wanted(in: feed([card("a", lastDate: 9)]), visible: [], focus: "a") == ["thr_a"])
}

@Test func aFailedRequestIsAskedAgain() {
    var s = FeedSuggestions()
    s.asking(["thr_a"], in: three)
    s.failed(["thr_a"], in: three)
    #expect(s.wanted(in: three, visible: [], focus: "a") == ["thr_a", "thr_b", "thr_c"])
}

// MARK: y accepts, n dismisses

@Test func acceptingMovesTheFocusToTheNextCard() {
    var s = showing([archive("thr_a")])
    var nav = on("a")
    _ = s.accept(card("a"), nav: &nav, in: three)
    #expect(nav.focus == "b")
}

@Test func theReadingPaneFollowsAnAcceptedChip() {
    var s = showing([archive("thr_a")])
    var nav = on("a")
    _ = s.accept(card("a"), nav: &nav, in: three)
    #expect(nav.pane == "thr_b")
}

@Test func anAcceptedChipLeavesTheCard() {
    var s = showing([archive("thr_a")])
    var nav = on("a")
    _ = s.accept(card("a"), nav: &nav, in: three)
    #expect(s.chip(for: card("a")) == nil)
}

@Test func acceptingWithoutAChipMovesNothing() {
    var s = FeedSuggestions()
    var nav = on("a")
    _ = s.accept(card("a"), nav: &nav, in: three)
    #expect(nav == on("a"))
}

@Test func aDismissedChipLeavesTheCard() {
    var s = showing([archive("thr_a")])
    _ = s.dismiss(card("a"))
    #expect(s.chip(for: card("a")) == nil)
}

@Test func dismissingLeavesTheOtherChips() {
    var s = showing([archive("thr_a"), archive("thr_b")])
    _ = s.dismiss(card("a"))
    #expect(s.chip(for: card("b")) != nil)
}

@Test func undoPutsTheChipBack() {
    var s = showing([archive("thr_a")])
    let taken = s.dismiss(card("a"))!
    s.putBack(taken, lastDate: 0)
    #expect(s.chip(for: card("a")) == taken)
}

@Test(arguments: [("y", FeedKeyAction.accept), ("n", FeedKeyAction.dismiss)])
func yAndNAnswerTheChip(_ key: String, _ want: FeedKeyAction) {
    #expect(FeedKeyMap.action(key) == want)
}

// MARK: a reloaded feed

@Test func newMailRetiresTheChip() {
    var s = showing([archive("thr_a")])
    s.sync(with: feed([card("a", lastDate: 9)]))
    #expect(s.chip(for: card("a", lastDate: 9)) == nil)
}

@Test func aThreadGoneFromTheFeedLosesItsChip() {
    var s = showing([archive("thr_a")])
    s.sync(with: feed([card("b")]))
    #expect(s.shown.isEmpty)
}

@Test func anUnchangedThreadKeepsItsChip() {
    var s = showing([archive("thr_a")])
    s.sync(with: three)
    #expect(s.chip(for: card("a")) != nil)
}

// MARK: with the daemon

private let suggestReply = #"{"suggestions":[{"id":"thr_a","suggestion":{"status":"archive","status_confidence":0.9,"projects":[]},"why":null}],"paused":null,"provider_name":"TypeSafe","offer":null}"#
private let answerReply = #"{"undo":{"thread":{"id":"thr_a","unread":false,"status":"todo","priority":0,"projects":[],"due":null},"since":5},"learned":null}"#

/// A daemon that suggests Archive for thr_a and records answers; `failAnswers` refuses them.
private func daemon(failAnswers: Bool = false) throws -> StubDaemon {
    try StubDaemon { id, method in
        switch method {
        case "threads.suggest": #"{"jsonrpc":"2.0","id":\#(id),"result":\#(suggestReply)}"#
        case "sort.answer" where failAnswers: #"{"jsonrpc":"2.0","id":\#(id),"error":{"code":-32000,"message":"busy"}}"#
        case "sort.answer": #"{"jsonrpc":"2.0","id":\#(id),"result":\#(answerReply)}"#
        default: #"{"jsonrpc":"2.0","id":\#(id),"result":{}}"#
        }
    }
}

/// The params of each request for `method` the stub received.
private func params(_ stub: StubDaemon, _ method: String) -> [[String: Any]] {
    stub.requests.compactMap { line in
        guard let obj = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any],
              obj["method"] as? String == method else { return nil }
        return obj["params"] as? [String: Any] ?? [:]
    }
}

@MainActor
private func suggester(_ stub: StubDaemon) async throws -> (FeedSuggester, DaemonClient) {
    let client = DaemonClient(connection: try Connection(path: stub.path))
    let s = FeedSuggester()
    try await s.refresh(client: client, feed: three, visible: [], focus: "a")
    return (s, client)
}

@MainActor
@Test func theDaemonsConfidentSuggestionShowsOnItsCard() async throws {
    let stub = try daemon()
    defer { withExtendedLifetime(stub) {} }
    let (s, _) = try await suggester(stub)
    #expect(s.state.chip(for: card("a"))?.suggestion.status == "archive")
}

@MainActor
@Test func theDaemonIsAskedAboutTheFocusedCardAndTheNextOnes() async throws {
    let stub = try daemon()
    defer { withExtendedLifetime(stub) {} }
    _ = try await suggester(stub)
    #expect(params(stub, "threads.suggest").first?["ids"] as? [String] == ["thr_a", "thr_b", "thr_c"])
}

@MainActor
@Test func acceptingTeachesTheDaemonAYes() async throws {
    let stub = try daemon()
    defer { withExtendedLifetime(stub) {} }
    let (s, client) = try await suggester(stub)
    var nav = on("a")
    let taken = s.accept(card("a"), nav: &nav, in: three)!
    _ = try await s.send("yes", taken, of: card("a"), client: client)
    #expect(params(stub, "sort.answer").first?["answer"] as? String == "yes")
}

@MainActor
@Test func dismissingTeachesTheDaemonANo() async throws {
    let stub = try daemon()
    defer { withExtendedLifetime(stub) {} }
    let (s, client) = try await suggester(stub)
    let taken = s.dismiss(card("a"))!
    _ = try await s.send("no", taken, of: card("a"), client: client)
    #expect(params(stub, "sort.answer").first?["answer"] as? String == "no")
}

@MainActor
@Test func theAnswerNamesTheThread() async throws {
    let stub = try daemon()
    defer { withExtendedLifetime(stub) {} }
    let (s, client) = try await suggester(stub)
    let taken = s.dismiss(card("a"))!
    _ = try await s.send("no", taken, of: card("a"), client: client)
    #expect(params(stub, "sort.answer").first?["id"] as? String == "thr_a")
}

@MainActor
@Test func anAnswerThatDoesNotLandBringsTheChipBack() async throws {
    let stub = try daemon(failAnswers: true)
    defer { withExtendedLifetime(stub) {} }
    let (s, client) = try await suggester(stub)
    let taken = s.dismiss(card("a"))!
    _ = try? await s.send("no", taken, of: card("a"), client: client)
    #expect(s.state.chip(for: card("a")) != nil)
}

// MARK: wire format and plain words

@Test func anAnswerDecodesItsUndo() throws {
    #expect(try RPCCoding.decoder().decode(AnswerResult.self, from: Data(answerReply.utf8)).undo.thread.status == "todo")
}

@Test func undoTokenEncodesWhatTheDaemonReads() throws {
    let token = SuggestionUndo(thread: ThreadState(thread("a")), since: 5)
    let obj = try JSONSerialization.jsonObject(with: JSONEncoder().encode(token)) as? [String: Any]
    #expect(Set(obj?.keys.map { $0 } ?? []) == ["thread", "since"])
}

@Test func targetReadsInPlainWords() {
    #expect(SuggestionText.target(status: "archive", projects: ["Travel/Voyage"], statusName: { names[$0] ?? $0 }) == "Archive · Voyage")
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

@Test func aLearnedItemEncodesWhatForgetReads() throws {
    let item = LearnedItem(sender: "ana@x.example", name: "Ana", status: nil, projects: ["Voyage"], kind: "rule", count: 3)
    let obj = try JSONSerialization.jsonObject(with: JSONEncoder().encode(item)) as? [String: Any]
    #expect(Set(obj?.keys.map { $0 } ?? []) == ["sender", "name", "projects", "kind", "count"])
}

@Test func routerStateReadsTheProviders() throws {
    let json = #"{"configured": true, "model": "subq-snap-preview", "provider": "subq", "provider_name": "SubQ", "providers": [{"id": "subq", "name": "SubQ", "model": "subq-snap-preview", "configured": true, "calls_today": 4}], "offer": null}"#
    let r = try RPCCoding.decoder().decode(RouterState.self, from: Data(json.utf8))
    #expect(r.providers?.first?.callsToday == 4)
}

@Test func routerStateFromAnOlderDaemonHasNoProviders() throws {
    let json = #"{"configured": true, "model": "jev-latest"}"#
    let r = try RPCCoding.decoder().decode(RouterState.self, from: Data(json.utf8))
    #expect(r.providers == nil)
}
