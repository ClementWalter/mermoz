/// Send answers within its deadline: done, failed, or late while it keeps going; and the
/// words a failed send shows.
import Foundation
import Testing
@testable import MermozKit

private let sent = #"{"jsonrpc":"2.0","id":%d,"result":{"sent":true}}"#
private let refused = #"{"jsonrpc":"2.0","id":%d,"error":{"code":-32020,"message":"provider error 400"}}"#

/// A daemon that answers `drafts.send` after `delay` with `answer`.
private func daemon(after delay: TimeInterval, _ answer: String = sent) throws -> (StubDaemon, DaemonClient) {
    let stub = try StubDaemon { id, _ in
        Thread.sleep(forTimeInterval: delay)
        return String(format: answer, id)
    }
    return (stub, DaemonClient(connection: try Connection(path: stub.path)))
}

private func sendTask(_ client: DaemonClient) -> Task<SendResult, Error> {
    Task { try await client.sendDraft("dft_1") }
}

@Test func promptSendIsDone() async throws {
    let (stub, client) = try daemon(after: 0)
    let r = await waitAtMost(.seconds(2), for: sendTask(client))
    #expect({ if case .done = r { true } else { false } }())
    _ = stub
}

@Test func slowSendIsLateInsteadOfSpinning() async throws {
    let (stub, client) = try daemon(after: 1.5)
    let r = await waitAtMost(.milliseconds(100), for: sendTask(client))
    #expect({ if case .late = r { true } else { false } }())
    _ = stub
}

@Test func slowSendAnswersAtTheDeadline() async throws {
    let (stub, client) = try daemon(after: 2)
    let started = ContinuousClock.now
    _ = await waitAtMost(.milliseconds(100), for: sendTask(client))
    #expect(ContinuousClock.now - started < .milliseconds(800))
    _ = stub
}

@Test func lateSendStillFinishesInTheBackground() async throws {
    let (stub, client) = try daemon(after: 0.5)
    let task = sendTask(client)
    _ = await waitAtMost(.milliseconds(50), for: task)
    #expect(try await task.value.sent)
    _ = stub
}

@Test func refusedSendReachesTheCaller() async throws {
    let (stub, client) = try daemon(after: 0, refused)
    let r = await waitAtMost(.seconds(2), for: sendTask(client))
    #expect({ if case .failed = r { true } else { false } }())
    _ = stub
}

@Test func refusedSendSaysGmailRefusedIt() {
    let e = RPCError(code: RPCError.upstream, message: "provider error 400")
    #expect(SendErrorText.describe(e) == "Gmail didn't accept this message (error 400). Check the recipients, then retry.")
}

@Test func networkFailureSaysCheckTheConnection() {
    let e = RPCError(code: -32000, message: "http error: error sending request for url (https://gmail.example/send)")
    #expect(SendErrorText.describe(e) == "Couldn't reach Gmail. Check your connection, then retry.")
}

@Test func lostDaemonSaysRetryInAMoment() {
    let e = RPCError(code: RPCError.transport, message: "socket closed")
    #expect(SendErrorText.describe(e) == "Mermoz lost touch with its background service. Retry in a moment.")
}
