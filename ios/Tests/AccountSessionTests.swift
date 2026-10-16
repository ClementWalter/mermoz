/// Isolated simulator Keychain entries verify credential persistence and removal.
import Foundation
import Testing
@testable import MermozMobile

struct AccountSessionTests {
    @Test func credentialsSurviveANewStore() throws {
        let service = "ai.mermoz.tests.\(UUID().uuidString)"
        let store = CredentialStore(service: service)
        defer { try? store.delete("account") }
        try store.save(AccountCredential(accountId: "account", clientId: "client", refreshToken: "test-token"))
        #expect(try CredentialStore(service: service).all().first?.refreshToken == "test-token")
    }

    @Test func replacingACredentialDoesNotDuplicateIt() throws {
        let store = CredentialStore(service: "ai.mermoz.tests.\(UUID().uuidString)")
        defer { try? store.delete("account") }
        try store.save(AccountCredential(accountId: "account", clientId: "client", refreshToken: "first"))
        try store.save(AccountCredential(accountId: "account", clientId: "client", refreshToken: "second"))
        #expect(try store.all().count == 1)
    }

    @Test func removingAnAccountRemovesItsCredential() throws {
        let store = CredentialStore(service: "ai.mermoz.tests.\(UUID().uuidString)")
        try store.save(AccountCredential(accountId: "account", clientId: "client", refreshToken: "test-token"))
        try store.delete("account")
        #expect(try store.all().isEmpty)
    }

    @Test func googleCallbackMatchesTheEmbeddedClient() throws {
        guard let client = Bundle.main.object(forInfoDictionaryKey: "MERMOZGoogleClientID") as? String,
              let types = Bundle.main.object(forInfoDictionaryKey: "CFBundleURLTypes") as? [[String: Any]] else {
            throw MailboxError(message: "Google callback configuration is missing.")
        }
        let schemes = types.flatMap { $0["CFBundleURLSchemes"] as? [String] ?? [] }
        #expect(schemes.contains(client.split(separator: ".").reversed().joined(separator: ".")))
    }
}
