/// Grouping keeps Linear's order and nests sub-groups.
import Foundation
import Testing
@testable import MermozKit

private func thread(_ id: String, status: String, priority: Int = 0, project: String? = nil) -> ThreadSummary {
    ThreadSummary(id: id, number: id, accountId: "a", subject: "", snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: false, messageCount: 1, hasAttachments: false, status: status, priority: priority, projects: project.map { [$0] } ?? [])
}

private let sample = [thread("1", status: "todo", priority: 2), thread("2", status: "unread"), thread("3", status: "todo", priority: 1, project: "p")]
private let order: @Sendable (Grouping) -> [String] = { f in
    switch f {
    case .status: ["unread", "todo"]
    case .priority: ["1", "2", "3", "4", "0"]
    default: []
    }
}

@Test func statusGroupsFollowWorkflowOrder() {
    #expect(Grouper.group(sample, by: .status, order: order).map(\.key) == ["unread", "todo"])
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

@Test func threadInTwoProjectsShowsInBoth() {
    var t = thread("9", status: "todo")
    t.projects = ["a", "b"]
    #expect(Grouper.group([t], by: .project, order: { _ in ["a", "b"] }).map(\.key) == ["a", "b"])
}

@Test(arguments: [
    ("Immo/Houlgate", Set(["Immo", "Immo/Houlgate"]), 1, "Houlgate"),
    ("[Gmail]/Famille", Set(["[Gmail]/Famille"]), 0, "[Gmail]/Famille"),
    ("A/B/C", Set(["A", "A/B/C"]), 1, "B/C"),
    ("Perso", Set(["Perso"]), 0, "Perso"),
])
func projectNesting(_ name: String, _ names: Set<String>, _ depth: Int, _ label: String) {
    let p = ProjectTree.place(name, among: names)
    #expect(p.depth == depth && p.label == label)
}

@Test func displayOptionsSavedBeforeDirectionStillLoad() throws {
    let old = #"{"grouping":"status","subGrouping":"none","ordering":"priority"}"#
    #expect(try JSONDecoder().decode(DisplayOptions.self, from: Data(old.utf8)).reversed == false)
}
