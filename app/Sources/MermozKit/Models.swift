/// Protocol objects exchanged with mermozd (see docs/protocol.md).
import Foundation

public struct SyncInfo: Codable, Sendable, Equatable {
    public var state: String
    public var synced: Int
    public var total: Int
    public var error: String?

    public init(state: String, synced: Int, total: Int, error: String? = nil) {
        self.state = state
        self.synced = synced
        self.total = total
        self.error = error
    }
}

public struct Account: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var id: String
    public var email: String
    public var provider: String
    public var sync: SyncInfo

    public static func == (a: Account, b: Account) -> Bool { a.id == b.id && a.sync == b.sync && a.email == b.email }
    public func hash(into h: inout Hasher) { h.combine(id) }
}

public struct ThreadSummary: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var id: String
    public var number: String
    public var accountId: String
    public var subject: String
    public var snippet: String
    public var participants: [String]
    public var lastFrom: String
    public var lastDate: Int64
    public var unread: Bool
    public var messageCount: Int
    public var hasAttachments: Bool
    public var status: String
    public var priority: Int
    public var projectId: String?
    public var due: String?
    public var summary: String?
    public var nextAction: String?

    public init(id: String, number: String, accountId: String, subject: String, snippet: String,
                participants: [String], lastFrom: String, lastDate: Int64, unread: Bool,
                messageCount: Int, hasAttachments: Bool, status: String, priority: Int,
                projectId: String? = nil, due: String? = nil, summary: String? = nil, nextAction: String? = nil) {
        self.id = id; self.number = number; self.accountId = accountId; self.subject = subject
        self.snippet = snippet; self.participants = participants; self.lastFrom = lastFrom
        self.lastDate = lastDate; self.unread = unread; self.messageCount = messageCount
        self.hasAttachments = hasAttachments; self.status = status; self.priority = priority
        self.projectId = projectId; self.due = due; self.summary = summary; self.nextAction = nextAction
    }

    public func hash(into h: inout Hasher) { h.combine(id) }
}

public struct Address: Codable, Sendable, Equatable, Hashable {
    public var name: String
    public var email: String

    public init(name: String, email: String) {
        self.name = name
        self.email = email
    }

    /// Display name falling back to the address, since many senders omit a name.
    public var display: String { name.isEmpty ? email : name }
}

public struct Attachment: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var id: String
    public var filename: String
    public var mime: String
    public var size: Int64
}

public struct Message: Codable, Sendable, Identifiable, Equatable {
    public var id: String
    public var from: Address
    public var to: [Address]
    public var cc: [Address]
    public var date: Int64
    public var subject: String
    public var bodyText: String
    /// Body without quoted history; optional so older daemons still decode.
    public var bodyNew: String?
    public var hasQuoted: Bool?
    /// Reading version written by the built-in agent.
    public var bodyClean: String?
    public var hasHtml: Bool
    public var isFromMe: Bool
    public var attachments: [Attachment]

    /// What a reader should see first: only what this message adds to the thread.
    public var displayBody: String {
        if let n = bodyNew, !n.isEmpty { return n }
        return bodyText
    }
}

public struct ThreadDetail: Codable, Sendable, Equatable {
    public var thread: ThreadSummary
    public var messages: [Message]
}

public struct Status: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var key: String
    public var name: String
    public var order: Int
    public var color: String
    public var id: String { key }

    public init(key: String, name: String, order: Int, color: String) {
        self.key = key; self.name = name; self.order = order; self.color = color
    }
}

/// An AI provider; its API key stays in the daemon's Keychain, only `hasKey` is visible.
public struct Provider: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var id: String
    public var name: String
    public var kind: String
    public var baseUrl: String?
    public var model: String
    public var hasKey: Bool?
    /// "key" or "oauth" (signed in with Anthropic).
    public var auth: String?
}

/// Built-in agent state for one account.
public struct AgentState: Codable, Sendable, Equatable {
    public var providerIds: [String]
    public var digestEnabled: Bool
    public var done: Int
    public var pending: Int
    public var failed: Int
    public var error: String?
}

/// Result of asking the AI to triage a status column.
public struct TriageResult: Codable, Sendable, Equatable, Identifiable {
    public struct Move: Codable, Sendable, Equatable, Identifiable {
        public var number: String
        public var subject: String
        public var from: String
        public var to: String
        public var priority: Int
        public var reason: String
        public var id: String { number }
    }
    public var examined: Int
    public var moved: Int
    public var moves: [Move]
    public var id: String { moves.map(\.number).joined(separator: ",") + "#\(examined)" }
}

public struct CleanResult: Codable, Sendable { public var cleaned: Int }

public struct ProviderModel: Codable, Sendable, Equatable, Identifiable, Hashable {
    public var id: String
    public var name: String
}

public struct LoginState: Codable, Sendable, Equatable {
    public var running: Bool
    public var error: String?

    public init(running: Bool, error: String?) {
        self.running = running; self.error = error
    }
}

public struct ProviderTest: Codable, Sendable, Equatable {
    public var ok: Bool
    public var cleanBody: String?
    public var error: String?
}

/// Directory entry built from email signatures.
public struct Contact: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var email: String
    public var name: String
    public var title: String
    public var company: String
    public var phones: [String]
    public var address: String
    public var website: String
    public var socials: [String]
    public var lastSeen: Int64
    /// Every address this person writes from (one entry per person).
    public var emails: [String]?
    /// Messages exchanged with them.
    public var messages: Int?
    public var id: String { email }

    public var display: String { name.isEmpty ? email : name }
    public var role: String { [title, company].filter { !$0.isEmpty }.joined(separator: " · ") }
}

public struct ContactDetail: Codable, Sendable, Equatable {
    public var contact: Contact
    public var threads: [ThreadSummary]
}

/// A Gmail label the user files mail into ("mailbox"); `id` is unique within its account.
public struct MailLabel: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var accountId: String
    public var id: String
    public var name: String
    public var count: Int

    /// Nesting depth from Gmail's "Parent/Child" naming.
    public var depth: Int { name.split(separator: "/").count - 1 }
    public var leaf: String { name.split(separator: "/").last.map(String.init) ?? name }
}

public struct Project: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var id: String
    public var name: String
    public var color: String
}

public struct Counts: Codable, Sendable, Equatable {
    public var status: [String: Int]
    public var project: [String: Int]
    public var account: [String: Int]
    /// Open threads per priority ("0"…"4"); optional so older daemons still decode.
    public var priority: [String: Int]?

    public init(status: [String: Int] = [:], project: [String: Int] = [:], account: [String: Int] = [:]) {
        self.status = status; self.project = project; self.account = account
    }
}

public struct ThreadPage: Codable, Sendable {
    public var threads: [ThreadSummary]
    public var total: Int
}

public struct SearchResult: Codable, Sendable {
    public var threads: [ThreadSummary]
}

public struct Draft: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var id: String
    public var accountId: String
    public var threadId: String?
    public var to: [String]
    public var cc: [String]
    public var subject: String
    public var body: String
    public var state: String
    public var origin: String
    public var created: Int64
    public var updated: Int64
}

public struct AppSettings: Codable, Sendable, Equatable {
    public var agentsMaySend: Bool
}

public struct FeedbackEntry: Codable, Sendable, Identifiable, Equatable {
    public var id: String
    public var text: String
    public var command: String?
    public var agent: String?
    public var created: Int64
}

public struct AttachmentFile: Codable, Sendable {
    public var path: String
    public var filename: String
    public var mime: String
    public var size: Int64
}

public struct AuthStart: Codable, Sendable {
    public var authUrl: String
}

public struct SendResult: Codable, Sendable {
    public var sent: Bool
}

public struct HTMLBody: Codable, Sendable {
    public var html: String
}

/// Notification pushed on a `subscribe` connection.
public enum DaemonEvent: Sendable, Equatable {
    case threadsChanged(ids: [String])
    case accountsChanged
    case syncProgress(accountId: String, synced: Int, total: Int)
    case draftsChanged
    case contactsChanged
    case providersChanged
    case unknown(kind: String)
}

/// Statuses the daemon treats as closed; used to scope "open" views.
public let closedStatusKeys: Set<String> = ["done", "canceled"]
