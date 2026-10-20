/// On the Board, a card read in Unread stays in its column while open and leaves once
/// another card is selected, so the arrows keep walking the column the user is in.
@testable import MermozKit
import Testing
@testable import Mermoz

private func thread(_ i: Int, status: String) -> ThreadSummary {
    ThreadSummary(id: "t\(i)", number: "M-\(i)", accountId: "a", subject: "Subject \(i)", snippet: "", participants: [],
                  lastFrom: "Sender", lastDate: Int64(1_000 - i), unread: status == "unread", messageCount: 1,
                  hasAttachments: false, status: status, priority: 0)
}

/// A board with three Unread cards and one To do card, the second Unread card open and
/// then read (the daemon moved it to To do).
@MainActor
private func boardAfterReading() -> AppModel {
    let model = AppModel()
    model.statuses = [Status(key: "unread", name: "Unread", order: 0, color: "#000000"),
                      Status(key: "todo", name: "To do", order: 1, color: "#000000")]
    model.layout = .board
    model.threads = [thread(0, status: "unread"), thread(1, status: "unread"), thread(2, status: "unread"), thread(3, status: "todo")]
    model.open("t1")
    model.threads[1].status = "todo"
    model.threads[1].unread = false
    return model
}

@MainActor @Test func anOpenCardStaysInUnreadOnceRead() {
    #expect(boardAfterReading().groups.first { $0.key == "unread" }?.threads.map(\.id) == ["t0", "t1", "t2"])
}

@MainActor @Test func arrowDownWalksTheUnreadColumn() {
    let model = boardAfterReading()
    model.moveBoard(dx: 0, dy: 1)
    #expect(model.selectedThreadId == "t2")
}

@MainActor @Test func aReadCardMovesOnceLeft() {
    let model = boardAfterReading()
    model.moveBoard(dx: 0, dy: 1)
    #expect(model.groups.first { $0.key == "todo" }?.threads.map(\.id) == ["t1", "t3"])
}
