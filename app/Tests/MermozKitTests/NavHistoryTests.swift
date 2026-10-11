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
