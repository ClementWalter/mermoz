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
    /// Signed in through an older Google setup: mail still syncs, but the user should sign in again.
    public var needsReconnect: Bool?

    public static func == (a: Account, b: Account) -> Bool {
        a.id == b.id && a.sync == b.sync && a.email == b.email && a.needsReconnect == b.needsReconnect
    }
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
    /// Names of the user's Gmail labels on the thread; a thread can be in several projects.
    public var projects: [String]
    public var due: String?
    public var summary: String?
    public var nextAction: String?
    /// The AI's pending prediction, until the thread is read or decided.
    public var suggestion: Suggestion?
    /// The conversation this Gmail thread leads or belongs to; optional so older daemons decode.
    public var conversationId: String?
    /// "person", "feed" (automated sender) or "list" (mailing list).
    public var conversationKind: String?
    /// Gmail threads in the conversation, this one included.
    public var conversationThreads: Int?

    /// More than one Gmail thread reads as this one row.
    public var groupsThreads: Bool { (conversationThreads ?? 1) > 1 }

    public init(id: String, number: String, accountId: String, subject: String, snippet: String,
                participants: [String], lastFrom: String, lastDate: Int64, unread: Bool,
                messageCount: Int, hasAttachments: Bool, status: String, priority: Int,
                projects: [String] = [], due: String? = nil, summary: String? = nil, nextAction: String? = nil,
                suggestion: Suggestion? = nil) {
        self.id = id; self.number = number; self.accountId = accountId; self.subject = subject
        self.snippet = snippet; self.participants = participants; self.lastFrom = lastFrom
        self.lastDate = lastDate; self.unread = unread; self.messageCount = messageCount
        self.hasAttachments = hasAttachments; self.status = status; self.priority = priority
        self.projects = projects; self.due = due; self.summary = summary; self.nextAction = nextAction
        self.suggestion = suggestion
    }

    public func hash(into h: inout Hasher) { h.combine(id) }
}

/// Where the AI would put a thread; fields it was unsure about are nil or empty.
public struct Suggestion: Codable, Sendable, Equatable, Hashable {
    public var status: String?
    public var statusConfidence: Double?
    public var projects: [String]
    public var projectConfidence: Double?
    public var priority: Int?
    /// Probability the mail was sent by a machine (newsletter, receipt, notification).
    public var automated: Double?

    public init(status: String? = nil, projects: [String] = [], priority: Int? = nil, automated: Double? = nil) {
        self.status = status; self.projects = projects; self.priority = priority; self.automated = automated
    }

    /// Nothing worth showing.
    public var isEmpty: Bool { status == nil && projects.isEmpty }
}

/// Instant sorting state, from `router.get`: the top-level fields describe the
/// provider in use.
public struct RouterState: Codable, Sendable, Equatable {
    public var configured: Bool
    public var error: String?
    public var model: String
    /// Background sorting allowance per day, and how much of it today used.
    public var dailyCap: Int?
    public var callsToday: Int?
    /// Id of the provider in use ("typesafe", "subq"); nil from older daemons.
    public var provider: String?
    public var providerName: String?
    /// Every provider with its own key, last error and calls today.
    public var providers: [RouterProvider]?
    /// The other provider, when the one in use ran out of credits.
    public var offer: ProviderOffer?

    public init(configured: Bool = false, error: String? = nil, model: String = "",
                dailyCap: Int? = nil, callsToday: Int? = nil, provider: String? = nil,
                providerName: String? = nil, providers: [RouterProvider]? = nil,
                offer: ProviderOffer? = nil) {
        self.configured = configured; self.error = error; self.model = model
        self.dailyCap = dailyCap; self.callsToday = callsToday
        self.provider = provider; self.providerName = providerName
        self.providers = providers; self.offer = offer
    }

    /// Whether any provider holds a key.
    public var anyKey: Bool { configured || (providers ?? []).contains { $0.configured } }
}

/// One instant sorting provider in `router.get`.
public struct RouterProvider: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var name: String
    public var model: String
    public var configured: Bool
    public var error: String?
    public var paused: String?
    public var callsToday: Int?

    public init(id: String, name: String, model: String = "", configured: Bool = false,
                error: String? = nil, paused: String? = nil, callsToday: Int? = nil) {
        self.id = id; self.name = name; self.model = model; self.configured = configured
        self.error = error; self.paused = paused; self.callsToday = callsToday
    }
}

/// "TypeSafe credits ran out. Use SubQ instead?": a switch the user may accept.
public struct ProviderOffer: Codable, Sendable, Equatable {
    public var provider: String
    public var name: String
    public var message: String

    public init(provider: String, name: String, message: String) {
        self.provider = provider; self.name = name; self.message = message
    }
}

public struct RouterTest: Codable, Sendable {
    public var ok: Bool
    public var ms: Int
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

    /// The sender's domain ("imaginecurve.com"), to tell apart generic names like "complaints".
    public var domain: String { email.split(separator: "@").last.map(String.init) ?? "" }
}

public struct Attachment: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var id: String
    public var filename: String
    public var mime: String
    public var size: Int64
    /// An image placed in the body (shown there), not a file sent along.
    public var inline: Bool?
    /// Decoration (signature logo, social icon, banner, tracking pixel): never a photo.
    public var decorative: Bool?

    public init(id: String, filename: String, mime: String, size: Int64, inline: Bool? = nil,
                decorative: Bool? = nil) {
        self.id = id; self.filename = filename; self.mime = mime; self.size = size; self.inline = inline
        self.decorative = decorative
    }
}

/// A reaction (Outlook/Teams "[heart] Nora reacted to your message") to another message.
public struct Reaction: Codable, Sendable, Equatable, Hashable {
    /// "❤️", "👍", "😆", "😮", "😢", "🎉" or "😠".
    public var emoji: String
    /// The message reacted to, when it is in this conversation.
    public var targetMessageId: String?

    public init(emoji: String, targetMessageId: String? = nil) {
        self.emoji = emoji; self.targetMessageId = targetMessageId
    }
}

/// The email exactly as received, from `messages.source`.
public struct MessageSource: Codable, Sendable, Equatable {
    public var source: String
    public var truncated: Bool
}

public struct Message: Codable, Sendable, Identifiable, Equatable {
    public var id: String
    /// Its Gmail thread, inside a conversation of several.
    public var threadId: String?
    public var from: Address
    public var to: [Address]
    public var cc: [Address]
    /// Blind copies, known only on messages the user sent.
    public var bcc: [Address]?
    /// Where the sender wants replies (`Reply-To`), when not at `from`.
    public var replyTo: [Address]? = nil
    public var date: Int64
    public var subject: String
    public var bodyText: String
    /// What the bubble shows, already clean (only the sender's new words, LF line ends,
    /// never empty when the message has text); optional so older daemons still decode.
    public var bodyNew: String?
    public var hasQuoted: Bool?
    /// A signature, mobile footer or disclaimer was folded out of `bodyNew`.
    public var hasSignature: Bool? = nil
    /// The message forwards an email: `bodyNew` is the note, then the forwarded text.
    public var forwarded: Bool? = nil
    /// Who wrote the forwarded email.
    public var forwardedFrom: String? = nil
    /// The message adds no words of its own: `bodyNew` is the text it quotes.
    public var quotedOnly: Bool? = nil
    /// The message is only a reaction to another one (`bodyNew` is then its emoji).
    public var reaction: Reaction? = nil
    /// Reading version written by the built-in agent.
    public var bodyClean: String?
    public var hasHtml: Bool
    public var isFromMe: Bool
    public var attachments: [Attachment]
    /// Sender verification by Gmail: "pass", "fail", "none", nil while unchecked.
    public var auth: String?

    /// What a reader should see first: only what this message adds to the thread.
    public var displayBody: String {
        if let n = bodyNew, !n.isEmpty { return n }
        return bodyText
    }
}

public struct ThreadDetail: Codable, Sendable, Equatable {
    public var thread: ThreadSummary
    public var messages: [Message]
    /// Present when the whole conversation was asked for.
    public var conversation: Conversation?
    /// Earlier messages of the conversation left out (asked for its latest ones only).
    public var earlier: Int? = nil
    /// Who a reply-all goes to, as the daemon worked it out (Reply-To honoured).
    public var reply: ReplyAddressing? = nil
}

/// The recipients a reply starts with, and whether any of them reads nothing.
public struct ReplyAddressing: Codable, Sendable, Equatable {
    public var to: [Address]
    public var cc: [Address]
    /// Addresses in `to` that don't accept replies.
    public var noReply: [String]
    /// A person's address found in the mail, offered instead of a no-reply one.
    public var suggestion: Address?

    public init(to: [Address], cc: [Address], noReply: [String] = [], suggestion: Address? = nil) {
        self.to = to; self.cc = cc; self.noReply = noReply; self.suggestion = suggestion
    }
}

extension ThreadDetail {
    /// What the pane shows of a card until its messages arrive: the header and subject.
    public static func placeholder(_ thread: ThreadSummary) -> ThreadDetail {
        ThreadDetail(thread: thread, messages: [])
    }

    /// The conversation once read: its lead as the daemon left it (maybe filed by the
    /// AI's suggestion) and every Gmail thread in it read, as `threads.mark_read` does.
    public func afterReading(_ lead: ThreadSummary) -> ThreadDetail {
        var d = self
        d.thread = lead
        if var c = d.conversation {
            c.threads = c.threads.map { t in
                if t.id == lead.id { return lead }
                var t = t
                t.unread = false
                return t
            }
            d.conversation = c
        }
        return d
    }
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

public struct CleanResult: Codable, Sendable { public var cleaned: Int }

/// Progress of a background AI job.
public struct JobState: Codable, Sendable, Equatable {
    public var running: Bool
    public var done: Int
    public var total: Int
    public var error: String?

    public init(running: Bool = false, done: Int = 0, total: Int = 0, error: String? = nil) {
        self.running = running; self.done = done; self.total = total; self.error = error
    }
}

public struct JobStart: Codable, Sendable { public var total: Int }

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

/// The last Google mail sign-in. `problem` is Google's refusal in plain words;
/// `appClientId` is what a Workspace admin allow-lists to trust Mermoz.
public struct GmailSignIn: Codable, Sendable, Equatable {
    public var running: Bool
    public var error: String?
    public var problem: SignInProblem?
    public var appClientId: String?

    public init(running: Bool = false, error: String? = nil, problem: SignInProblem? = nil, appClientId: String? = nil) {
        self.running = running; self.error = error; self.problem = problem; self.appClientId = appClientId
    }
}

public struct SignInProblem: Codable, Sendable, Equatable {
    /// admin_policy, unverified, org_internal, app_blocked or declined.
    public var kind: String
    public var message: String
    /// The client the refused consent used.
    public var clientId: String
    /// That client is the user's own, not Mermoz's.
    public var custom: Bool
}

/// A Google sign-in client the user brings for one account: the downloaded JSON
/// file's contents, or the pasted ID and secret.
public enum OwnGoogleClient: Sendable, Equatable {
    case file(String)
    case ids(clientId: String, secret: String)
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
    /// Conversations (threads) with them, whoever wrote.
    public var conversations: Int?
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
    /// Open feeds and lists ("feed", "list"), counted per conversation.
    public var kind: [String: Int]?

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

/// A message Gmail's own search found, for mail Mermoz doesn't hold.
public struct GmailHit: Codable, Sendable, Identifiable, Equatable {
    public var accountId: String
    public var id: String
    public var threadId: String
    public var from: String
    public var subject: String
    public var date: String
    /// Already stored locally; otherwise it shows "on Gmail" until pulled in.
    public var inMermoz: Bool
}

public struct GmailSearch: Codable, Sendable {
    public var results: [GmailHit]
}

public struct PulledThreads: Codable, Sendable {
    public var threads: [String]
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
    public var attachments: [DraftAttachment]
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
    /// The message has remote images (kept only when they were asked for).
    public var remoteImages: Bool?
}

/// Notification pushed on a `subscribe` connection.
public enum DaemonEvent: Sendable, Equatable {
    case threadsChanged(ids: [String])
    case accountsChanged
    case syncProgress(accountId: String, synced: Int, total: Int)
    case draftsChanged
    case contactsChanged
    case providersChanged
    /// Background sorting progressed: column suggestions or progress changed.
    case suggestionsChanged
    /// Feed judgments or card words changed.
    case feedChanged
    /// The answer streaming into a thread's chat so far.
    case chatDelta(threadId: String, text: String)
    /// An account's Drive connection, a file pick or a saved attachment changed.
    case driveChanged(accountId: String)
    case unknown(kind: String)
}

/// Statuses the daemon treats as closed; used to scope "open" views.
public let closedStatusKeys: Set<String> = ["done", "archive", "deleted"]
