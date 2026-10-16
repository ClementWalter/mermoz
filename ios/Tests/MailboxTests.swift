/// Integration checks use the embedded Rust library and isolated persistent stores.
import Foundation
import Testing
@testable import MermozMobile

struct MailboxTests {
    @Test func theBundleTargetsIPhone() {
        #expect(Bundle.main.object(forInfoDictionaryKey: "UIDeviceFamily") as? [Int] == [1])
    }

    private func directory() -> URL { URL.temporaryDirectory.appending(path: UUID().uuidString) }

    @Test func demoMailLoadsThroughTheBridge() async throws {
        let home = directory()
        defer { try? FileManager.default.removeItem(at: home) }
        let mailbox = Mailbox(directory: home)
        let page: MailPage = try await mailbox.call("threads.list")
        #expect(!page.threads.isEmpty)
    }

    @Test func messageBodiesDecodeThroughTheBridge() async throws {
        let home = directory()
        defer { try? FileManager.default.removeItem(at: home) }
        let mailbox = Mailbox(directory: home)
        let page: MailPage = try await mailbox.call("threads.list")
        let detail: MailDetail = try await mailbox.call("threads.get", params: ["id": page.threads[0].id])
        #expect(!detail.messages[0].bodyText.isEmpty)
    }

    @Test func statusChangesSurviveANewEngine() async throws {
        let home = directory()
        defer { try? FileManager.default.removeItem(at: home) }
        let id: String
        do {
            let mailbox = Mailbox(directory: home)
            let page: MailPage = try await mailbox.call("threads.list")
            id = page.threads[0].id
            let _: MailThread = try await mailbox.call("threads.update", params: ["id": id, "status": "archive"])
        }
        let reopened = Mailbox(directory: home)
        let detail: MailDetail = try await reopened.call("threads.get", params: ["id": id])
        #expect(detail.thread.status == "archive")
    }

    @Test func aMissingThreadReturnsAnError() async throws {
        let home = directory()
        defer { try? FileManager.default.removeItem(at: home) }
        let mailbox = Mailbox(directory: home)
        await #expect(throws: (any Error).self) {
            let _: MailDetail = try await mailbox.call("threads.get", params: ["id": "missing"])
        }
    }

    @Test func aReplyStaysLocalUntilSend() async throws {
        let home = directory()
        defer { try? FileManager.default.removeItem(at: home) }
        let mailbox = Mailbox(directory: home)
        let page: MailPage = try await mailbox.call("search", params: ["query": "Ride tracking screens"])
        let before: MailDetail = try await mailbox.call("threads.get", params: ["id": page.threads[0].id])
        let _: MailDraft = try await mailbox.call("drafts.reply", params: ["id": page.threads[0].id, "body": "Local draft test."])
        let after: MailDetail = try await mailbox.call("threads.get", params: ["id": page.threads[0].id])
        #expect(after.messages.count == before.messages.count)
    }

    @Test func aSavedReplySurvivesANewEngine() async throws {
        let home = directory()
        defer { try? FileManager.default.removeItem(at: home) }
        let mailbox = Mailbox(directory: home)
        let page: MailPage = try await mailbox.call("search", params: ["query": "Ride tracking screens"])
        let draft: MailDraft = try await mailbox.call("drafts.reply", params: ["id": page.threads[0].id, "body": "Persistent draft test."])
        let reopened = Mailbox(directory: home)
        let drafts: [MailDraft] = try await reopened.call("drafts.list")
        #expect(drafts.contains { $0.id == draft.id && $0.body == "Persistent draft test." })
    }

    @Test func aDemoReplyCanBeSentWithoutANetwork() async throws {
        let home = directory()
        defer { try? FileManager.default.removeItem(at: home) }
        let mailbox = Mailbox(directory: home)
        let page: MailPage = try await mailbox.call("search", params: ["query": "Ride tracking screens"])
        let draft: MailDraft = try await mailbox.call("drafts.reply", params: ["id": page.threads[0].id, "body": "Offline demo reply."])
        let sent: MailDraft = try await mailbox.call("drafts.send", params: ["id": draft.id])
        #expect(sent.state == "sent")
    }
}
