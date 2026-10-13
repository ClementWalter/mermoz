/// A reply written with CRLF line endings (M-2629's shape) draws as a bubble with its text,
/// in a fresh row and in a row recycled from another conversation, clamped or not.
import AppKit
import Testing
@testable import MermozKit

let crlfReply = "Salut Clément\r\n\r\nMerci pour ton retour,\r\nTant pis ce sera pour une prochaine fois !\r\n\r\n"
    + "Serais-tu dispo pour un café la semaine prochaine ?\r\n\r\nA dispo\r\n\r\nRichard\r\n\r\n"
    + "Le lun. 12 janv. 2026 à 11:54, Clément Walter <clement@example.com> a\r\nécrit :\r\n\r\n"
    + "> Salut Richard\r\n>\r\n> Merci pour l'invitation, je ne pourrai pas venir cette fois.\r\n>\r\n> Clément\r\n"

let crlfMessage = Message(id: "r1", threadId: "t1", from: Address(name: "Richard Roe", email: "richard@onchain.example"),
                          to: [], cc: [], bcc: nil, date: 2_000, subject: "Re: Invitation", bodyText: crlfReply,
                          bodyNew: nil, hasQuoted: nil, bodyClean: nil, hasHtml: false, isFromMe: false,
                          attachments: [], auth: nil)

private let mine = Message(id: "c1", threadId: "t1", from: Address(name: "Clément", email: "clement@example.com"),
                           to: [], cc: [], bcc: nil, date: 1_000, subject: "Invitation", bodyText: "Salut Richard,\r\nPas dispo, désolé !",
                           bodyNew: nil, hasQuoted: nil, bodyClean: nil, hasHtml: false, isFromMe: true, attachments: [], auth: nil)

@Test func aCRLFTextIsLaidOut() {
    let laid = BubbleTextCache().layout(id: "r1", text: crlfReply, style: BubbleTextStyle(), width: 400, maxLines: 8)
    #expect(laid.manager.glyphRange(for: laid.container).length > 0)
}

@Test func aCRLFTextTakesItsLines() {
    let laid = BubbleTextCache().layout(id: "r1", text: crlfReply, style: BubbleTextStyle(), width: 400, maxLines: 8)
    #expect(laid.size.height > 100)
}

@Test func aCRLFTextIsDrawnWithPlainLineBreaks() {
    let laid = BubbleTextCache().layout(id: "r1", text: crlfReply, style: BubbleTextStyle(), width: 400, maxLines: 0)
    #expect(!laid.storage.string.contains("\r"))
}

/// The thread as the chat stream shows it, in a fresh stream or after another conversation.
@MainActor
private func shownReply(recycled: Bool) -> SelectableBubbleText? {
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 700, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView = stream
    func show(_ messages: [Message], _ conversation: String) {
        let rows = ChatRows.build(items: messages.map(TimelineItem.message), hidden: 0, summary: nil, expanded: [],
                                  shownInFull: [], focused: nil, time: { _ in "11:54" }).rows
        stream.show(rows, conversation: conversation, messages: messages.count)
        stream.layoutSubtreeIfNeeded()
        stream.documentView?.layoutSubtreeIfNeeded()
    }
    if recycled {
        let other = Message(id: "o1", threadId: "t0", from: Address(name: "Ada", email: "ada@example.com"), to: [], cc: [],
                            bcc: nil, date: 1, subject: "Hi", bodyText: "Lunch tomorrow?", bodyNew: nil, hasQuoted: nil,
                            bodyClean: nil, hasHtml: false, isFromMe: false, attachments: [], auth: nil)
        show([other, mine], "c0")
    }
    show([mine, crlfMessage], "c1")
    return stream.bubbleText("r1")
}

@MainActor @Test func theReplyBubbleHoldsItsText() {
    #expect(shownReply(recycled: false)?.textStorage?.string.contains("Merci pour ton retour,") == true)
}

@MainActor @Test func theReplyBubbleDrawsItsText() {
    #expect(inkPixels(shownReply(recycled: false)!) > 100)
}

@MainActor @Test func theReplyBubbleHasRoomForItsText() {
    #expect((shownReply(recycled: false)?.frame.height ?? 0) > 100)
}

@MainActor @Test func aRecycledRowHoldsTheReplyText() {
    #expect(shownReply(recycled: true)?.textStorage?.string.contains("Merci pour ton retour,") == true)
}

@MainActor @Test func aRecycledRowDrawsTheReplyText() {
    #expect(inkPixels(shownReply(recycled: true)!) > 100)
}

@MainActor @Test func aRecycledRowKeepsNoCarriageReturn() {
    #expect(shownReply(recycled: true)?.textStorage?.string.contains("\r") == false)
}
