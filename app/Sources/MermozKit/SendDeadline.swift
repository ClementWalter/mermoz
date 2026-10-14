/// A send answers within a few seconds: done, failed, or still going in the background.
/// Plain words for why a send failed, with what to do next.
import Foundation

/// How a send stood when the deadline came.
public enum DeadlineResult<T: Sendable>: Sendable {
    case done(T)
    case failed(Error)
    /// Not finished yet; the task keeps running.
    case late
}

/// Waits for `task` at most `deadline`, never cancelling it. A race of two watchers on
/// a one-shot continuation: a task group would wait for the send anyway, since awaiting
/// `Task.value` ignores cancellation.
public func waitAtMost<T: Sendable>(_ deadline: Duration, for task: Task<T, Error>) async -> DeadlineResult<T> {
    let once = Once<DeadlineResult<T>>()
    return await withCheckedContinuation { continuation in
        once.arm(continuation)
        Task {
            do { once.resume(.done(try await task.value)) } catch { once.resume(.failed(error)) }
        }
        Task {
            try? await Task.sleep(for: deadline)
            once.resume(.late)
        }
    }
}

/// Resumes a continuation with the first value only.
private final class Once<V: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<V, Never>?

    func arm(_ c: CheckedContinuation<V, Never>) {
        lock.lock(); defer { lock.unlock() }
        continuation = c
    }

    func resume(_ value: V) {
        lock.lock()
        let c = continuation
        continuation = nil
        lock.unlock()
        c?.resume(returning: value)
    }
}

public enum SendErrorText {
    /// The sentence the composer shows under Send.
    public static func describe(_ error: Error) -> String {
        let rpc = error as? RPCError
        let message = rpc?.message ?? error.localizedDescription
        let lower = message.lowercased()
        if rpc?.code == RPCError.transport {
            return "Mermoz lost touch with its background service. Retry in a moment."
        }
        if lower.contains("error sending request") || lower.contains("timed out") || lower.contains("dns error") {
            return "Couldn't reach Gmail. Check your connection, then retry."
        }
        if rpc?.code == RPCError.upstream {
            return "Gmail didn't accept this message (\(message.replacingOccurrences(of: "provider error", with: "error"))). Check the recipients, then retry."
        }
        return "Couldn't send: \(message)"
    }
}
