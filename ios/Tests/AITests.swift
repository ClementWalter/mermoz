/// The phone decodes the shared Feed and keeps assistant credentials in the real Keychain.
import Foundation
import Testing
@testable import MermozMobile

struct AITests {
    @Test func demoFeedContainsPreparedReplies() async throws {
        let home = URL.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let mailbox = Mailbox(directory: home)
        let feed: MobileFeed = try await mailbox.call("feed.list")
        #expect(feed.cards.contains { $0.texts?.draft.isEmpty == false })
    }

    @Test func aiCredentialsSurviveAKeychainReopen() throws {
        let service = "ai.mermoz.tests.\(UUID().uuidString)"
        let store = AIKeychain(service: service)
        defer { try? store.remove() }
        let provider = AIProvider(name: "Test", kind: "anthropic", model: "test-model")
        try store.save(AIConfiguration(provider: provider, key: "test-ai-key", accounts: ["acc"], sorterKey: ""))
        #expect(try AIKeychain(service: service).load()?.key == "test-ai-key")
    }

    @Test func disconnectRemovesSavedAIKeys() throws {
        let store = AIKeychain(service: "ai.mermoz.tests.\(UUID().uuidString)")
        let provider = AIProvider(name: "Test", kind: "anthropic", model: "test-model")
        try store.save(AIConfiguration(provider: provider, key: "test-ai-key", accounts: ["acc"], sorterKey: ""))
        try store.remove()
        #expect(try store.load() == nil)
    }

    @Test func macAndPhoneUseTheSameAccent() {
        #expect(MermozPalette.accent == "#1F6FB5")
    }
}
