/// An email delivered more than once shows once in the chat, with "Received twice".
import Testing
@testable import MermozKit

private func message(_ id: String, copies: Int?, date: Int64 = 1) -> Message {
    var m = Message(id: id, threadId: "t", from: Address(name: "Nora", email: "nora@x.com"), to: [], cc: [], bcc: nil,
                    date: date, subject: "S", bodyText: "The keys are at the desk.", bodyNew: "The keys are at the desk.",
                    hasQuoted: nil, bodyClean: nil, hasHtml: false, isFromMe: false, attachments: [], auth: nil)
    m.copies = copies
    return m
}

private func note(_ m: Message) -> String? {
    ChatRows.build(items: [.message(m)], hidden: 0, summary: nil, expanded: [], shownInFull: [], focused: nil,
                   time: { _ in "09:41" }).rows.compactMap { if case let .bubble(b) = $0 { b.note } else { nil } }.first
}

@Test func anEmailReceivedTwiceSaysSo() {
    #expect(note(message("m", copies: 2)) == "Received twice")
}

@Test func anEmailReceivedThreeTimesCountsThem() {
    #expect(note(message("m", copies: 3)) == "Received 3 times")
}

@Test func anEmailReceivedOnceHasNoNote() {
    #expect(note(message("m", copies: 1)) == nil)
}

@Test func anOlderDaemonsEmailHasNoNote() {
    #expect(note(message("m", copies: nil)) == nil)
}
