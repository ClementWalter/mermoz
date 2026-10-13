/// The List drawn by an AppKit table behaves as the SwiftUI list it replaces: rows as
/// tall as they draw, pinned group headers, clicks with ⇧/⌘, ⇧↑/⇧↓, the context menu,
/// the drag payload, paging, the ✦ chip, and the selection always in view, scrolled in
/// the same update as it moves. Driven in-process in an offscreen window.
import AppKit
import MermozKit
import SwiftUI
import Testing
@testable import Mermoz

private func thread(_ i: Int, priority: Int = 0, unread: Bool = false, projects: [String] = [], due: String? = nil,
                    suggestion: Suggestion? = nil, nextAction: String? = nil, attachments: Bool = false) -> ThreadSummary {
    ThreadSummary(id: "t\(i)", number: "M-\(i)", accountId: "a", subject: "Subject \(i)", snippet: "Snippet \(i)",
                  participants: [], lastFrom: "Sender \(i)", lastDate: Int64(1_000_000 - i), unread: unread, messageCount: 1,
                  hasAttachments: attachments, status: "todo", priority: priority, projects: projects, due: due,
                  nextAction: nextAction, suggestion: suggestion)
}

private func threads(_ n: Int, priority: (Int) -> Int = { _ in 0 }) -> [ThreadSummary] {
    (0..<n).map { thread($0, priority: priority($0)) }
}

/// The List in an offscreen window, with the model it shows.
@MainActor
private final class List {
    let model = AppModel()
    let window: NSWindow
    let host: NSHostingView<AnyView>

    init(_ threads: [ThreadSummary], grouping: Grouping = .none, total: Int? = nil, height: CGFloat = 400) {
        model.layout = .list
        model.listDisplay = DisplayOptions(grouping: grouping)
        model.threads = threads
        model.totalThreads = total ?? threads.count
        host = NSHostingView(rootView: AnyView(ThreadListView().environment(model)))
        window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 420, height: height), styleMask: .borderless,
                          backing: .buffered, defer: false)
        window.contentView = host
        settle()
    }

    func settle() {
        host.layoutSubtreeIfNeeded()
        table.layoutSubtreeIfNeeded()
        // A window never ordered on screen builds no rows by itself: build the visible ones.
        for r in 0..<min(table.numberOfRows, 8) { _ = table.view(atColumn: 0, row: r, makeIfNecessary: true) }
    }

    /// Lets the rows check their height once drawn (on the next turns of the main queue).
    func settled() async -> List {
        try? await Task.sleep(for: .milliseconds(30))
        table.layoutSubtreeIfNeeded()
        return self
    }

    var table: PassiveTableView {
        func find(_ v: NSView) -> PassiveTableView? {
            if let t = v as? PassiveTableView { return t }
            for s in v.subviews { if let t = find(s) { return t } }
            return nil
        }
        return find(host)!
    }

    var coordinator: ThreadTable.Coordinator { table.delegate as! ThreadTable.Coordinator }

    func row(of id: String) -> Int { ListItems.row(of: id, in: coordinator.items)! }

    /// The selected row lies wholly in the visible area, below any pinned header.
    var selectionShown: Bool {
        guard let id = model.selectedThreadId, let clip = table.enclosingScrollView?.contentView else { return false }
        var visible = clip.bounds
        if let header = coordinator.items.first(where: { if case .group = $0 { true } else { false } }) {
            let h = coordinator.height(of: header)
            visible = CGRect(x: visible.minX, y: visible.minY + h, width: visible.width, height: visible.height - h)
        }
        return visible.contains(table.rect(ofRow: row(of: id)))
    }

    /// The SwiftUI row hosted in a table row, laid out.
    func rowHost(_ row: Int) -> NSView? {
        (table.view(atColumn: 0, row: row, makeIfNecessary: true) as? HostCell)?.host
    }
}

// MARK: row heights

private let variants: [(String, ThreadSummary)] = [
    ("plain", thread(1)),
    ("unread", thread(1, unread: true)),
    ("projects", thread(1, projects: ["Home", "Travel", "Kids"])),
    ("due", thread(1, due: "2026-11-02")),
    ("suggestion", thread(1, suggestion: Suggestion(status: "done", projects: ["Home"]))),
    ("next action", thread(1, nextAction: "Reply to Nora about the keys")),
    ("attachments", thread(1, attachments: true)),
]

@MainActor @Test(arguments: variants.map(\.0))
func aRowIsAsTallAsItDraws(variant: String) async {
    let t = variants.first { $0.0 == variant }!.1
    let list = await List([t]).settled()
    #expect(ThreadTable.Coordinator.fitting(ThreadRow(thread: t), model: list.model)
        == list.table.rect(ofRow: list.row(of: t.id)).height)
}

// MARK: group headers

@MainActor @Test func groupsGetAHeaderRow() async {
    let list = await List(threads(6) { $0 < 3 ? 1 : 2 }, grouping: .priority).settled()
    #expect(list.coordinator.items.filter { if case .group = $0 { true } else { false } }.count == 2)
}

@MainActor @Test func aGroupHeaderIsAGroupRow() async {
    let list = await List(threads(6) { $0 < 3 ? 1 : 2 }, grouping: .priority).settled()
    #expect(list.coordinator.tableView(list.table, isGroupRow: 0))
}

@MainActor @Test func groupHeadersStayPinned() async {
    let list = await List(threads(6) { $0 < 3 ? 1 : 2 }, grouping: .priority).settled()
    #expect(list.table.floatsGroupRows)
}

@MainActor @Test func aGroupHeaderIsAsTallAsItDraws() async {
    let list = await List(threads(6) { $0 < 3 ? 1 : 2 }, grouping: .priority).settled()
    guard case let .group(g) = list.coordinator.items[0] else { Issue.record("no header"); return }
    #expect(ThreadTable.Coordinator.fitting(GroupHeaderView(group: g), model: list.model) == list.table.rect(ofRow: 0).height)
}

@MainActor @Test func ungroupedListsHaveNoHeaderRows() async {
    let list = await List(threads(6)).settled()
    #expect(list.coordinator.items.allSatisfy { if case .thread = $0 { true } else { false } })
}

// MARK: clicks

@MainActor @Test func aClickOpensTheThread() async {
    let list = await List(threads(10)).settled()
    list.coordinator.modifiers = { [] }
    list.coordinator.click("t3")
    #expect(list.model.selectedThreadId == "t3")
}

@MainActor @Test func aShiftClickSelectsTheRange() async {
    let list = await List(threads(10)).settled()
    list.coordinator.modifiers = { [] }
    list.coordinator.click("t2")
    list.coordinator.modifiers = { .shift }
    list.coordinator.click("t5")
    #expect(list.model.multiSelection == ["t2", "t3", "t4", "t5"])
}

@MainActor @Test func aCommandClickAddsOneThread() async {
    let list = await List(threads(10)).settled()
    list.coordinator.modifiers = { [] }
    list.coordinator.click("t2")
    list.coordinator.modifiers = { .command }
    list.coordinator.click("t7")
    #expect(Set(list.model.multiSelection) == ["t2", "t7"])
}

// MARK: keyboard

@MainActor @Test func shiftDownExtendsTheSelection() async {
    let list = await List(threads(10)).settled()
    list.model.open("t2")
    list.model.moveSelection(1, extend: true)
    #expect(list.model.multiSelection == ["t2", "t3"])
}

@MainActor @Test func shiftUpExtendsTheSelectionUpwards() async {
    let list = await List(threads(10)).settled()
    list.model.open("t5")
    list.model.moveSelection(-1, extend: true)
    #expect(Set(list.model.multiSelection) == ["t4", "t5"])
}

@MainActor @Test func jStepsDownWhereverTheFocusIs() async {
    let list = await List(threads(10)).settled()
    list.model.open("t2")
    _ = ThreadKeys.handle("j", model: list.model)
    #expect(list.model.selectedThreadId == "t3")
}

@MainActor @Test func theTableNeverTakesTheKeyboard() async {
    let list = await List(threads(3)).settled()
    #expect(!list.table.acceptsFirstResponder)
}

// MARK: selection in view

@MainActor @Test func steppingPastTheBottomKeepsTheSelectionShown() async {
    let list = await List(threads(60), height: 300).settled()
    list.model.open("t0")
    for _ in 0..<20 { list.model.moveSelection(1) }
    #expect(list.selectionShown)
}

@MainActor @Test func steppingBackUpKeepsTheSelectionShown() async {
    let list = await List(threads(60), height: 300).settled()
    list.model.open("t40")
    for _ in 0..<15 { list.model.moveSelection(-1) }
    #expect(list.selectionShown)
}

@MainActor @Test func steppingUpUnderAPinnedHeaderKeepsTheSelectionShown() async {
    let list = await List(threads(60) { $0 < 30 ? 1 : 2 }, grouping: .priority, height: 300).settled()
    list.model.open("t45")
    for _ in 0..<12 { list.model.moveSelection(-1) }
    #expect(list.selectionShown)
}

@MainActor @Test func aJumpFarDownShowsTheSelection() async {
    let list = await List(threads(200), height: 300).settled()
    list.model.open("t180")
    #expect(list.selectionShown)
}

@MainActor @Test func theScrollHappensInTheSameUpdateAsTheSelection() async {
    let list = await List(threads(60), height: 300).settled()
    list.model.open("t0")
    let before = list.table.enclosingScrollView!.contentView.bounds.minY
    list.model.open("t30")
    // No layout pass and no turn of the run loop in between.
    #expect(list.table.enclosingScrollView!.contentView.bounds.minY > before)
}

// MARK: context menu and drag

@MainActor @Test func aRowHasTheConversationMenu() async {
    let list = await List(threads(5)).settled()
    guard let rowHost = list.rowHost(list.row(of: "t1")) else { Issue.record("no row"); return }
    let event = NSEvent.mouseEvent(with: .rightMouseDown, location: rowHost.convert(CGPoint(x: 40, y: 20), to: nil),
                                   modifierFlags: [], timestamp: 0, windowNumber: list.window.windowNumber, context: nil,
                                   eventNumber: 0, clickCount: 1, pressure: 1)!
    #expect(rowHost.menu(for: event)?.items.map(\.title).contains("Join into one conversation (⇧M)") == true)
}

@MainActor @Test func aDraggedRowCarriesItsThread() async {
    let list = await List(threads(5)).settled()
    #expect(list.model.dragPayload("t2") == "t2")
}

@MainActor @Test func aDraggedSelectedRowCarriesTheWholeSelection() async {
    let list = await List(threads(5)).settled()
    list.coordinator.modifiers = { [] }
    list.coordinator.click("t1")
    list.coordinator.modifiers = { .shift }
    list.coordinator.click("t3")
    #expect(list.model.dragPayload("t2") == "t1\nt2\nt3")
}

// MARK: paging

@MainActor @Test func aListWithMorePagesEndsWithALoadingRow() async {
    let list = await List(threads(5), total: 50).settled()
    #expect(list.coordinator.items.last == .loadMore(loaded: 5, total: 50))
}

@MainActor @Test func reachingTheLoadingRowAsksForTheNextPage() async {
    let list = await List(threads(40), total: 200, height: 300).settled()
    var asked = 0
    list.coordinator.loadMore = { asked += 1 }
    list.table.scrollRowToVisible(list.coordinator.items.count - 1)
    list.settle()
    #expect(asked > 0)
}

@MainActor @Test func aFullListHasNoLoadingRow() async {
    let list = await List(threads(5)).settled()
    #expect(list.coordinator.items.last.flatMap(\.threadId) == "t4")
}

// MARK: chips and accessibility

/// The pixels a table row draws.
@MainActor
private func drawn(_ list: List, row: Int) -> Data? {
    guard let v = list.rowHost(row) else { return nil }
    v.layoutSubtreeIfNeeded()
    guard let rep = v.bitmapImageRepForCachingDisplay(in: v.bounds) else { return nil }
    v.cacheDisplay(in: v.bounds, to: rep)
    return rep.tiffRepresentation
}

@MainActor @Test func aSuggestedThreadDrawsItsChip() async {
    let plain = await List([thread(1)]).settled()
    let suggested = await List([thread(1, suggestion: Suggestion(status: "done", projects: ["Home"]))]).settled()
    #expect(drawn(suggested, row: 0) != drawn(plain, row: 0))
}

@MainActor @Test func aThreadDueSoonDrawsItsDate() async {
    let plain = await List([thread(1)]).settled()
    let due = await List([thread(1, due: "2026-11-02")]).settled()
    #expect(drawn(due, row: 0) != drawn(plain, row: 0))
}

@MainActor @Test func aRowThatGainsAChipGrows() async {
    let list = await List([thread(1), thread(2)]).settled()
    let before = list.table.rect(ofRow: 0).height
    list.model.threads[0].projects = ["Home"]
    list.host.layoutSubtreeIfNeeded()
    #expect(list.table.rect(ofRow: 0).height > before)
}

@MainActor @Test func aRowIsNamedForVoiceOver() async {
    let list = await List([thread(1, unread: true)]).settled()
    let row = list.table.rowView(atRow: 0, makeIfNecessary: true)
    #expect(row?.accessibilityLabel() == "Unread, Sender 1, Subject 1")
}

@MainActor @Test func aSuggestedRowSaysSo() async {
    let label = ThreadTable.Coordinator.accessibilityLabel(.thread(thread(1, suggestion: Suggestion(status: "done")), key: "k"))
    #expect(label.hasSuffix("AI suggestion"))
}

// MARK: other layouts

@MainActor @Test func theBoardHasNoTable() async {
    let list = await List(threads(5)).settled()
    list.model.layout = .board
    list.host.rootView = AnyView(BoardView().environment(list.model))
    list.host.layoutSubtreeIfNeeded()
    func has(_ v: NSView) -> Bool { v is PassiveTableView || v.subviews.contains(where: has) }
    #expect(!has(list.host))
}

@MainActor @Test func leavingTheListStopsItsScrolling() async {
    let list = await List(threads(5)).settled()
    list.host.rootView = AnyView(EmptyView())
    list.host.layoutSubtreeIfNeeded()
    #expect(list.model.revealSelection == nil)
}
