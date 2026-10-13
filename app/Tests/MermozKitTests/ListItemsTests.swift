/// The List as table rows: headers, threads and the paging row in display order, and how
/// a table catches up with a new list (nothing, some rows, or everything).
import Foundation
import Testing
@testable import MermozKit

private func thread(_ id: String, unread: Bool = false) -> ThreadSummary {
    ThreadSummary(id: id, number: id, accountId: "a", subject: "S", snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: unread, messageCount: 1, hasAttachments: false, status: "todo", priority: 0)
}

private func group(_ key: String, _ ids: [String], field: Grouping = .priority) -> ThreadGroup {
    ThreadGroup(field: field, key: key, threads: ids.map { thread($0) }, subgroups: [])
}

@Test func aGroupedListStartsEachGroupWithItsHeader() {
    let items = ListItems.flatten([group("1", ["a", "b"]), group("2", ["c"])], loaded: 3, total: 3)
    #expect(items.map(\.key) == ["g|" + group("1", []).id, "\(group("1", []).id)|a", "\(group("1", []).id)|b",
                                 "g|" + group("2", []).id, "\(group("2", []).id)|c"])
}

@Test func anUngroupedListHasNoHeader() {
    let items = ListItems.flatten([group("", ["a", "b"], field: .none)], loaded: 2, total: 2)
    #expect(items.compactMap(\.threadId) == ["a", "b"])
}

@Test func moreToLoadEndsWithThePagingRow() {
    #expect(ListItems.flatten([group("", ["a"], field: .none)], loaded: 1, total: 9).last == .loadMore(loaded: 1, total: 9))
}

@Test func aThreadIsFoundByItsRow() {
    let items = ListItems.flatten([group("1", ["a", "b"])], loaded: 2, total: 2)
    #expect(ListItems.row(of: "b", in: items) == 2)
}

@Test func theSameListChangesNothing() {
    let items = ListItems.flatten([group("", ["a", "b"], field: .none)], loaded: 2, total: 2)
    #expect(ListItems.change(from: items, to: items) == .none)
}

@Test func aThreadReadInPlaceReloadsOnlyItsRow() {
    let before = ListItems.flatten([ThreadGroup(field: .none, key: "", threads: [thread("a", unread: true), thread("b")], subgroups: [])],
                                   loaded: 2, total: 2)
    let after = ListItems.flatten([ThreadGroup(field: .none, key: "", threads: [thread("a"), thread("b")], subgroups: [])], loaded: 2, total: 2)
    #expect(ListItems.change(from: before, to: after) == .rows([0]))
}

@Test func aThreadLeavingTheListReloadsEverything() {
    let before = ListItems.flatten([group("", ["a", "b"], field: .none)], loaded: 2, total: 2)
    let after = ListItems.flatten([group("", ["b"], field: .none)], loaded: 1, total: 1)
    #expect(ListItems.change(from: before, to: after) == .all)
}
