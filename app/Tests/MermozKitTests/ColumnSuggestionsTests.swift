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
