/// Typed wrapper over the mermozd protocol methods.
import Foundation

/// Where the daemon socket lives; `MERMOZ_HOME` overrides it for tests and dev setups.
public enum DaemonPaths {
    public static var home: URL {
        if let h = ProcessInfo.processInfo.environment["MERMOZ_HOME"], !h.isEmpty {
            return URL(fileURLWithPath: h)
        }
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("Mermoz")
    }

    /// `MERMOZ_SOCKET` overrides the path, since macOS caps socket paths at 104 bytes.
    public static var socket: String {
        if let s = ProcessInfo.processInfo.environment["MERMOZ_SOCKET"], !s.isEmpty { return s }
        return home.appendingPathComponent("mermozd.sock").path
    }
}

/// Builds `params` objects; keys are written exactly as the protocol spells them (snake_case).
public struct Params: Encodable, Sendable {
    var fields: [String: JSONValue] = [:]

    public init() {}

    public func with<T: Encodable & Sendable>(_ key: String, _ value: T?) -> Params {
        var p = self
        if let value { p.fields[key] = JSONValue(value) }
        return p
    }

    /// Explicit JSON null, used to clear optional thread fields.
    public func null(_ key: String) -> Params {
        var p = self
        p.fields[key] = .null
        return p
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: DynamicKey.self)
        for (k, v) in fields { try c.encode(v, forKey: DynamicKey(k)) }
    }

    public func data() -> Data { (try? JSONEncoder().encode(self)) ?? Data("{}".utf8) }
}

struct DynamicKey: CodingKey {
    var stringValue: String
    var intValue: Int? { nil }
    init(_ s: String) { stringValue = s }
    init?(stringValue: String) { self.stringValue = stringValue }
    init?(intValue: Int) { nil }
}

/// Patch applied by `threads.update`; `.clear` sends an explicit null.
public enum Patch<T: Encodable & Sendable>: Sendable {
    case keep
    case set(T)
    case clear
}

public final class DaemonClient: Sendable {
    public let connection: Connection

    public init(connection: Connection) { self.connection = connection }

    private func call<R: Decodable>(_ method: String, _ params: Params = Params(), as: R.Type = R.self) async throws -> R {
        let data = try await connection.call(method, params: params.data())
        return try RPCCoding.decoder().decode(R.self, from: data)
    }

    private func callVoid(_ method: String, _ params: Params = Params()) async throws {
        _ = try await connection.call(method, params: params.data())
    }

    public func hello() async throws -> String {
        struct H: Decodable { var version: String }
        return try await call("hello", as: H.self).version
    }

    public func accounts() async throws -> [Account] { try await call("accounts.list") }
    public func addGmail() async throws -> AuthStart { try await call("accounts.add_gmail") }
    public func removeAccount(_ id: String) async throws { try await callVoid("accounts.remove", Params().with("id", id)) }
    public func statuses() async throws -> [Status] { try await call("statuses.list") }
    public func projects() async throws -> [Project] { try await call("projects.list") }

    public func reorderStatuses(_ keys: [String]) async throws -> [Status] {
        try await call("statuses.reorder", Params().with("keys", keys))
    }

    public func labels() async throws -> [MailLabel] { try await call("labels.list") }

    public func updateProject(_ id: String, name: String? = nil, color: String? = nil) async throws -> Project {
        try await call("projects.update", Params().with("id", id).with("name", name).with("color", color))
    }

    public func deleteProject(_ id: String) async throws { try await callVoid("projects.delete", Params().with("id", id)) }

    public func createProject(name: String, color: String?) async throws -> Project {
        try await call("projects.create", Params().with("name", name).with("color", color))
    }

    /// Open conversations per status, project, account and priority (threads with `conversations: false`).
    public func counts(conversations: Bool = true) async throws -> Counts {
        try await call("counts", Params().with("conversations", conversations))
    }

    public func threads(status: [String]? = nil, project: String? = nil, accountId: String? = nil,
                        labelId: String? = nil, priority: [Int]? = nil, order: String? = nil, reverse: Bool = false,
                        limit: Int = 300, offset: Int = 0,
                        conversations: Bool = true, kind: [String]? = nil) async throws -> ThreadPage {
        // `conversations`: one row per conversation, the thread leading it.
        let p = Params()
            .with("status", status).with("project", project).with("account_id", accountId).with("label_id", labelId).with("order", order)
            .with("priority", priority).with("reverse", reverse).with("limit", limit).with("offset", offset)
            .with("conversations", conversations).with("kind", kind)
        return try await call("threads.list", p)
    }

    /// A thread with every message of its conversation (or only its own with `conversation: false`).
    public func thread(_ id: String, conversation: Bool = true) async throws -> ThreadDetail {
        try await call("threads.get", Params().with("id", id).with("conversation", conversation))
    }

    /// Takes a thread out of its conversation for good.
    public func splitConversation(_ threadId: String) async throws {
        try await callVoid("conversations.split", Params().with("id", threadId))
    }

    /// Joins the conversations of these threads into one.
    public func mergeConversations(_ threadIds: [String]) async throws {
        try await callVoid("conversations.merge", Params().with("ids", threadIds))
    }

    /// Files shared anywhere, newest first.
    public func files(query: String? = nil, limit: Int = 300) async throws -> [SharedFile] {
        try await call("files.list", Params().with("query", query).with("limit", limit))
    }

    public func updateThreads(ids: [String], status: String? = nil, priority: Int? = nil,
                              addProjects: [String] = [], removeProjects: [String] = [],
                              due: Patch<String> = .keep) async throws {
        var p = Params().with("ids", ids).with("status", status).with("priority", priority)
        if !addProjects.isEmpty { p = p.with("add_projects", addProjects) }
        if !removeProjects.isEmpty { p = p.with("remove_projects", removeProjects) }
        switch due {
        case .keep: break
        case let .set(v): p = p.with("due", v)
        case .clear: p = p.null("due")
        }
        try await callVoid("threads.update", p)
    }

    /// Turns suggestions down; the threads stay put and are not suggested again.
    public func rejectSuggestions(_ ids: [String]) async throws {
        try await callVoid("threads.reject_suggestions", Params().with("ids", ids))
    }

    /// Undoes `rejectSuggestions`.
    public func reopenSuggestions(_ ids: [String]) async throws {
        try await callVoid("threads.reopen_suggestions", Params().with("ids", ids))
    }

    public func acceptSuggestions(_ ids: [String]) async throws {
        try await callVoid("threads.accept_suggestions", Params().with("ids", ids))
    }

    public func router() async throws -> RouterState { try await call("router.get") }

    /// Sorts every column again in the background; nothing moves until accepted.
    /// What waits in the Sort queue (of `statuses`, else every open column) and what it learned.
    public func sortState(statuses: [String] = []) async throws -> SortInfo {
        try await call("sort.state", Params().with("statuses", statuses))
    }

    /// The next cards; the daemon asks the AI for these few threads only.
    public func sortNext(statuses: [String], exclude: [String], count: Int = SortQueue.lookahead) async throws -> SortBatch {
        try await call("sort.next", Params().with("statuses", statuses).with("exclude", exclude).with("count", count))
    }

    /// Answers a card: "yes", "no", or "change" with the destination the user chose.
    public func sortAnswer(_ id: String, answer: String, status: String? = nil, projects: [String] = []) async throws -> SortAnswerResult {
        try await call("sort.answer", Params().with("id", id).with("answer", answer).with("status", status).with("projects", projects))
    }

    /// Takes an answer back.
    public func sortUndo(_ token: SortUndoToken) async throws {
        try await callVoid("sort.undo", Params().with("undo", token))
    }

    /// Clears every suggestion on read threads nobody reviewed; returns how many.
    public func clearSuggestions() async throws -> Int {
        try await call("suggestions.clear", as: Cleared.self).cleared
    }

    public func setDailyCap(_ cap: Int) async throws -> RouterState {
        try await call("router.set", Params().with("daily_cap", cap))
    }

    /// `nil` removes the key and stops instant sorting.
    public func setRouterKey(_ key: String?) async throws -> RouterState {
        let p = key.map { Params().with("api_key", $0) } ?? Params().null("api_key")
        return try await call("router.set", p)
    }

    public func testRouter(key: String?) async throws -> RouterTest {
        try await call("router.test", Params().with("api_key", key))
    }

    public func markRead(_ id: String, read: Bool = true) async throws {
        try await callVoid("threads.mark_read", Params().with("id", id).with("read", read))
    }

    /// `images`: keep remote images (they can tell the sender the mail was opened).
    public func html(messageId: String, full: Bool = false, images: Bool = false) async throws -> HTMLBody {
        try await call("messages.html", Params().with("id", messageId).with("full", full).with("images", images), as: HTMLBody.self)
    }

    public func search(_ query: String, limit: Int = 50) async throws -> [ThreadSummary] {
        try await call("search", Params().with("query", query).with("limit", limit), as: SearchResult.self).threads
    }

    public func attachment(_ id: String) async throws -> AttachmentFile { try await call("attachments.get", Params().with("id", id)) }

    public func drafts(state: String? = nil) async throws -> [Draft] { try await call("drafts.list", Params().with("state", state)) }
    public func draft(_ id: String) async throws -> Draft { try await call("drafts.get", Params().with("id", id)) }

    /// With attachments, the draft is created with all of them or not at all.
    public func createDraft(accountId: String?, threadId: String?, to: [String], cc: [String],
                            subject: String?, body: String, attachments: [AttachSpec] = []) async throws -> Draft {
        try await call("drafts.create", Params()
            .with("account_id", accountId).with("thread_id", threadId)
            .with("to", to).with("cc", cc).with("subject", subject).with("body", body)
            .with("attachments", attachments.isEmpty ? nil : attachments))
    }

    public func attach(draft id: String, _ attachments: [AttachSpec]) async throws -> Draft {
        try await call("drafts.attach", Params().with("id", id).with("attachments", attachments))
    }

    public func detach(draft id: String, attachmentId: String) async throws -> Draft {
        try await call("drafts.detach", Params().with("id", id).with("attachment_id", attachmentId))
    }

    public func updateDraft(_ id: String, to: [String]?, cc: [String]?, subject: String?, body: String?) async throws -> Draft {
        try await call("drafts.update", Params().with("id", id).with("to", to).with("cc", cc)
            .with("subject", subject).with("body", body))
    }

    public func deleteDraft(_ id: String) async throws { try await callVoid("drafts.delete", Params().with("id", id)) }
    public func sendDraft(_ id: String) async throws -> SendResult { try await call("drafts.send", Params().with("id", id)) }
    public func approve(_ id: String) async throws -> SendResult { try await call("review.approve", Params().with("id", id)) }
    public func reject(_ id: String) async throws { try await callVoid("review.reject", Params().with("id", id)) }
    public func settings() async throws -> AppSettings { try await call("settings.get") }

    public func providers() async throws -> [Provider] { try await call("providers.list") }

    public func saveProvider(id: String?, name: String, kind: String, baseUrl: String?, model: String, apiKey: String?) async throws -> Provider {
        try await call("providers.save", Params().with("id", id).with("name", name).with("kind", kind)
            .with("base_url", baseUrl).with("model", model).with("api_key", apiKey))
    }

    public func peopleClean() async throws -> JobStart { try await call("agent.people_clean") }

    public func peopleStatus() async throws -> JobState { try await call("agent.people_status") }

    /// `missingOnly`: clean only messages without a stored version (never re-pays).
    /// `message`: clean only that message of the thread.
    public func aiClean(thread id: String, message: String? = nil, missingOnly: Bool = false) async throws -> CleanResult {
        try await call("agent.clean", Params().with("id", id).with("message_id", message).with("missing_only", missingOnly))
    }


    public func loginAnthropic() async throws { try await callVoid("providers.login_anthropic") }

    public func loginChatGPT() async throws -> AuthStart { try await call("providers.login_chatgpt") }

    public func providerModels(_ id: String) async throws -> [ProviderModel] {
        try await call("providers.models", Params().with("id", id))
    }

    public func loginStatus() async throws -> LoginState { try await call("providers.login_status") }

    public func deleteProvider(_ id: String) async throws { try await callVoid("providers.delete", Params().with("id", id)) }

    public func testProvider(_ id: String) async throws -> ProviderTest { try await call("providers.test", Params().with("id", id)) }

    public func agentState(account: String) async throws -> AgentState {
        try await call("accounts.agent.get", Params().with("account_id", account))
    }

    public func setAgent(account: String, providerIds: [String]? = nil, digestEnabled: Bool? = nil) async throws {
        try await callVoid("accounts.agent.set", Params().with("account_id", account).with("provider_ids", providerIds)
            .with("digest_enabled", digestEnabled))
    }

    public func contacts(query: String? = nil, limit: Int = 500) async throws -> [Contact] {
        try await call("contacts.list", Params().with("query", query).with("limit", limit))
    }

    public func contact(_ email: String) async throws -> ContactDetail { try await call("contacts.get", Params().with("email", email)) }

    public func setAgentsMaySend(_ v: Bool) async throws {
        try await callVoid("settings.set", Params().with("agents_may_send", v))
    }

    public func feedback() async throws -> [FeedbackEntry] { try await call("feedback.list") }

    public func chat(thread id: String) async throws -> ChatHistory { try await call("chat.get", Params().with("thread_id", id)) }

    /// Returns the AI's answer once complete; `chat_delta` events stream it meanwhile.
    public func sendChat(thread id: String, text: String, research: Bool) async throws -> ChatMessage {
        try await call("chat.send", Params().with("thread_id", id).with("text", text).with("research", research))
    }

    public func clearChat(thread id: String) async throws { try await callVoid("chat.clear", Params().with("thread_id", id)) }

    /// The Feed for a board scope (a project and/or an account).
    public func feed(project: String?, accountId: String?) async throws -> Feed {
        try await call("feed.list", Params().with("project", project).with("account_id", accountId))
    }

    /// Asks the AI to write these cards' words; they arrive with `feed_changed`.
    public func feedTexts(_ ids: [String]) async throws {
        try await callVoid("feed.texts", Params().with("ids", ids))
    }

    /// Later: hides threads from the feed until `until` (or new mail).
    public func snooze(_ ids: [String], until: Date) async throws {
        try await callVoid("feed.snooze", Params().with("ids", ids).with("until", FeedLater.millis(until)))
    }

    public func unsnooze(_ ids: [String]) async throws { try await callVoid("feed.unsnooze", Params().with("ids", ids)) }

    /// The user archived these automated mails from a card: a yes to "Archive" for their senders.
    public func learnArchive(_ ids: [String]) async throws { try await callVoid("feed.learn_archive", Params().with("ids", ids)) }
    public func subscribe() async throws { try await callVoid("subscribe") }
}


/// `suggestions.clear`.
struct Cleared: Decodable, Sendable { var cleared: Int }
