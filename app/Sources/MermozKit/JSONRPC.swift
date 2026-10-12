/// Newline-delimited JSON-RPC 2.0 framing and envelope decoding for the mermozd socket.
import Foundation

/// Error returned by the daemon or raised by the transport.
public struct RPCError: Error, Sendable, Equatable, LocalizedError {
    public var code: Int
    public var message: String
    /// Raw `data` member, kept as JSON so callers can decode what they need.
    public var data: Data?

    public static let methodNotFound = -32601
    public static let invalidParams = -32602
    public static let notFound = -32004
    public static let approvalRequired = -32010
    public static let appOnly = -32011
    public static let upstream = -32020
    /// Local transport failure (socket closed, connect failed).
    public static let transport = -1

    public init(code: Int, message: String, data: Data? = nil) {
        self.code = code
        self.message = message
        self.data = data
    }

    public var errorDescription: String? { message }
}

/// Splits a byte stream into lines; the daemon frames one JSON object per `\n`.
public struct LineBuffer: Sendable {
    private var pending = Data()

    public init() {}

    /// Appends bytes and returns every complete line (without the newline).
    public mutating func append(_ chunk: Data) -> [Data] {
        pending.append(chunk)
        var lines: [Data] = []
        while let nl = pending.firstIndex(of: 0x0A) {
            let line = pending[pending.startIndex..<nl]
            if !line.isEmpty { lines.append(Data(line)) }
            pending.removeSubrange(pending.startIndex...nl)
        }
        return lines
    }
}

/// Snake-case <-> camelCase coders shared by every protocol call.
public enum RPCCoding {
    public static func decoder() -> JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }

    public static func encoder() -> JSONEncoder {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        return e
    }
}

/// Builds a request line. Params are pre-encoded so any Encodable (or a dictionary) can be sent.
public func encodeRequest(id: Int, method: String, params: Data) -> Data {
    var line = Data("{\"jsonrpc\":\"2.0\",\"id\":\(id),\"method\":".utf8)
    line.append(try! JSONEncoder().encode(method))
    line.append(Data(",\"params\":".utf8))
    line.append(params)
    line.append(Data("}\n".utf8))
    return line
}

/// A decoded incoming line: either a response to a request or a notification.
public enum Incoming: Sendable {
    case response(id: Int, result: Result<Data, RPCError>)
    case event(DaemonEvent)
    case malformed
}

/// Parses one line from the daemon without knowing the result type yet; results stay raw JSON.
public func decodeIncoming(_ line: Data) -> Incoming {
    guard let obj = try? JSONSerialization.jsonObject(with: line, options: [.fragmentsAllowed]) as? [String: Any] else {
        return .malformed
    }
    if let id = obj["id"] as? Int {
        if let err = obj["error"] as? [String: Any] {
            let code = err["code"] as? Int ?? 0
            let message = err["message"] as? String ?? "unknown error"
            let data = err["data"].flatMap { try? JSONSerialization.data(withJSONObject: $0, options: [.fragmentsAllowed]) }
            return .response(id: id, result: .failure(RPCError(code: code, message: message, data: data)))
        }
        let result = obj["result"] ?? NSNull()
        let data = (try? JSONSerialization.data(withJSONObject: result, options: [.fragmentsAllowed])) ?? Data("null".utf8)
        return .response(id: id, result: .success(data))
    }
    if obj["method"] as? String == "event", let params = obj["params"] as? [String: Any] {
        return .event(decodeEvent(params))
    }
    return .malformed
}

func decodeEvent(_ p: [String: Any]) -> DaemonEvent {
    let kind = p["kind"] as? String ?? ""
    switch kind {
    case "threads_changed":
        return .threadsChanged(ids: p["ids"] as? [String] ?? [])
    case "accounts_changed":
        return .accountsChanged
    case "sync_progress":
        return .syncProgress(accountId: p["account_id"] as? String ?? "",
                             synced: p["synced"] as? Int ?? 0,
                             total: p["total"] as? Int ?? 0)
    case "drafts_changed":
        return .draftsChanged
    case "contacts_changed":
        return .contactsChanged
    case "providers_changed":
        return .providersChanged
    case "suggestions_changed":
        return .suggestionsChanged
    default:
        return .unknown(kind: kind)
    }
}

/// Type-erased JSON params so call sites can pass literal dictionaries.
public struct JSONValue: Encodable, Sendable {
    let encodeFn: @Sendable (Encoder) throws -> Void

    public init<T: Encodable & Sendable>(_ v: T) { encodeFn = { try v.encode(to: $0) } }

    public func encode(to encoder: Encoder) throws { try encodeFn(encoder) }

    public static let null = JSONValue(NullValue())
}

struct NullValue: Encodable, Sendable {
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encodeNil()
    }
}
