/// Typed access to the embedded engine, serialized off the main actor for SQLite work.
import Foundation
import Observation

struct MailThread: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let accountId: String
    let subject: String
    let snippet: String
    let lastFrom: String
    let lastDate: Int64
    let unread: Bool
    let messageCount: Int
    let status: String
    let projects: [String]
}

struct MailAddress: Decodable, Sendable {
    let name: String
    let email: String
    var display: String { name.isEmpty ? email : name }
}

struct MailMessage: Decodable, Identifiable, Sendable {
    let id: String
    let from: MailAddress
    let date: Int64
    let bodyText: String
}

struct MailStatus: Decodable, Identifiable, Sendable {
    let key: String
    let name: String
    var id: String { key }
}

struct MailPage: Decodable, Sendable {
    let threads: [MailThread]
    let total: Int?
}

struct MailDetail: Decodable, Sendable {
    let thread: MailThread
    let messages: [MailMessage]
}

struct MailboxError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

private struct Envelope<T: Decodable>: Decodable {
    let result: T?
    let error: String?
}

actor Mailbox {
    private let directory: URL
    // An integer keeps ownership transferable to deinit; only this actor dereferences it.
    private var handleBits: UInt?

    init(directory: URL) { self.directory = directory }

    deinit {
        if let handleBits { mermoz_mobile_close(OpaquePointer(bitPattern: handleBits)) }
    }

    func call<T: Decodable & Sendable>(_ method: String, params: [String: String] = [:]) throws -> T {
        try Task.checkCancellation()
        if handleBits == nil {
            var error: UnsafeMutablePointer<CChar>?
            let handle = directory.path.withCString { mermoz_mobile_open($0, &error) }
            defer { mermoz_mobile_string_free(error) }
            guard let handle else {
                throw MailboxError(message: error.map { String(cString: $0) } ?? "Couldn't open the mailbox.")
            }
            handleBits = UInt(bitPattern: handle)
        }
        let request = try JSONSerialization.data(withJSONObject: ["method": method, "params": params])
        let text = String(decoding: request, as: UTF8.self)
        let pointer = text.withCString { mermoz_mobile_request(OpaquePointer(bitPattern: handleBits!), $0) }
        guard let pointer else { throw MailboxError(message: "The mailbox returned no response.") }
        defer { mermoz_mobile_string_free(pointer) }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let envelope = try decoder.decode(Envelope<T>.self, from: Data(String(cString: pointer).utf8))
        if let error = envelope.error { throw MailboxError(message: error) }
        guard let result = envelope.result else { throw MailboxError(message: "The mailbox response was empty.") }
        return result
    }
}

@MainActor @Observable
final class MailboxModel {
    let mailbox: Mailbox
    var threads: [MailThread] = []
    var statuses: [MailStatus] = []
    var error: String?
    var loading = false
    var revision = 0
    var accounts: [MailAccount] = []
    var accountError: String?
    var connecting = false
    var total = 0
    var feed: MobileFeed?
    var ai: AIProgress?
    var aiError: String?
    private var aiRestored = false
    private let credentials = CredentialStore()
    private let login = GoogleLogin()

    var isDemo: Bool { !accounts.contains { $0.provider == "gmail" && !$0.hidden } }

    init(directory: URL) { mailbox = Mailbox(directory: directory) }

    func load(status: String?, query: String) async {
        loading = true
        defer { if !Task.isCancelled { loading = false } }
        do {
            if statuses.isEmpty { statuses = try await mailbox.call("statuses.list") }
            let page: MailPage
            if query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                page = try await mailbox.call("threads.list", params: status.map { ["status": $0] } ?? [:])
            } else {
                // A short debounce keeps typing responsive without opening stale searches.
                try await Task.sleep(for: .milliseconds(180))
                page = try await mailbox.call("search", params: ["query": query])
            }
            try Task.checkCancellation()
            threads = page.threads
            total = page.total ?? page.threads.count
            error = nil
        } catch is CancellationError {
        } catch {
            if !Task.isCancelled { self.error = error.localizedDescription }
        }
    }

    func resume() async {
        do {
            let accounts: [MailAccount] = try await mailbox.call("accounts.list")
            let known = Set(accounts.map(\.id))
            for credential in try credentials.all() {
                // Keychain entries can outlive an app uninstall and its mailbox database.
                guard known.contains(credential.accountId) else {
                    try credentials.delete(credential.accountId)
                    continue
                }
                do {
                    let _: EmptyResult = try await mailbox.call("accounts.resume", params: [
                        "account_id": credential.accountId, "client_id": credential.clientId,
                        "refresh_token": credential.refreshToken,
                    ])
                } catch { accountError = error.localizedDescription }
            }
            let _: EmptyResult = try await mailbox.call("sync.start")
            await refreshAccounts()
            if !aiRestored {
                if let config = try AIKeychain().load() {
                    // Removed mailboxes must not regain consent from an old Keychain entry.
                    var current = config
                    current.accounts = config.accounts.filter { known.contains($0) }
                    if !current.accounts.isEmpty {
                        let _: EmptyResult = try await mailbox.call("ai.configure", params: ["configuration": try current.engineJSON])
                    }
                }
                aiRestored = true
            }
            ai = try await mailbox.call("ai.status")
            await prepareFeed()
        } catch { accountError = error.localizedDescription }
    }

    func refreshAccounts() async {
        do {
            let fresh: [MailAccount] = try await mailbox.call("accounts.list")
            if fresh != accounts { accounts = fresh; revision += 1 }
            let wasRunning = ai?.running == true
            ai = try await mailbox.call("ai.status")
            if wasRunning || ai?.running == true || feed == nil { await loadFeed() }
        }
        catch { accountError = error.localizedDescription }
    }

    func pause() async {
        do { let _: EmptyResult = try await mailbox.call("sync.pause") }
        catch { accountError = error.localizedDescription }
    }

    func connectGoogle() async {
        guard let clientId = Bundle.main.object(forInfoDictionaryKey: "MERMOZGoogleClientID") as? String,
              !clientId.isEmpty else {
            accountError = "Google sign-in isn't configured in this build."
            return
        }
        connecting = true
        defer { connecting = false }
        do {
            let start: LoginStart = try await mailbox.call("auth.start", params: ["client_id": clientId])
            let callback = try await login.authorize(start)
            let result: LoginResult = try await mailbox.call("auth.finish", params: ["callback": callback])
            // Credentials must survive relaunch before any mailbox import starts.
            try credentials.save(result.credential)
            accountError = nil
            await resume()
            revision += 1
        } catch { accountError = error.localizedDescription }
    }

    func removeAccount(_ account: MailAccount) async {
        do {
            let _: EmptyResult = try await mailbox.call("accounts.remove", params: ["id": account.id])
            try credentials.delete(account.id)
            await refreshAccounts()
            revision += 1
        } catch { accountError = error.localizedDescription }
    }

    func loadMore(status: String?) async {
        do {
            var params = ["offset": String(threads.count)]
            if let status { params["status"] = status }
            let page: MailPage = try await mailbox.call("threads.list", params: params)
            let existing = Set(threads.map(\.id))
            threads += page.threads.filter { !existing.contains($0.id) }
            total = page.total ?? threads.count
        } catch { self.error = error.localizedDescription }
    }

    func loadFeed() async {
        do { feed = try await mailbox.call("feed.list") }
        catch { aiError = error.localizedDescription }
    }

    func prepareFeed() async {
        await loadFeed()
        guard ai?.enabled == true, ai?.running != true, let feed else { return }
        do {
            let ids = Array(feed.cards.flatMap { [$0.thread.id] + $0.members.map(\.id) }.prefix(12))
            let data = try JSONEncoder().encode(ids)
            let _: EmptyResult = try await mailbox.call("feed.refresh", params: ["ids": String(decoding: data, as: UTF8.self)])
            ai = try await mailbox.call("ai.status")
        } catch { aiError = error.localizedDescription }
    }

    func configureAI(_ configuration: AIConfiguration?) async throws {
        let _: EmptyResult = try await mailbox.call("ai.configure", params: ["configuration": try configuration?.engineJSON ?? "null"])
        do {
            if let configuration { try AIKeychain().save(configuration) }
            else { try AIKeychain().remove() }
        } catch {
            let _: EmptyResult? = try? await mailbox.call("ai.configure", params: ["configuration": "null"])
            throw error
        }
        aiRestored = true
        ai = try await mailbox.call("ai.status")
        aiError = nil
        await prepareFeed()
    }
}
