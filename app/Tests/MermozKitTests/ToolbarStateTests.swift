/// The reading pane's toolbar stays as it is between threads that show the same chips.
import Foundation
import Testing
@testable import MermozKit

private func thread(_ id: String, status: String = "todo", priority: Int = 0, projects: [String] = [],
                    due: String? = nil) -> ThreadSummary {
    var t = ThreadSummary(id: id, number: id, accountId: "a", subject: "Subject \(id)", snippet: "", participants: [],
                          lastFrom: "", lastDate: 0, unread: false, messageCount: 1, hasAttachments: false, status: status,
                          priority: priority)
    t.projects = projects
    t.due = due
    return t
}

@Test func anotherThreadWithTheSameChipsShowsTheSameToolbar() {
    #expect(thread("t1", projects: ["Home"]).showsSameChips(as: thread("t2", projects: ["Home"])))
}

@Test func anotherStatusChangesTheToolbar() {
    #expect(!thread("t1").showsSameChips(as: thread("t2", status: "waiting")))
}

@Test func anotherPriorityChangesTheToolbar() {
    #expect(!thread("t1").showsSameChips(as: thread("t2", priority: 2)))
}

@Test func anotherProjectChangesTheToolbar() {
    #expect(!thread("t1", projects: ["Home"]).showsSameChips(as: thread("t2", projects: ["Work"])))
}

@Test func aDueDateChangesTheToolbar() {
    #expect(!thread("t1").showsSameChips(as: thread("t2", due: "2026-10-20")))
}
