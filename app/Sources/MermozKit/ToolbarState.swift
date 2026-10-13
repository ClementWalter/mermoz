/// What the reading pane's toolbar shows of a thread. Stepping between threads that
/// show the same chips leaves the toolbar as it is.
import Foundation

public extension ThreadSummary {
    /// Same status, priority, projects and due date: the toolbar would look the same.
    func showsSameChips(as other: ThreadSummary) -> Bool {
        status == other.status && priority == other.priority && projects == other.projects && due == other.due
    }
}
