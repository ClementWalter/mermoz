/// Browser-style back/forward history: recording a new place clears the forward trail.
public struct NavHistory<Place: Equatable>: Sendable where Place: Sendable {
    private var back: [Place] = []
    private var forward: [Place] = []
    private let limit: Int

    public init(limit: Int = 200) { self.limit = limit }

    public var canGoBack: Bool { !back.isEmpty }
    public var canGoForward: Bool { !forward.isEmpty }

    /// Call before leaving `current` for somewhere new.
    public mutating func record(leaving current: Place) {
        guard back.last != current else { return }
        back.append(current)
        if back.count > limit { back.removeFirst() }
        forward.removeAll()
    }

    /// Where to go back to, given where we are now.
    public mutating func goBack(from current: Place) -> Place? {
        guard let p = back.popLast() else { return nil }
        forward.append(current)
        return p
    }

    public mutating func goForward(from current: Place) -> Place? {
        guard let p = forward.popLast() else { return nil }
        back.append(current)
        return p
    }
}

/// What to send to put a thread back the way it was before a change (⌘Z).
public struct RestorePlan: Equatable, Sendable {
    /// Read state to set first, if it changed (`true` = read).
    public var markRead: Bool?
    public var status: String
    public var priority: Int?
    public var addProjects: [String]
    public var removeProjects: [String]
    /// `nil`: keep; `.some(nil)`: clear; `.some(d)`: set.
    public var due: String??

    public static func between(before: ThreadSummary, now: ThreadSummary) -> RestorePlan {
        RestorePlan(
            markRead: before.unread == now.unread ? nil : !before.unread,
            status: before.status,
            priority: before.priority == now.priority ? nil : before.priority,
            addProjects: Set(before.projects).subtracting(now.projects).sorted(),
            removeProjects: Set(now.projects).subtracting(before.projects).sorted(),
            due: before.due == now.due ? nil : .some(before.due))
    }
}
