/// Joining conversations by hand: what a drop of rows or cards onto another one merges,
/// the daemon calls behind it, and the token that takes a merge back.
import Foundation

/// A thread's conversation before a merge.
public struct MergedThread: Codable, Sendable, Equatable {
    public var id: String
    public var conversation: String?
    public var state: String

    public init(id: String, conversation: String?, state: String) {
        self.id = id
        self.conversation = conversation
        self.state = state
    }
}

/// A join pair's row before a merge recorded it.
public struct PairRecord: Codable, Sendable, Equatable {
    public var people: String
    public var jev: Double?
    public var ai: Int?
    public var decision: String
    public var source: String
    public var created: Int64
}

/// A pair a merge recorded as joined; `prior` is nil when the pair was new.
public struct MergedPair: Codable, Sendable, Equatable {
    public var later: String
    public var earlier: String
    public var prior: PairRecord?
}

/// What `conversations.unmerge` needs to put a merge back. Its keys are single words, so
/// it round-trips through the snake_case decoder and the plain params encoder alike.
public struct MergeUndo: Codable, Sendable, Equatable {
    public var threads: [MergedThread]
    public var pairs: [MergedPair]

    public init(threads: [MergedThread] = [], pairs: [MergedPair] = []) {
        self.threads = threads
        self.pairs = pairs
    }
}

/// `conversations.merge`.
public struct MergeResult: Codable, Sendable, Equatable {
    public var conversationId: String
    /// Every thread now in the conversation.
    public var members: [String]
    public var undo: MergeUndo

    public init(conversationId: String, members: [String], undo: MergeUndo = MergeUndo()) {
        self.conversationId = conversationId
        self.members = members
        self.undo = undo
    }
}

/// The daemon calls a manual merge makes; the app's client, or a stub in tests.
public protocol ConversationBackend: Sendable {
    func mergeConversations(_ threadIds: [String]) async throws -> MergeResult
    func unmergeConversations(_ undo: MergeUndo) async throws
}

extension DaemonClient: ConversationBackend {
    /// Joins the conversations of these threads into one; the grouping learns the join.
    public func mergeConversations(_ threadIds: [String]) async throws -> MergeResult {
        let data = try await connection.call("conversations.merge", params: Params().with("ids", threadIds).data())
        return try RPCCoding.decoder().decode(MergeResult.self, from: data)
    }

    /// Takes a merge back: each thread in its conversation again, the join unlearned.
    public func unmergeConversations(_ undo: MergeUndo) async throws {
        _ = try await connection.call("conversations.unmerge", params: Params().with("undo", undo).data())
    }
}

public enum MergeDrop {
    /// The thread ids a dropped payload carries: a multi-selection holds one per line; a
    /// dragged sidebar status ("status:…") carries none.
    public static func ids(in items: [String]) -> [String] {
        var out: [String] = []
        for line in items.flatMap({ $0.split(separator: "\n").map(String.init) })
            where !line.isEmpty && !line.hasPrefix("status:") && !out.contains(line) {
            out.append(line)
        }
        return out
    }

    /// The threads a drop onto `target` joins, the target last; nil when nothing else is
    /// dropped (the card itself, or a selection that holds it).
    public static func merge(_ dragged: [String], onto target: String) -> [String]? {
        guard !dragged.isEmpty, !dragged.contains(target) else { return nil }
        return dragged + [target]
    }
}
