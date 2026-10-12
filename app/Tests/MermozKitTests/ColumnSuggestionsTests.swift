/// Column suggestion bars: protocol decoding, undo snapshots and plain-language text.
import Foundation
import Testing
@testable import MermozKit

private let en = Locale(identifier: "en_US")

private func column(_ status: String = "backlog", classified: Int = 1240, total: Int = 7256) -> ColumnSuggestions {
    ColumnSuggestions(status: status, pending: 3, groups: [], classified: classified, total: total)
}

@Test func summaryDecodesSnakeCase() throws {
    let json = Data(#"""
    {"columns":[{"status":"backlog","pending":3,"groups":[{"status":"archive","projects":["Receipts"],"count":3}],
     "classified":10,"total":12}],"sorting":true,"paused":null,"calls_today":42,"daily_cap":20000}
    """#.utf8)
    let s = try RPCCoding.decoder().decode(SuggestionsSummary.self, from: json)
    #expect(s.columns[0].groups[0] == SuggestionGroup(status: "archive", projects: ["Receipts"], count: 3))
}

@Test func summaryDecodesDailyCap() throws {
    let json = Data(#"{"columns":[],"sorting":false,"paused":"limit","calls_today":20000,"daily_cap":20000}"#.utf8)
    #expect(try RPCCoding.decoder().decode(SuggestionsSummary.self, from: json).dailyCap == 20000)
}

@Test func suggestionsChangedEventDecodes() {
    let raw = #"{"jsonrpc":"2.0","method":"event","params":{"kind":"suggestions_changed"}}"#
    guard case let .event(e) = decodeIncoming(Data(raw.utf8)) else {
        Issue.record("expected event")
        return
    }
    #expect(e == .suggestionsChanged)
}

@Test func threadStateEncodesProtocolKeys() throws {
    let t = ThreadSummary(id: "thr_1", number: "M-1", accountId: "acc", subject: "s", snippet: "", participants: [],
                          lastFrom: "", lastDate: 0, unread: false, messageCount: 1, hasAttachments: false,
                          status: "backlog", priority: 2, projects: ["Immo"], due: nil)
    let obj = try JSONSerialization.jsonObject(with: JSONEncoder().encode(ThreadState(t))) as? [String: Any]
    #expect(Set(obj?.keys.map { $0 } ?? []) == ["id", "unread", "status", "priority", "projects", "due"])
}

@Test(arguments: [
    (SuggestionGroup(status: "archive", projects: ["Receipts"], count: 1), "Archive · Receipts"),
    (SuggestionGroup(status: "todo", projects: [], count: 1), "Todo"),
    (SuggestionGroup(status: nil, projects: ["Immo/Houlgate"], count: 1), "Houlgate"),
])
func groupLabelUsesStatusNamesAndProjectLeaves(_ g: SuggestionGroup, _ want: String) {
    let names = ["archive": "Archive", "todo": "Todo"]
    #expect(g.label { names[$0] ?? $0 } == want)
}

@Test func progressCountsSortedThreads() {
    #expect(ColumnSortText.progress(column(), sorting: true, paused: nil, locale: en) == "Sorting… 1,240 / 7,256")
}

@Test func finishedColumnShowsNoProgress() {
    #expect(ColumnSortText.progress(column(classified: 12, total: 12), sorting: true, paused: nil) == nil)
}

@Test(arguments: ["unread", "done", "archive"])
func unsortedColumnsShowNoProgress(_ status: String) {
    #expect(ColumnSortText.progress(column(status), sorting: true, paused: nil) == nil)
}

@Test func progressIsHiddenWhenSortingIsOff() {
    #expect(ColumnSortText.progress(column(), sorting: false, paused: nil) == nil)
}

@Test(arguments: [("limit", "Sorting continues tomorrow"), ("unavailable", "Sorting paused, retrying soon")])
func pausedSortingSaysWhy(_ paused: String, _ want: String) {
    #expect(ColumnSortText.progress(column(), sorting: false, paused: paused) == want)
}

@Test(arguments: [(1, "1 suggestion"), (312, "312 suggestions"), (7256, "7,256 suggestions")])
func pendingCountReads(_ n: Int, _ want: String) {
    #expect(ColumnSortText.pending(n, locale: en) == want)
}

@Test func remainingNeverGoesNegative() {
    #expect(column(classified: 13, total: 12).remaining == 0)
}

// MARK: reviewing

private let names = ["archive": "Archive", "todo": "Todo"]

@Test(arguments: [
    SuggestionGroup(status: "archive", projects: ["Receipts"], count: 0),
    SuggestionGroup(status: nil, projects: ["Clients/Acme, Inc"], count: 0),
    SuggestionGroup(status: "todo", projects: [], count: 0),
])
func destinationSurvivesItsGroupKey(_ g: SuggestionGroup) {
    #expect(SuggestionGroup(id: g.id) == g)
}

@Test func suggestionTargetsItsColumnGroup() {
    let sg = Suggestion(status: "archive", projects: ["Receipts"])
    #expect(sg.target.id == SuggestionGroup(status: "archive", projects: ["Receipts"], count: 758).id)
}

@Test func reviewOfOneDestinationNamesIt() {
    let r = SuggestionReview(status: "backlog", group: SuggestionGroup(status: "archive", projects: [], count: 758))
    #expect(r.title(count: 758, statusName: { names[$0] ?? $0 }, locale: en) == "Reviewing → Archive · 758")
}

@Test func reviewOfEverythingCountsSuggestions() {
    let r = SuggestionReview(status: "backlog")
    #expect(r.title(count: 2693, statusName: { $0 }, locale: en) == "Reviewing 2,693 suggestions")
}

@Test(arguments: [
    (SuggestionReview(status: "backlog", group: SuggestionGroup(status: "archive", projects: [], count: 0)), "Accept these 758"),
    (SuggestionReview(status: "backlog"), "Accept all 758"),
])
func reviewAcceptSaysWhatItCovers(_ r: SuggestionReview, _ want: String) {
    #expect(r.acceptLabel(count: 758, locale: en) == want)
}

private func suggested(_ id: String, _ sg: Suggestion?) -> ThreadSummary {
    ThreadSummary(id: id, number: "M-\(id)", accountId: "acc", subject: "s", snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: false, messageCount: 1, hasAttachments: false, status: "backlog", priority: 0,
                  suggestion: sg)
}

@Test func threadsGroupByDestination() {
    let threads = [suggested("1", Suggestion(status: "archive")), suggested("2", Suggestion(status: "todo")),
                   suggested("3", Suggestion(status: "archive"))]
    let groups = Grouper.group(threads, by: .suggestion) { _ in ["archive|", "todo|"] }
    #expect(groups.map { $0.threads.map(\.id) } == [["1", "3"], ["2"]])
}

@Test func displayMenuDoesNotOfferSuggestionGrouping() {
    #expect(!Grouping.menuCases.contains(.suggestion))
}
