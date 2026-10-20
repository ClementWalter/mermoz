/// The open card keeps its board place until it is left, and moves land on the card below.
import Foundation
import Testing
@testable import MermozKit

private func thread(_ id: String, status: String, unread: Bool = false) -> ThreadSummary {
    ThreadSummary(id: id, number: id, accountId: "a", subject: "", snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: unread, messageCount: 1, hasAttachments: false, status: status, priority: 0, projects: [])
}

private let order: @Sendable (Grouping) -> [String] = { _ in ["unread", "todo"] }
private let board = [thread("1", status: "unread"), thread("2", status: "unread", unread: true),
                     thread("3", status: "unread"), thread("4", status: "todo")]

/// The board once "2" is read and moved to To do in place, as `markRead` leaves it.
private var afterReading: [ThreadSummary] {
    var ts = board
    ts[1].status = "todo"
    ts[1].unread = false
    return ts
}

@Test func readCardStaysInItsColumn() {
    let hold = BoardHold("2", in: board)!
    #expect(Grouper.group(hold.apply(afterReading), by: .status, order: order)[0].threads.map(\.id) == ["1", "2", "3"])
}

@Test func readCardLeavesTheOtherColumnAlone() {
    let hold = BoardHold("2", in: board)!
    #expect(Grouper.group(hold.apply(afterReading), by: .status, order: order)[1].threads.map(\.id) == ["4"])
}

@Test func heldCardShowsItsFreshState() {
    let hold = BoardHold("2", in: board)!
    #expect(hold.apply(afterReading)[1].unread == false)
}

@Test func heldCardKeepsItsRowAfterAReloadSortedByStatus() {
    let hold = BoardHold("2", in: board)!
    let reloaded = [board[0], board[2], afterReading[1], board[3]]
    #expect(Grouper.group(hold.apply(reloaded), by: .status, order: order)[0].threads.map(\.id) == ["1", "2", "3"])
}

@Test func firstCardStaysFirst() {
    var read = board
    read[0].status = "todo"
    let hold = BoardHold("1", in: board)!
    #expect(Grouper.group(hold.apply([read[1], read[2], read[3], read[0]]), by: .status, order: order)[0].threads.map(\.id) == ["1", "2", "3"])
}

@Test func cardOffScreenIsNotHeld() {
    #expect(BoardHold("9", in: board) == nil)
}

@Test func movedCardLandsOnTheOneBelow() {
    #expect(BoardStep.next(after: ["2"], in: [["1", "2", "3"], ["4"]]) == "3")
}

@Test func movedLastCardLandsOnTheOneAbove() {
    #expect(BoardStep.next(after: ["3"], in: [["1", "2", "3"], ["4"]]) == "2")
}

@Test func emptiedColumnLandsOnTheNearestColumn() {
    #expect(BoardStep.next(after: ["4"], in: [["1", "2"], ["4"], ["5", "6"]]) == "1")
}

@Test func emptiedBoardLandsNowhere() {
    #expect(BoardStep.next(after: ["4"], in: [["4"]]) == nil)
}

@Test func movedSelectionLandsBelowItsFirstCard() {
    #expect(BoardStep.next(after: ["2", "3"], in: [["1", "2", "3", "7"], ["4"]]) == "7")
}
