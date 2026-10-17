/// Feed cards and foreground AI state mirror the desktop engine's persisted objects.
import Foundation
import Security

struct MobileFeed: Decodable, Sendable {
    let sections: [MobileFeedSection]
    let open: Int
    let judged: Int
    var cards: [MobileFeedCard] { sections.flatMap(\.cards) }
}

struct FeedPreparationQueue {
    private var pending: [String] = []
    var hasPending: Bool { !pending.isEmpty }

    mutating func request(_ ids: [String]) {
        for id in ids where !pending.contains(id) { pending.append(id) }
    }

    func batch(fallback: [String]) -> [String] {
        Array((pending.isEmpty ? fallback : pending).prefix(12))
    }

    mutating func accepted(_ ids: [String]) {
        pending.removeAll { ids.contains($0) }
    }
}

struct MobileFeedSection: Decodable, Identifiable, Sendable {
    let key: String
    let title: String
    let cards: [MobileFeedCard]
    let more: Int
    var id: String { key }
}

struct MobileFeedCard: Decodable, Identifiable, Sendable {
    let id: String
    let kind: FeedKind
    let thread: MailThread
    let members: [MailThread]
    let title: String
    let why: String?
    let reasons: [String]
    let due: String?
    let amount: String?
    let texts: CardTexts?
    let textsWanted: Bool
    var label: String {
        kind.label
    }
    var symbol: String {
        kind.symbol
    }
}

struct CardTexts: Decodable, Sendable {
    let title: String
    let why: String
    let draft: String
    let summary: String
    let points: [String]
    let options: [CardChoice]
}

struct CardChoice: Decodable, Sendable {
    let label: String
    let detail: String
    let recommended: Bool
    let reason: String
    let reply: String
}

struct AIProgress: Decodable, Sendable {
    let enabled: Bool
    let running: Bool
    let completed: Int
    let total: Int
    let error: String?
    let threadId: String?
    let text: String
}

struct AIChatMessage: Decodable, Identifiable, Sendable {
    let id: String
    let role: String
    let text: String
    let actions: [AIChatAction]
}

struct AIChatAction: Decodable, Sendable {
    let kind: String
    let label: String
    let body: String?
    let status: String?
}

struct AIProvider: Codable, Sendable {
    var id = "mobile-assistant"
    var name: String
    var kind: String
    var baseUrl: String?
    var model: String
    var auth = "key"
}

struct AIConfiguration: Codable, Sendable {
    var provider: AIProvider
    var key: String
    var accounts: [String]
    var sorter: String?
    var sorterKey: String

    var engineJSON: String {
        get throws {
            let encoder = JSONEncoder()
            encoder.keyEncodingStrategy = .convertToSnakeCase
            return String(decoding: try encoder.encode(self), as: UTF8.self)
        }
    }
}

struct AIKeychain: Sendable {
    var service = "ai.mermoz.iphone.assistant"

    func load() throws -> AIConfiguration? {
        var query = base
        query[kSecReturnData as String] = true
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        try check(status)
        guard let data = result as? Data else { throw MailboxError(message: "Couldn't read saved AI access.") }
        return try JSONDecoder().decode(AIConfiguration.self, from: data)
    }

    func save(_ configuration: AIConfiguration) throws {
        let data = try JSONEncoder().encode(configuration)
        let status = SecItemUpdate(base as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var query = base
            query[kSecValueData as String] = data
            query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            try check(SecItemAdd(query as CFDictionary, nil))
        } else { try check(status) }
    }

    func remove() throws {
        let status = SecItemDelete(base as CFDictionary)
        if status != errSecItemNotFound { try check(status) }
    }

    private var base: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
         kSecAttrAccount as String: "assistant"]
    }

    private func check(_ status: OSStatus) throws {
        guard status == errSecSuccess else { throw MailboxError(message: "Couldn't access AI credentials (Keychain \(status)).") }
    }
}
