/// Integration checks use the embedded Rust library and isolated persistent stores.
import Foundation
import Testing
@testable import MermozMobile

struct MailboxTests {
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
}
