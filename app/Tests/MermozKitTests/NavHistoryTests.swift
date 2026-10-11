/// Back/forward behaves like a browser.
import Testing
@testable import MermozKit

@Test func backReturnsPreviousPlace() {
    var h = NavHistory<String>()
    h.record(leaving: "inbox")
    #expect(h.goBack(from: "thread") == "inbox")
}

@Test func forwardUndoesBack() {
    var h = NavHistory<String>()
    h.record(leaving: "inbox")
    _ = h.goBack(from: "thread")
    #expect(h.goForward(from: "inbox") == "thread")
}

@Test func newPlaceClearsForward() {
    var h = NavHistory<String>()
    h.record(leaving: "inbox")
    _ = h.goBack(from: "thread")
    h.record(leaving: "inbox")
    #expect(!h.canGoForward)
}

@Test func samePlaceIsNotRecordedTwice() {
    var h = NavHistory<String>()
    h.record(leaving: "inbox")
    h.record(leaving: "inbox")
    _ = h.goBack(from: "thread")
    #expect(!h.canGoBack)
}

@Test func historyIsCapped() {
    var h = NavHistory<Int>(limit: 2)
    for i in 0..<5 { h.record(leaving: i) }
    _ = h.goBack(from: 9); _ = h.goBack(from: 4)
    #expect(!h.canGoBack)
}

private func snap(status: String = "todo", unread: Bool = false, priority: Int = 0, projects: [String] = [], due: String? = nil) -> ThreadSummary {
    ThreadSummary(id: "t", number: "M-1", accountId: "a", subject: "", snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: unread, messageCount: 1, hasAttachments: false, status: status, priority: priority,
                  projects: projects, due: due)
}

@Test func undoingADeleteRestoresTheStatus() {
    #expect(RestorePlan.between(before: snap(status: "todo"), now: snap(status: "deleted")).status == "todo")
}

@Test func undoingAReadMarksUnreadAgain() {
    #expect(RestorePlan.between(before: snap(unread: true), now: snap(unread: false)).markRead == false)
}

@Test func unchangedReadStateIsLeftAlone() {
    #expect(RestorePlan.between(before: snap(), now: snap(status: "archive")).markRead == nil)
}

@Test func undoingAnAcceptedProjectRemovesIt() {
    #expect(RestorePlan.between(before: snap(projects: ["A"]), now: snap(projects: ["A", "Receipts"])).removeProjects == ["Receipts"])
}

@Test func undoingARemovedProjectAddsItBack() {
    #expect(RestorePlan.between(before: snap(projects: ["A"]), now: snap()).addProjects == ["A"])
}

@Test func undoingASetFollowUpClearsIt() {
    #expect(RestorePlan.between(before: snap(), now: snap(due: "2026-10-20")).due == .some(nil))
}
