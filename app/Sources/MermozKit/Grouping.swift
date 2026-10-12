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

    /// The two directions, named for this ordering: (default, reversed).
    public var directions: (String, String) {
        switch self {
        case .lastActivity, .created: ("Newest first", "Oldest first")
        case .priority: ("Urgent first", "Low first")
        case .due: ("Soonest first", "Latest first")
        }
    }
}

public struct DisplayOptions: Codable, Equatable, Sendable {
    public var grouping: Grouping
    public var subGrouping: Grouping
    public var ordering: Ordering
    /// The ordering's second direction (oldest / low / latest first).
    public var reversed: Bool

    public init(grouping: Grouping, subGrouping: Grouping = .none, ordering: Ordering = .lastActivity, reversed: Bool = false) {
        self.grouping = grouping; self.subGrouping = subGrouping; self.ordering = ordering; self.reversed = reversed
    }

    /// Options saved before `reversed` existed still load.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        grouping = try c.decode(Grouping.self, forKey: .grouping)
        subGrouping = try c.decode(Grouping.self, forKey: .subGrouping)
        ordering = try c.decode(Ordering.self, forKey: .ordering)
        reversed = try c.decodeIfPresent(Bool.self, forKey: .reversed) ?? false
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

/// Nesting of "/"-separated project names in the sidebar.
public enum ProjectTree {
    /// Indent by the ancestors that are projects themselves, and label with the rest of
    /// the path: "[Gmail]/Famille" without a "[Gmail]" project stays a top-level row.
    public static func place(_ name: String, among names: Set<String>) -> (depth: Int, label: String) {
        let parts = name.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
        var depth = 0
        var start = 0
        for i in 1..<max(parts.count, 1) where names.contains(parts[0..<i].joined(separator: "/")) {
            depth += 1
            start = i
        }
        return (depth, parts[start...].joined(separator: "/"))
    }

    /// Projects that have sub-projects (and so get a fold arrow).
    public static func parents(among names: [String]) -> Set<String> {
        let all = Set(names)
        return Set(names.compactMap { n in ancestors(n, among: all).last })
    }

    /// The projects to list: those with no folded ancestor.
    public static func visible(_ names: [String], collapsed: Set<String>) -> [String] {
        let all = Set(names)
        return names.filter { n in !ancestors(n, among: all).contains(where: collapsed.contains) }
    }

    /// Ancestors that are projects themselves, outermost first.
    static func ancestors(_ name: String, among names: Set<String>) -> [String] {
        let parts = name.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
        return (1..<max(parts.count, 1)).map { parts[0..<$0].joined(separator: "/") }.filter(names.contains)
    }
}

/// Which statuses a view lists.
public enum ViewStatuses {
    public enum Kind: Sendable {
        /// One status (a single-status list; the board shows every open status).
        case status(String)
        /// A project or Gmail label: everything filed there except the Trash.
        case mailbox
        /// An account: its open threads.
        case open
    }

    public static func visible(_ kind: Kind, all: [String], open: [String], board: Bool) -> [String] {
        switch kind {
        case let .status(k): board ? open : [k]
        case .mailbox: all.filter { $0 != "deleted" }
        case .open: open
        }
    }
}
