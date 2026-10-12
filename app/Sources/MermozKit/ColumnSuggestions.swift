/// Background sorting of the open columns: each column's pending AI suggestions,
/// grouped by where they would send threads, and how far sorting has got.
import Foundation

/// Threads of a column the AI would send to the same place.
public struct SuggestionGroup: Codable, Sendable, Equatable, Hashable, Identifiable {
    public var status: String?
    public var projects: [String]
    public var count: Int

    public init(status: String?, projects: [String], count: Int) {
        self.status = status; self.projects = projects; self.count = count
    }

    /// Same for every thread going to the same place (the count is not part of it).
    public var id: String { (status ?? "") + "|" + projects.joined(separator: "\u{1F}") }

    /// Rebuilds a destination from its `id` (a `.suggestion` group key).
    public init?(id: String) {
        guard let bar = id.firstIndex(of: "|") else { return nil }
        let s = String(id[..<bar])
        let p = String(id[id.index(after: bar)...])
        self.init(status: s.isEmpty ? nil : s, projects: p.isEmpty ? [] : p.components(separatedBy: "\u{1F}"), count: 0)
    }

    /// "Archive · Receipts": the status by its display name, nested projects by their leaf.
    public func label(statusName: (String) -> String) -> String {
        var parts: [String] = status.map { [statusName($0)] } ?? []
        parts += projects.map { $0.split(separator: "/").last.map(String.init) ?? $0 }
        return parts.joined(separator: " · ")
    }
}

/// One column's pending suggestions and sorting progress.
public struct ColumnSuggestions: Codable, Sendable, Equatable {
    public var status: String
    public var pending: Int
    public var groups: [SuggestionGroup]
    /// Threads the AI has already looked at (with or without a suggestion).
    public var classified: Int
    public var total: Int
    /// Whether background sorting works through this column; older daemons leave it
    /// out and only the open columns are sorted.
    public var sorted: Bool?

    public init(status: String, pending: Int, groups: [SuggestionGroup], classified: Int, total: Int,
                sorted: Bool? = nil) {
        self.status = status; self.pending = pending; self.groups = groups
        self.classified = classified; self.total = total; self.sorted = sorted
    }

    /// Threads still waiting for the AI's look.
    public var remaining: Int { max(0, total - classified) }
}

/// `suggestions.summary`.
public struct SuggestionsSummary: Codable, Sendable, Equatable {
    public var columns: [ColumnSuggestions]
    public var sorting: Bool
    /// "limit" (today's allowance is used up) or "unavailable" (retrying later).
    public var paused: String?
    public var callsToday: Int
    public var dailyCap: Int
    /// The whole-mailbox sort, while it runs.
    public var resort: Resort?
}

/// A second look at whole columns, asked for by the user; the daily limit does not
/// apply while it runs.
public struct Resort: Codable, Sendable, Equatable {
    public var since: Int
    public var statuses: [String]
    /// Emails in those columns when it started.
    public var total: Int
    public var ceiling: Int
    /// Emails looked at so far.
    public var calls: Int
    public var inputTokens: Int

    public init(since: Int = 0, statuses: [String], total: Int, ceiling: Int, calls: Int, inputTokens: Int = 0) {
        self.since = since; self.statuses = statuses; self.total = total
        self.ceiling = ceiling; self.calls = calls; self.inputTokens = inputTokens
    }
}

/// `suggestions.resort`.
public struct ResortStart: Codable, Sendable, Equatable {
    public var resort: Resort
    /// Emails due for a fresh look.
    public var due: Int
}

/// Plain words for the whole-mailbox sort in Settings.
public enum ResortText {
    /// "Sorting your whole mailbox: 1,240 of 27,315 emails looked at".
    public static func progress(_ r: Resort, locale: Locale = .current) -> String {
        let n = { (v: Int) in v.formatted(.number.locale(locale)) }
        return "Sorting your whole mailbox: \(n(min(r.calls, r.total))) of \(n(r.total)) emails looked at"
    }
}

/// `suggestions.accept`: how many moved, and the threads as they were (for undo).
public struct ColumnAccept: Codable, Sendable {
    public var accepted: Int
    public var before: [ThreadSummary]
}

/// What `threads.restore` puts back; keys spelled as the protocol does.
public struct ThreadState: Encodable, Sendable, Equatable {
    public var id: String
    public var unread: Bool
    public var status: String
    public var priority: Int
    public var projects: [String]
    public var due: String?

    public init(_ t: ThreadSummary) {
        id = t.id; unread = t.unread; status = t.status; priority = t.priority
        projects = t.projects; due = t.due
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: DynamicKey.self)
        try c.encode(id, forKey: DynamicKey("id"))
        try c.encode(unread, forKey: DynamicKey("unread"))
        try c.encode(status, forKey: DynamicKey("status"))
        try c.encode(priority, forKey: DynamicKey("priority"))
        try c.encode(projects, forKey: DynamicKey("projects"))
        // An explicit null clears a follow-up date the accept could not have set.
        try c.encode(due, forKey: DynamicKey("due"))
    }
}

/// Reviewing a column's suggestions before accepting: one destination, or all of them
/// grouped by destination.
public struct SuggestionReview: Equatable, Sendable {
    public var status: String
    /// nil: every suggestion of the column.
    public var group: SuggestionGroup?

    public init(status: String, group: SuggestionGroup? = nil) {
        self.status = status; self.group = group
    }

    /// "Reviewing → Archive · 758" or "Reviewing 2,693 suggestions".
    public func title(count: Int, statusName: (String) -> String, locale: Locale = .current) -> String {
        let n = count.formatted(.number.locale(locale))
        guard let group else { return "Reviewing \(ColumnSortText.pending(count, locale: locale))" }
        return "Reviewing → \(group.label(statusName: statusName)) · \(n)"
    }

    /// "Accept these 758" / "Accept all 2,693".
    public func acceptLabel(count: Int, locale: Locale = .current) -> String {
        let n = count.formatted(.number.locale(locale))
        return group == nil ? "Accept all \(n)" : "Accept these \(n)"
    }
}

/// Plain-language text for a column's suggestion bar.
public enum ColumnSortText {
    /// Columns the background pass sorts; the others have no progress to show.
    public static let sortedStatuses: Set<String> = ["todo", "waiting", "in_progress", "backlog"]

    /// "Sorting… 1,240 / 7,256" while threads of the column still wait for a look; nil
    /// once the column is done, for columns that are not sorted, or when sorting is off.
    public static func progress(_ c: ColumnSuggestions, sorting: Bool, paused: String?,
                                locale: Locale = .current) -> String? {
        guard c.sorted ?? sortedStatuses.contains(c.status), c.remaining > 0 else { return nil }
        switch paused {
        case "limit": return "Sorting continues tomorrow"
        case "unavailable": return "Sorting paused, retrying soon"
        default: break
        }
        guard sorting else { return nil }
        let n = { (v: Int) in v.formatted(.number.locale(locale)) }
        return "Sorting… \(n(c.classified)) / \(n(c.total))"
    }

    /// "312 suggestions".
    public static func pending(_ n: Int, locale: Locale = .current) -> String {
        "\(n.formatted(.number.locale(locale))) suggestion\(n == 1 ? "" : "s")"
    }
}
