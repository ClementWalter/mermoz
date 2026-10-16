/// Google browser consent and device-bound Keychain storage keep tokens out of SQLite.
import AuthenticationServices
import Foundation
import Security
import UIKit

struct AccountCredential: Codable, Sendable {
    let accountId: String
    let clientId: String
    let refreshToken: String
}

struct AccountSync: Decodable, Equatable, Sendable {
    let state: String
    let synced: Int
    let total: Int
    let error: String?
}

struct MailAccount: Decodable, Equatable, Identifiable, Sendable {
    let id: String
    let email: String
    let provider: String
    let hidden: Bool
    let sync: AccountSync
}

struct LoginStart: Decodable, Sendable {
    let url: URL
    let scheme: String
}

struct LoginResult: Decodable, Sendable {
    let account: MailAccount
    let credential: AccountCredential
}

struct EmptyResult: Decodable, Sendable {}

struct CredentialStore: Sendable {
    var service = "ai.mermoz.iphone.oauth"

    func save(_ credential: AccountCredential) throws {
        let data = try JSONEncoder().encode(credential)
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
                                   kSecAttrService as String: service,
                                   kSecAttrAccount as String: credential.accountId]
        let status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var item = query
            item[kSecValueData as String] = data
            item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            try check(SecItemAdd(item as CFDictionary, nil))
        } else { try check(status) }
    }

    func all() throws -> [AccountCredential] {
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
                                   kSecAttrService as String: service,
                                   kSecMatchLimit as String: kSecMatchLimitAll,
                                   kSecReturnData as String: true]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        try check(status)
        return try (result as? [Data] ?? []).map { try JSONDecoder().decode(AccountCredential.self, from: $0) }
    }

    func delete(_ accountId: String) throws {
        let status = SecItemDelete([kSecClass as String: kSecClassGenericPassword,
                                   kSecAttrService as String: service,
                                   kSecAttrAccount as String: accountId] as CFDictionary)
        if status != errSecItemNotFound { try check(status) }
    }

    private func check(_ status: OSStatus) throws {
        guard status == errSecSuccess else {
            throw MailboxError(message: "Couldn't access saved sign-in (Keychain \(status)).")
        }
    }
}

@MainActor
final class GoogleLogin: NSObject, ASWebAuthenticationPresentationContextProviding {
    private var session: ASWebAuthenticationSession?

    func authorize(_ start: LoginStart) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            let session = ASWebAuthenticationSession(url: start.url, callbackURLScheme: start.scheme) { url, error in
                if let url { continuation.resume(returning: url.absoluteString) }
                else { continuation.resume(throwing: error ?? MailboxError(message: "Sign-in was cancelled.")) }
            }
            session.presentationContextProvider = self
            self.session = session
            if !session.start() {
                self.session = nil
                continuation.resume(throwing: MailboxError(message: "Couldn't open Google sign-in."))
            }
        }
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows).first(where: \.isKeyWindow) ?? ASPresentationAnchor()
    }
}
