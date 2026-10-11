/// Grouping keeps Linear's order and nests sub-groups.
import Testing
@testable import MermozKit

private func thread(_ id: String, status: String, priority: Int = 0, project: String? = nil) -> ThreadSummary {
    ThreadSummary(id: id, number: id, accountId: "a", subject: "", snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: false, messageCount: 1, hasAttachments: false, status: status, priority: priority, projectId: project)
}

private let sample = [thread("1", status: "todo", priority: 2), thread("2", status: "triage"), thread("3", status: "todo", priority: 1, project: "p")]
private let order: @Sendable (Grouping) -> [String] = { f in
    switch f {
    case .status: ["triage", "todo"]
    case .priority: ["1", "2", "3", "4", "0"]
    default: []
    }
}

@Test func statusGroupsFollowWorkflowOrder() {
    #expect(Grouper.group(sample, by: .status, order: order).map(\.key) == ["triage", "todo"])
}

@Test func groupsKeepIncomingThreadOrder() {
    #expect(Grouper.group(sample, by: .status, order: order)[1].threads.map(\.id) == ["1", "3"])
}

@Test func subGroupsNestByPriority() {
    #expect(Grouper.group(sample, by: .status, then: .priority, order: order)[1].subgroups.map(\.key) == ["1", "2"])
}

@Test func noneKeyComesLast() {
    #expect(Grouper.group(sample, by: .project, order: order).map(\.key) == ["p", ""])
}

@Test func flattenedOrderFollowsSubGroups() {
    #expect(Grouper.group(sample, by: .status, then: .priority, order: order)[1].all.map(\.id) == ["3", "1"])
}
