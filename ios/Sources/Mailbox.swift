/// Typed access to the embedded engine, serialized off the main actor for SQLite work.
import Foundation
import Observation

struct MailThread: Decodable, Identifiable, Hashable, Sendable {
    let id: String
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
            error = nil
        } catch is CancellationError {
        } catch {
            if !Task.isCancelled { self.error = error.localizedDescription }
        }
    }
}
