/// Sidebar counts as one observable cell per badge, and the refresh that fills them.
///
/// A view reading one cell is invalidated only when that number changes, so a count
/// moving after an email is read redraws its own badge, not the projects tree.
import Foundation
import Observation

/// One badge: a status, project, account, priority or conversation kind, or a figure the
/// app derives (Review's drafts, the Feeds total).
public enum CountKey: Hashable, Sendable {
    case status(String)
    case project(String)
    case account(String)
    case priority(String)
    case kind(String)
    case reviewBadge
    case feeds
}

/// The number on one badge; nil when nothing is counted there.
@MainActor
@Observable
public final class CountCell {
    public fileprivate(set) var value: Int?

    init(_ value: Int?) { self.value = value }
}

/// Every badge's cell, created when a view first asks for it. Updates touch only the
/// cells whose number changed.
@MainActor
public final class CountCells {
    private var cells: [CountKey: CountCell] = [:]
    /// The last values applied, so a cell asked for later starts right.
    private var values: [CountKey: Int] = [:]

    public init() {}

    public func cell(_ key: CountKey) -> CountCell {
        if let c = cells[key] { return c }
        let c = CountCell(values[key])
        cells[key] = c
        return c
    }

    public func value(_ key: CountKey) -> Int? { cell(key).value }

    /// Applies a daemon answer plus derived badges; returns the keys whose number moved.
    @discardableResult
    public func apply(_ counts: Counts, extra: [CountKey: Int?] = [:]) -> Set<CountKey> {
        var next: [CountKey: Int] = [:]
        for (k, n) in counts.status { next[.status(k)] = n }
        for (k, n) in counts.project { next[.project(k)] = n }
        for (k, n) in counts.account { next[.account(k)] = n }
        for (k, n) in counts.priority ?? [:] { next[.priority(k)] = n }
        for (k, n) in counts.kind ?? [:] { next[.kind(k)] = n }
        for (k, n) in extra { next[k] = n }
        var changed = Set<CountKey>()
        for k in Set(values.keys).union(next.keys) where values[k] != next[k] {
            changed.insert(k)
            // Setting an Observable property notifies even when equal: only moved cells are set.
            cells[k]?.value = next[k]
        }
        values = next
        return changed
    }
}

/// Runs a refresh at most once per burst: requests within `delay` merge into one run,
/// and requests while a run is in flight cause exactly one more run after it.
@MainActor
public final class RefreshCoalescer {
    private let delay: Duration
    private let work: @MainActor () async -> Void
    private var pending: Task<Void, Never>?
    private var running = false
    private var again = false
    /// How many times `work` started; for tests and logs.
    public private(set) var runs = 0

    public init(delay: Duration = .milliseconds(100), work: @escaping @MainActor () async -> Void) {
        self.delay = delay
        self.work = work
    }

    public func request() {
        if running { again = true; return }
        guard pending == nil else { return }
        pending = Task { [weak self, delay] in
            try? await Task.sleep(for: delay)
            await self?.run()
        }
    }

    /// Waits until no run is pending or in flight.
    public func settle() async {
        while pending != nil || running {
            await pending?.value
            if running { try? await Task.sleep(for: .milliseconds(5)) }
        }
    }

    private func run() async {
        pending = nil
        running = true
        runs += 1
        await work()
        running = false
        if again {
            again = false
            request()
        }
    }
}
