/// Reading patches the open conversation in place from `threads.mark_read`'s answer.
import Foundation
import Testing
@testable import MermozKit

private func thread(_ id: String, status: String = "unread", unread: Bool = true) -> ThreadSummary {
    ThreadSummary(id: id, number: id, accountId: "a", subject: "Flat sale", snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: unread, messageCount: 1, hasAttachments: false, status: status, priority: 0)
}

private let message = Message(id: "m1", threadId: "t1", from: Address(name: "Nora Lind", email: "nora@agency.example"),
                              to: [], cc: [], bcc: nil, date: 1, subject: "Flat sale", bodyText: "Hello", bodyNew: nil,
                              hasQuoted: nil, bodyClean: nil, hasHtml: false, isFromMe: false, attachments: [], auth: nil)

private let open = ThreadDetail(thread: thread("t1"), messages: [message],
                                conversation: Conversation(id: "t1", threads: [thread("t1"), thread("t2")]))
private let filed = thread("t1", status: "archive", unread: false)

@Test func theLeadBecomesTheThreadAsReadingLeftIt() {
    #expect(open.afterReading(filed).thread == filed)
}

@Test func theLeadIsReplacedInTheConversationToo() {
    #expect(open.afterReading(filed).conversation?.threads.first == filed)
}

@Test func theOtherThreadsOfTheConversationAreRead() {
    #expect(open.afterReading(filed).conversation?.threads.last?.unread == false)
}

@Test func theMessagesStayAsTheyWere() {
    #expect(open.afterReading(filed).messages == open.messages)
}
