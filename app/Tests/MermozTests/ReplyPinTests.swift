/// A reply pinned to one message (slide to reply, ↩, or r on a message in focus): the
/// composer answers that message, addressed to its sender; × goes back to the default.
import AppKit
@testable import MermozKit
import Testing
@testable import Mermoz

private func message(_ id: String, from: Address, to: [Address] = [], cc: [Address] = [], replyTo: [Address]? = nil,
                     mine: Bool = false, date: Int64) -> Message {
    var m = Message(id: id, threadId: "t1", from: from, to: to, cc: cc, bcc: nil, date: date, subject: "Keys",
                    bodyText: "Message \(id)", bodyNew: nil, hasQuoted: nil, bodyClean: nil, hasHtml: false, isFromMe: mine,
                    attachments: [], auth: nil)
    m.replyTo = replyTo
    return m
}

private let me = Address(name: "Me", email: "me@x.com")
private let adam = Address(name: "Hélène Adam", email: "adam@x.com")
private let sam = Address(name: "Sam", email: "sam@x.com")

/// Adam wrote first (cc Sam), Sam answered last.
private let conversation = ThreadDetail(
    thread: ThreadSummary(id: "t1", number: "M-1", accountId: "a", subject: "Keys", snippet: "", participants: [], lastFrom: "Sam",
                          lastDate: 2, unread: false, messageCount: 2, hasAttachments: false, status: "todo", priority: 0),
    messages: [message("m1", from: adam, to: [me], cc: [sam], date: 1), message("m2", from: sam, to: [me], date: 2)])

@MainActor
private func model() -> AppModel {
    let m = AppModel()
    m.accounts = [Account(id: "a", email: "me@x.com", provider: "gmail", sync: SyncInfo(state: "idle", synced: 0, total: 0))]
    m.detail = conversation
    return m
}

@MainActor @Test func pinningMarksTheMessageTheReplyAnswers() {
    let m = model()
    m.pinReply(to: "m1")
    #expect(m.inlineReply(conversation).replyToMessageId == "m1")
}

@MainActor @Test func aPinnedReplyGoesToThatMessagesSender() {
    let m = model()
    m.pinReply(to: "m1")
    #expect(m.inlineReply(conversation).to == ["adam@x.com"])
}

@MainActor @Test func aPinnedReplyCopiesThatMessagesOthers() {
    let m = model()
    m.pinReply(to: "m1")
    #expect(m.inlineReply(conversation).cc == ["sam@x.com"])
}

@MainActor @Test func aPinnedReplyHonoursReplyTo() {
    let m = model()
    m.detail = ThreadDetail(thread: conversation.thread,
                            messages: [message("m1", from: adam, to: [me], replyTo: [Address(name: "", email: "office@x.com")], date: 1)])
    m.pinReply(to: "m1")
    #expect(m.inlineReply(m.detail!).to == ["office@x.com"])
}

@MainActor @Test func pinningKeepsWhatWasTyped() {
    let m = model()
    var c = m.inlineReply(conversation)
    c.body = "Yes, Monday."
    m.setInlineReply(c, for: conversation)
    m.pinReply(to: "m1")
    #expect(m.inlineReply(conversation).body == "Yes, Monday.")
}

@MainActor @Test func pinningPutsTheKeyboardInTheReply() {
    let m = model()
    let before = m.replyFocusRequest
    m.pinReply(to: "m1")
    #expect(m.replyFocusRequest == before + 1)
}

@MainActor @Test func thePinnedMessageIsShownInTheComposer() {
    let m = model()
    m.pinReply(to: "m1")
    #expect(m.pinnedMessage(conversation)?.id == "m1")
}

@MainActor @Test func unpinningForgetsTheMessage() {
    let m = model()
    m.pinReply(to: "m1")
    m.unpinReply()
    #expect(m.pinnedMessage(conversation) == nil)
}

@MainActor @Test func unpinningGoesBackToTheDefaultRecipients() {
    let m = model()
    m.pinReply(to: "m1")
    m.unpinReply()
    #expect(m.inlineReply(conversation).to == m.replyContext(conversation).to)
}

@MainActor @Test func rRepliesToTheMessageInFocus() {
    let m = model()
    m.focusedMessageId = "m1"
    _ = ThreadKeys.handle("r", model: m)
    #expect(m.inlineReply(conversation).replyToMessageId == "m1")
}

@MainActor @Test func rWithNoMessageInFocusRepliesToTheConversation() {
    let m = model()
    _ = ThreadKeys.handle("r", model: m)
    #expect(m.inlineReply(conversation).replyToMessageId == nil)
}

@MainActor @Test func theExpandedWindowKeepsThePin() {
    let m = model()
    m.pinReply(to: "m1")
    m.expandInlineReply()
    #expect(m.compose?.replyToMessageId == "m1")
}
