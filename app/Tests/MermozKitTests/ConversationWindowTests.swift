/// A long conversation arrives as its latest messages plus a count of the earlier ones.
import Foundation
import Testing
@testable import MermozKit

private let lead = ThreadSummary(id: "t1", number: "M-1", accountId: "a", subject: "Flat sale", snippet: "",
                                 participants: [], lastFrom: "", lastDate: 0, unread: false, messageCount: 1,
                                 hasAttachments: false, status: "todo", priority: 0)

/// The detail as the daemon sends it, with `earlier` removed when nil.
private func wire(earlier: Int?) throws -> Data {
    let d = ThreadDetail(thread: lead, messages: [], conversation: nil, earlier: earlier)
    return try RPCCoding.encoder().encode(d)
}

@Test func aWindowSaysHowManyMessagesItLeftOut() throws {
    let d = try RPCCoding.decoder().decode(ThreadDetail.self, from: try wire(earlier: 40))
    #expect(d.earlier == 40)
}

@Test func aWholeConversationLeavesNothingOut() throws {
    let d = try RPCCoding.decoder().decode(ThreadDetail.self, from: try wire(earlier: nil))
    #expect(d.earlier == nil)
}
