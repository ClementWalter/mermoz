/// "Ask Mermoz to do" in ⌘K: what the daemon's `do.*` methods return, when typed text
/// reads as an instruction rather than a search, and the typed confirmation of a delete.
import Foundation

/// Whether a plan can be asked for now (an AI is allowed for some account).
public struct DoAvailability: Codable, Sendable, Equatable {
    public var available: Bool
    public var message: String?
    public var providerName: String?

    public init(available: Bool, message: String? = nil, providerName: String? = nil) {
        self.available = available
        self.message = message
        self.providerName = providerName
    }
}

/// A thread a step would change, as the preview lists it.
public struct DoSample: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var number: String
    public var status: String
    public var subject: String
    public var lastFrom: String

    public init(id: String, number: String, status: String, subject: String, lastFrom: String) {
        self.id = id
        self.number = number
        self.status = status
        self.subject = subject
        self.lastFrom = lastFrom
    }
}

public struct DoStatusCount: Codable, Sendable, Equatable {
    public var status: String
    public var name: String
    public var count: Int

    public init(status: String, name: String, count: Int) {
        self.status = status
        self.name = name
        self.count = count
    }
}

public struct DoStep: Codable, Sendable, Equatable {
    public var summary: String
    public var interpretation: String
    public var action: String
    public var count: Int
    public var byStatus: [DoStatusCount]
    public var sample: [DoSample]

    public init(summary: String, interpretation: String, action: String, count: Int,
                byStatus: [DoStatusCount] = [], sample: [DoSample] = []) {
        self.summary = summary
        self.interpretation = interpretation
        self.action = action
        self.count = count
        self.byStatus = byStatus
        self.sample = sample
    }
}

/// `do.ask`: a plan previewed (`kind` "plan"), or a sentence back ("question", "needs_ai").
public struct DoPreview: Codable, Sendable, Equatable {
    public var kind: String
    public var message: String?
    public var id: String?
    public var summary: String?
    public var interpretation: String?
    public var steps: [DoStep]?
    public var total: Int?
    public var deletes: Bool?
    public var warning: String?
    /// "enter", or "type_count" for a delete.
    public var confirm: String?

    public init(kind: String, message: String? = nil, id: String? = nil, summary: String? = nil,
                interpretation: String? = nil, steps: [DoStep]? = nil, total: Int? = nil, deletes: Bool? = nil,
                warning: String? = nil, confirm: String? = nil) {
        self.kind = kind
        self.message = message
        self.id = id
        self.summary = summary
        self.interpretation = interpretation
        self.steps = steps
        self.total = total
        self.deletes = deletes
        self.warning = warning
        self.confirm = confirm
    }

    public var isPlan: Bool { kind == "plan" && id != nil }
    public var needsTypedCount: Bool { confirm == "type_count" }
}

public struct DoRunResult: Codable, Sendable, Equatable {
    public var id: String
    public var done: Int
    public var total: Int
    public var undoLabel: String

    public init(id: String, done: Int, total: Int, undoLabel: String) {
        self.id = id
        self.done = done
        self.total = total
        self.undoLabel = undoLabel
    }
}

/// A logged run, polled for its progress.
public struct DoOperation: Codable, Sendable, Equatable {
    public var id: String
    public var state: String
    public var done: Int
    public var total: Int

    public init(id: String, state: String, done: Int, total: Int) {
        self.id = id
        self.state = state
        self.done = done
        self.total = total
    }
}

/// The daemon calls the bar makes; the app's client, or a stub in tests.
public protocol AskBackend: Sendable {
    func doStatus() async throws -> DoAvailability
    func doAsk(_ text: String) async throws -> DoPreview
    func doRun(_ id: String, confirm: Int?) async throws -> DoRunResult
    func doGet(_ id: String) async throws -> DoOperation
    func doUndo(_ id: String) async throws
}

extension DaemonClient: AskBackend {
    public func doStatus() async throws -> DoAvailability {
        let data = try await connection.call("do.status", params: Params().data())
        return try RPCCoding.decoder().decode(DoAvailability.self, from: data)
    }

    public func doAsk(_ text: String) async throws -> DoPreview {
        let data = try await connection.call("do.ask", params: Params().with("text", text).data())
        return try RPCCoding.decoder().decode(DoPreview.self, from: data)
    }

    public func doRun(_ id: String, confirm: Int?) async throws -> DoRunResult {
        let data = try await connection.call("do.run", params: Params().with("id", id).with("confirm", confirm).data())
        return try RPCCoding.decoder().decode(DoRunResult.self, from: data)
    }

    public func doGet(_ id: String) async throws -> DoOperation {
        let data = try await connection.call("do.get", params: Params().with("id", id).data())
        return try RPCCoding.decoder().decode(DoOperation.self, from: data)
    }

    public func doUndo(_ id: String) async throws {
        _ = try await connection.call("do.undo", params: Params().with("id", id).data())
    }
}

public enum AskIntent {
    /// First words that make typed text an instruction (English and French).
    static let verbs: Set<String> = [
        "archive", "archiver", "delete", "trash", "remove", "supprime", "supprimer", "efface",
        "move", "deplace", "deplacer", "mark", "marque", "marquer", "label", "tag", "file", "classe", "range",
        "put", "mets", "snooze", "later", "postpone", "add", "ajoute", "clean", "clear", "nettoie", "sort",
        "trie", "set", "done", "finish", "unlabel", "untag", "read", "unread",
    ]

    /// Typed text reads as an instruction: it starts with an imperative verb and says
    /// more than the verb, or it has three words or more and nothing else matched.
    public static func isInstruction(_ text: String, otherMatches: Bool) -> Bool {
        let words = CommandMatch.words(text)
        guard let first = words.first, words.count >= 2 else { return false }
        if verbs.contains(first) { return true }
        return !otherMatches && words.count >= 3
    }

    /// The typed confirmation of a delete: the count, with or without separators.
    public static func confirms(_ typed: String, total: Int) -> Bool {
        let digits = typed.filter(\.isNumber)
        return !digits.isEmpty && Int(digits) == total
    }

    /// 1842 → "1,842", as the daemon writes counts.
    public static func count(_ n: Int) -> String {
        let f = NumberFormatter()
        f.numberStyle = .decimal
        f.groupingSeparator = ","
        f.usesGroupingSeparator = true
        return f.string(from: NSNumber(value: n)) ?? "\(n)"
    }
}
