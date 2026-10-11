/// Linear-style display options: group threads by a field, optionally sub-group, and order them.
import Foundation

public enum Grouping: String, CaseIterable, Codable, Sendable {
    case status, priority, project, account, none

    public var label: String {
        switch self {
        case .status: "Status"
        case .priority: "Priority"
        case .project: "Project"
        case .account: "Account"
        case .none: "No grouping"
        }
    }
}

public enum Ordering: String, CaseIterable, Codable, Sendable {
    case lastActivity = "last_date", priority, due, created

    public var label: String {
        switch self {
        case .lastActivity: "Last activity"
        case .priority: "Priority"
        case .due: "Follow-up date"
        case .created: "Created"
        }
    }
}

public struct DisplayOptions: Codable, Equatable, Sendable {
    public var grouping: Grouping
    public var subGrouping: Grouping
    public var ordering: Ordering

    public init(grouping: Grouping, subGrouping: Grouping = .none, ordering: Ordering = .lastActivity) {
        self.grouping = grouping; self.subGrouping = subGrouping; self.ordering = ordering
    }
}

public struct ThreadGroup: Identifiable, Equatable, Sendable {
    public var field: Grouping
    public var key: String
    public var threads: [ThreadSummary]
    public var subgroups: [ThreadGroup]
    public var id: String { "\(field.rawValue):\(key)" }

    /// Threads in display order, sub-groups flattened. A thread in several projects
    /// appears once per project group.
    public var all: [ThreadSummary] { subgroups.isEmpty ? threads : subgroups.flatMap(\.all) }
}

public enum Grouper {
    /// The values a thread has for a field; "" means "none". Only projects can have several.
    public static func keys(_ t: ThreadSummary, by field: Grouping) -> [String] {
        switch field {
        case .status: [t.status]
        case .priority: [String(t.priority)]
        case .project: t.projects.isEmpty ? [""] : t.projects
        case .account: [t.accountId]
        case .none: [""]
        }
    }

    /// Groups threads in `order` (known keys first, then any others, "none" last), keeping
    /// each group's incoming order, then sub-groups each group the same way.
    public static func group(_ threads: [ThreadSummary], by field: Grouping, then sub: Grouping = .none,
                             order: (Grouping) -> [String]) -> [ThreadGroup] {
        guard field != .none else {
            return [ThreadGroup(field: .none, key: "", threads: threads, subgroups: sub == .none ? [] : group(threads, by: sub, order: order))]
        }
        var buckets: [String: [ThreadSummary]] = [:]
        var seen: [String] = []
        for t in threads {
            for k in keys(t, by: field) {
                if buckets[k] == nil { seen.append(k) }
                buckets[k, default: []].append(t)
            }
        }
        let known = order(field)
        var keys = known.filter { buckets[$0] != nil }
        keys += seen.filter { !known.contains($0) && !$0.isEmpty }
        if buckets[""] != nil, !keys.contains("") { keys.append("") }
        return keys.map { k in
            let ts = buckets[k] ?? []
            let subs = (sub == .none || sub == field) ? [] : group(ts, by: sub, order: order)
            return ThreadGroup(field: field, key: k, threads: ts, subgroups: subs)
        }
    }
}
