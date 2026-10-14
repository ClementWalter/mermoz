/// Dragging rows and cards onto another one merges their conversations: the drop
/// handlers in-process against a stub daemon, the Board's card and column hit zones,
/// and ⌘Z splitting them back. A real drag starts a window-server session, so the
/// drops here are handed to the views as AppKit hands them a drop.
import AppKit
import MermozKit
import SwiftUI
import Testing
@testable import Mermoz

/// Stands in for the daemon: records each merge and unmerge.
private final class StubConversations: ConversationBackend, @unchecked Sendable {
    private let lock = NSLock()
    private var _merged: [[String]] = []
    private var _unmerged: [MergeUndo] = []
    let undo = MergeUndo(threads: [MergedThread(id: "t1", conversation: "t1", state: "done")])

    var merged: [[String]] { lock.withLock { _merged } }
    var unmerged: [MergeUndo] { lock.withLock { _unmerged } }

    func mergeConversations(_ threadIds: [String]) async throws -> MergeResult {
        lock.withLock { _merged.append(threadIds) }
        return MergeResult(conversationId: threadIds.last!, members: threadIds, undo: undo)
    }

    func unmergeConversations(_ undo: MergeUndo) async throws {
        lock.withLock { _unmerged.append(undo) }
    }
}

private func thread(_ i: Int, status: String = "todo") -> ThreadSummary {
    ThreadSummary(id: "t\(i)", number: "M-\(i)", accountId: "a", subject: "Subject \(i)", snippet: "Snippet \(i)",
                  participants: [], lastFrom: "Sender \(i)", lastDate: Int64(1_000_000 - i), unread: false, messageCount: 1,
                  hasAttachments: false, status: status, priority: 0, projects: [], due: nil)
}

@MainActor
private func model(_ n: Int = 4) -> (AppModel, StubConversations) {
    let m = AppModel()
    let stub = StubConversations()
    m.conversationsStub = stub
    m.threads = (0..<n).map { thread($0) }
    m.totalThreads = n
    return (m, stub)
}

// MARK: drop handling

@MainActor @Test func droppingARowOnAnotherMergesBoth() async {
    let (m, stub) = model()
    await m.dropMerge(["t2"], onto: "t1")?.value
    #expect(stub.merged == [["t2", "t1"]])
}

@MainActor @Test func droppingASelectionOnACardMergesAllIntoIt() async {
    let (m, stub) = model()
    await m.dropMerge(["t1\nt2\nt3"], onto: "t0")?.value
    #expect(stub.merged == [["t1", "t2", "t3", "t0"]])
}

@MainActor @Test func droppingACardOnItselfIsRefused() {
    let (m, _) = model()
    #expect(m.dropMerge(["t1"], onto: "t1") == nil)
}

@MainActor @Test func droppingACardOnItselfMergesNothing() async {
    let (m, stub) = model()
    await m.dropMerge(["t1"], onto: "t1")?.value
    #expect(stub.merged.isEmpty)
}

@MainActor @Test func droppingASelectionOnOneOfItsOwnCardsIsRefused() {
    let (m, _) = model()
    #expect(m.dropMerge(["t1\nt2"], onto: "t2") == nil)
}

@MainActor @Test func aDraggedStatusIsNoMerge() {
    let (m, _) = model()
    #expect(m.dropMerge(["status:todo"], onto: "t1") == nil)
}

// MARK: hover

@MainActor @Test func aCardOffersToMergeTheCardDraggedOverIt() {
    let (m, _) = model()
    _ = m.dragPayload("t2")
    #expect(m.canMerge(into: "t1"))
}

@MainActor @Test func aCardOffersNoMergeWithItself() {
    let (m, _) = model()
    _ = m.dragPayload("t1")
    #expect(!m.canMerge(into: "t1"))
}

@MainActor @Test func aSelectedCardOffersNoMergeWithItsOwnSelection() {
    let (m, _) = model()
    m.open("t1")
    m.click("t2", shift: true, command: false)
    _ = m.dragPayload("t1")
    #expect(!m.canMerge(into: "t2"))
}

@MainActor @Test func aDraggedStatusOffersNoMerge() {
    let (m, _) = model()
    _ = m.dragPayload("t2")
    _ = m.statusDragPayload("todo")
    #expect(!m.canMerge(into: "t1"))
}

// MARK: after merging

@MainActor @Test func theMergedConversationIsSelected() async {
    let (m, _) = model()
    m.open("t3")
    await m.dropMerge(["t2"], onto: "t1")?.value
    #expect(m.selectedThreadId == "t1")
}

@MainActor @Test func theToastNamesTheConversation() async {
    let (m, _) = model()
    await m.dropMerge(["t2"], onto: "t1")?.value
    #expect(m.toast == "Merged into “Subject 1”")
}

@MainActor @Test func theToastOffersUndo() async {
    let (m, _) = model()
    await m.dropMerge(["t2"], onto: "t1")?.value
    #expect(m.toastUndo == m.toast)
}

@MainActor @Test func aMergeIsOneUndoEntry() async {
    let (m, _) = model()
    await m.dropMerge(["t2"], onto: "t1")?.value
    #expect(m.undoLabel == "Merge")
}

@MainActor @Test func aMergeClearsTheSelectionItCarried() async {
    let (m, _) = model()
    m.open("t1")
    m.click("t2", shift: true, command: false)
    await m.dropMerge(["t1\nt2"], onto: "t0")?.value
    #expect(m.multiSelection.isEmpty)
}

@MainActor @Test func undoSplitsTheMergeBack() async {
    let (m, stub) = model()
    await m.dropMerge(["t2"], onto: "t1")?.value
    await m.undo()?.value
    #expect(stub.unmerged == [stub.undo])
}

@MainActor @Test func shiftMJoinsTheSelectionIntoTheOpenRow() async {
    let (m, stub) = model()
    m.open("t1")
    m.click("t3", shift: true, command: false)
    await m.mergeSelected()?.value
    #expect(stub.merged == [["t1", "t2", "t3"]])
}

@MainActor @Test func shiftMCanBeUndone() async {
    let (m, _) = model()
    m.open("t1")
    m.click("t2", shift: true, command: false)
    await m.mergeSelected()?.value
    #expect(m.undoLabel == "Merge")
}

// MARK: drops handed to the views

/// A drop as AppKit hands it to a view: a pasteboard holding the dragged text, at a
/// point in the window.
private final class FakeDrop: NSObject, NSDraggingInfo {
    let pasteboard: NSPasteboard
    let location: NSPoint
    let window: NSWindow

    init(_ text: String, at location: NSPoint, in window: NSWindow) {
        pasteboard = NSPasteboard(name: NSPasteboard.Name("mermoz-test-\(UUID().uuidString)"))
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
        self.location = location
        self.window = window
    }

    var draggingDestinationWindow: NSWindow? { window }
    var draggingSourceOperationMask: NSDragOperation { [.copy, .move, .generic] }
    var draggingLocation: NSPoint { location }
    var draggedImageLocation: NSPoint { location }
    var draggedImage: NSImage? { nil }
    var draggingPasteboard: NSPasteboard { pasteboard }
    var draggingSource: Any? { nil }
    var draggingSequenceNumber: Int { 1 }
    func slideDraggedImage(to screenPoint: NSPoint) {}
    var draggingFormation: NSDraggingFormation = .default
    var animatesToDestination = false
    var numberOfValidItemsForDrop = 1
    func enumerateDraggingItems(options enumOpts: NSDraggingItemEnumerationOptions = [], for view: NSView?,
                                classes classArray: [AnyClass], searchOptions: [NSPasteboard.ReadingOptionKey: Any] = [:],
                                using block: (NSDraggingItem, Int, UnsafeMutablePointer<ObjCBool>) -> Void) {
        let objects = pasteboard.readObjects(forClasses: classArray, options: searchOptions) ?? []
        var stop: ObjCBool = false
        for (i, o) in objects.enumerated() {
            guard let writer = o as? NSPasteboardWriting else { continue }
            block(NSDraggingItem(pasteboardWriter: writer), i, &stop)
            if stop.boolValue { break }
        }
    }
    var springLoadingHighlight: NSSpringLoadingHighlight { .none }
    func resetSpringLoading() {}
    /// Asked by AppKit's own drag session object, which this stands in for.
    @objc func _lastDragDestinationOperation() -> NSDragOperation { .copy }
}

/// A view in an offscreen window, dropped onto at points of its own.
@MainActor
private final class Stage {
    let window: NSWindow
    let host: NSHostingView<AnyView>
    let model: AppModel

    init(_ view: some View, model: AppModel, size: CGSize) {
        self.model = model
        host = NSHostingView(rootView: AnyView(view.environment(model)))
        window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: size.width, height: size.height),
                          styleMask: .borderless, backing: .buffered, defer: false)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
    }

    /// Drops `text` at `point` (from the top left), as AppKit would; whether it was taken.
    func drop(_ text: String, at point: CGPoint) async -> Bool {
        host.layoutSubtreeIfNeeded()
        // The hosting view is flipped: its y runs down from the top, as `point` does.
        let inWindow = host.convert(NSPoint(x: point.x, y: host.isFlipped ? point.y : host.bounds.height - point.y), to: nil)
        guard let target = destination(at: inWindow, in: host) else { return false }
        let info = FakeDrop(text, at: inWindow, in: window)
        // Every drop handler clears the drag it ends: that tells when one has run.
        let pending = ["(drop pending)"]
        model.dragIds = pending
        _ = target.draggingEntered(info)
        _ = target.draggingUpdated(info)
        let taken = target.prepareForDragOperation(info) && target.performDragOperation(info)
        target.concludeDragOperation(info)
        // SwiftUI loads the dropped items asynchronously before calling the handler.
        for _ in 0..<100 where model.dragIds == pending { try? await Task.sleep(for: .milliseconds(10)) }
        // Then the merge it started reaches the stub daemon.
        try? await Task.sleep(for: .milliseconds(30))
        return taken
    }

    /// The view AppKit would hand the drop to: the frontmost one under the point that
    /// takes drags (SwiftUI adds one such view per drop destination).
    private func destination(at p: NSPoint, in v: NSView) -> NSView? {
        for sub in v.subviews.reversed() where !sub.isHidden {
            if let d = destination(at: p, in: sub) { return d }
        }
        let takes = !v.registeredDraggedTypes.isEmpty && v.convert(v.bounds, to: nil).contains(p)
        return takes ? v : nil
    }
}

// MARK: Board: the card merges, the column around it moves

/// One Board column of three To do cards, 400 pt tall: the cards fill its top, its
/// bottom is empty column.
@MainActor
private func boardColumn() -> (Stage, AppModel, StubConversations) {
    let (m, stub) = model(3)
    m.layout = .board
    let group = m.groups.first { $0.key == "todo" } ?? m.groups[0]
    let stage = Stage(BoardColumn(group: group, total: 3), model: m, size: CGSize(width: 250, height: 400))
    return (stage, m, stub)
}

/// The first card's middle: column padding, header, then the card (35-112 pt down).
private let onFirstCard = CGPoint(x: 125, y: 75)
/// The 8 pt gap between the first and second cards.
private let betweenTheCards = CGPoint(x: 125, y: 116)
/// Below the last card.
private let belowTheCards = CGPoint(x: 125, y: 380)

@MainActor @Test func aDropOnABoardCardMergesIntoIt() async {
    let (stage, _, stub) = boardColumn()
    _ = await stage.drop("t2", at: onFirstCard)
    #expect(stub.merged == [["t2", "t0"]])
}

@MainActor @Test func aDropOnTheColumnBesideTheCardsMergesNothing() async {
    let (stage, _, stub) = boardColumn()
    _ = await stage.drop("t2", at: belowTheCards)
    #expect(stub.merged.isEmpty)
}

@MainActor @Test func aDropOnTheColumnBesideTheCardsIsTakenByTheColumn() async {
    let (stage, _, _) = boardColumn()
    #expect(await stage.drop("t2", at: belowTheCards))
}

@MainActor @Test func aDropInTheGapBetweenCardsMergesNothing() async {
    let (stage, _, stub) = boardColumn()
    _ = await stage.drop("t2", at: betweenTheCards)
    #expect(stub.merged.isEmpty)
}

@MainActor @Test func aDropInTheGapBetweenCardsIsTakenByTheColumn() async {
    let (stage, _, _) = boardColumn()
    #expect(await stage.drop("t2", at: betweenTheCards))
}

@MainActor @Test func aCardDroppedOnItselfOnTheBoardMergesNothing() async {
    let (stage, _, stub) = boardColumn()
    _ = await stage.drop("t0", at: onFirstCard)
    #expect(stub.merged.isEmpty)
}

// MARK: List rows

/// The middle of a thread's row, in the hosting view's (top-left) points.
@MainActor
private func listRow(_ stage: Stage, _ id: String) -> CGPoint {
    func find(_ v: NSView) -> PassiveTableView? {
        if let t = v as? PassiveTableView { return t }
        for s in v.subviews { if let t = find(s) { return t } }
        return nil
    }
    let table = find(stage.host)!
    for r in 0..<min(table.numberOfRows, 8) { _ = table.view(atColumn: 0, row: r, makeIfNecessary: true) }
    let row = ListItems.row(of: id, in: (table.delegate as! ThreadTable.Coordinator).items)!
    let rect = stage.host.convert(table.rect(ofRow: row), from: table)
    return CGPoint(x: rect.midX, y: rect.midY)
}

@MainActor
private func list() -> (Stage, AppModel, StubConversations) {
    let (m, stub) = model()
    m.layout = .list
    let stage = Stage(ThreadListView(), model: m, size: CGSize(width: 420, height: 500))
    return (stage, m, stub)
}

@MainActor @Test func aRowDroppedOnAnotherRowMergesIntoIt() async {
    let (stage, _, stub) = list()
    _ = await stage.drop("t2", at: listRow(stage, "t0"))
    #expect(stub.merged == [["t2", "t0"]])
}

@MainActor @Test func aSelectionDroppedOnARowMergesAllIntoIt() async {
    let (stage, _, stub) = list()
    _ = await stage.drop("t2\nt3", at: listRow(stage, "t0"))
    #expect(stub.merged == [["t2", "t3", "t0"]])
}

@MainActor @Test func aRowDroppedOnItselfMergesNothing() async {
    let (stage, _, stub) = list()
    _ = await stage.drop("t0", at: listRow(stage, "t0"))
    #expect(stub.merged.isEmpty)
}

@MainActor @Test func drawingTheRowsStartsNoDrag() {
    let (_, m, _) = list()
    #expect(m.dragIds.isEmpty)
}

// MARK: Feed cards

private func card(_ i: Int, batch: Bool = false) -> FeedCard {
    FeedCard(id: "c\(i)", kind: .reply, section: "now", thread: thread(i), members: batch ? [thread(8), thread(9)] : [],
             title: "Card \(i)")
}

@MainActor
private func feedCard(_ c: FeedCard) -> (Stage, StubConversations) {
    let (m, stub) = model()
    let view = FeedCardView(card: c, focus: m.feedFocusCells.flag(c.id), strip: nil, chip: nil)
    return (Stage(view, model: m, size: CGSize(width: 500, height: 220)), stub)
}

@MainActor @Test func aCardDroppedOnAFeedCardMergesIntoIt() async {
    let (stage, stub) = feedCard(card(0))
    _ = await stage.drop("t2", at: CGPoint(x: 250, y: 110))
    #expect(stub.merged == [["t2", "t0"]])
}

@MainActor @Test func aBatchCardTakesNoMerge() async {
    let (stage, stub) = feedCard(card(0, batch: true))
    _ = await stage.drop("t2", at: CGPoint(x: 250, y: 110))
    #expect(stub.merged.isEmpty)
}
