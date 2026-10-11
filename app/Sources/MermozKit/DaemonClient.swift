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

    public func counts() async throws -> Counts { try await call("counts") }

    public func threads(status: [String]? = nil, project: String? = nil, accountId: String? = nil,
                        labelId: String? = nil, priority: [Int]? = nil, order: String? = nil, reverse: Bool = false,
                        limit: Int = 300, offset: Int = 0) async throws -> ThreadPage {
        try await call("threads.list", Params()
            .with("status", status).with("project", project).with("account_id", accountId).with("label_id", labelId).with("order", order)
            .with("priority", priority).with("reverse", reverse).with("limit", limit).with("offset", offset))
    }

    public func thread(_ id: String) async throws -> ThreadDetail { try await call("threads.get", Params().with("id", id)) }

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

    public func acceptSuggestions(_ ids: [String]) async throws {
        try await callVoid("threads.accept_suggestions", Params().with("ids", ids))
    }

    public func router() async throws -> RouterState { try await call("router.get") }

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

    public func html(messageId: String, full: Bool = false) async throws -> String {
        try await call("messages.html", Params().with("id", messageId).with("full", full), as: HTMLBody.self).html
    }

    public func search(_ query: String, limit: Int = 50) async throws -> [ThreadSummary] {
        try await call("search", Params().with("query", query).with("limit", limit), as: SearchResult.self).threads
    }

    public func attachment(_ id: String) async throws -> AttachmentFile { try await call("attachments.get", Params().with("id", id)) }

    public func drafts(state: String? = nil) async throws -> [Draft] { try await call("drafts.list", Params().with("state", state)) }
    public func draft(_ id: String) async throws -> Draft { try await call("drafts.get", Params().with("id", id)) }

    public func createDraft(accountId: String?, threadId: String?, to: [String], cc: [String],
                            subject: String?, body: String) async throws -> Draft {
        try await call("drafts.create", Params()
            .with("account_id", accountId).with("thread_id", threadId)
            .with("to", to).with("cc", cc).with("subject", subject).with("body", body))
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
    public func aiClean(thread id: String, missingOnly: Bool = false) async throws -> CleanResult {
        try await call("agent.clean", Params().with("id", id).with("missing_only", missingOnly))
    }

    public func aiTriage(status: String, limit: Int = 40) async throws -> TriageResult {
        try await call("agent.triage", Params().with("status", status).with("limit", limit))
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
    public func subscribe() async throws { try await callVoid("subscribe") }
}
