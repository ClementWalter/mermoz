/// A bubble shows the email at once (never the AI's version unless chosen), whole, with
/// pasted images in the body, the same way for every kind of conversation.
import Foundation
import Testing
@testable import MermozKit

private func message(text: String = "Hello Nora", new: String? = "Hello Nora", html: Bool = false, clean: String? = nil,
                     attachments: [MermozKit.Attachment] = [], mine: Bool = false) -> Message {
    Message(id: "m", threadId: "t", from: Address(name: "Nora Lind", email: "nora@agency.example"), to: [], cc: [],
            bcc: nil, date: 0, subject: "Flat sale", bodyText: text, bodyNew: new, hasQuoted: nil, bodyClean: clean,
            hasHtml: html, isFromMe: mine, attachments: attachments, auth: nil)
}

private let nora = Address(name: "Nora Lind", email: "nora@agency.example")
private let sam = Address(name: "Sam Ito", email: "sam@studio.example")

@Test func openingShowsTheEmailNotTheAIVersion() {
    #expect(MessageBody.shown(message(clean: "Cleaned"), mode: .original, htmlReady: false, showQuoted: false) == .text("Hello Nora"))
}

@Test func formattedEmailShowsItsTextWhileLoading() {
    #expect(MessageBody.shown(message(html: true), mode: .original, htmlReady: false, showQuoted: false) == .text("Hello Nora"))
}

@Test func formattedEmailShowsOnceLoaded() {
    #expect(MessageBody.shown(message(html: true), mode: .original, htmlReady: true, showQuoted: false) == .html)
}

@Test func optimizedWithoutAVersionYetShowsTheEmail() {
    #expect(MessageBody.shown(message(), mode: .optimized, htmlReady: false, showQuoted: false) == .text("Hello Nora"))
}

@Test func optimizedShowsTheAIVersionOnceMade() {
    #expect(MessageBody.shown(message(clean: "Cleaned"), mode: .optimized, htmlReady: false, showQuoted: false) == .clean("Cleaned"))
}

@Test func quotedHistoryShowsOnRequest() {
    let m = message(text: "Hello Nora\n> earlier", new: "Hello Nora")
    #expect(MessageBody.shown(m, mode: .original, htmlReady: false, showQuoted: true) == .text("Hello Nora\n> earlier"))
}

@Test func theBubbleShowsTheWholeBodyNotASnippet() {
    let body = String(repeating: "Mais sans le document je ne peux pas vérifier. ", count: 40)
    #expect(MessageBody.shown(message(text: body, new: body), mode: .original, htmlReady: false, showQuoted: false) == .text(body))
}

@Test func anEmptyNewPartFallsBackToTheFullText() {
    #expect(MessageBody.shown(message(text: "Only quoted", new: ""), mode: .original, htmlReady: false, showQuoted: false) == .text("Only quoted"))
}

@Test func ordinaryMessagesAreNeverClamped() {
    #expect(!MessageBody.clamp("short message", expanded: false).clamped)
}

@Test func veryLongMessagesAreClampedVisibly() {
    #expect(MessageBody.clamp(String(repeating: "word ", count: 3000), expanded: false).clamped)
}

@Test func clampingCutsAtAWord() {
    #expect(MessageBody.clamp("alpha beta gamma", expanded: false, limit: 12).text == "alpha beta")
}

@Test func showMoreGivesTheWholeText() {
    let long = String(repeating: "word ", count: 3000)
    #expect(MessageBody.clamp(long, expanded: true).text == long)
}

@Test func pastedImagesAreNotChips() {
    let m = message(attachments: [MermozKit.Attachment(id: "a", filename: "chart.png", mime: "image/png", size: 1, inline: true),
                                  MermozKit.Attachment(id: "b", filename: "notes.pdf", mime: "application/pdf", size: 1, inline: false)])
    #expect(MessageBody.chips(m).map(\.filename) == ["notes.pdf"])
}

@Test func pastedImagesShowInTheBody() {
    let m = message(attachments: [MermozKit.Attachment(id: "a", filename: "chart.png", mime: "image/png", size: 1, inline: true)])
    #expect(MessageBody.inlineImages(m).map(\.filename) == ["chart.png"])
}

@Test func olderMailKeepsItsChips() {
    let m = message(attachments: [MermozKit.Attachment(id: "a", filename: "chart.png", mime: "image/png", size: 1)])
    #expect(MessageBody.chips(m).count == 1)
}

@Test(arguments: ["person", "feed", "list", "unknown"])
func everyKindIsABubble(kind: String) {
    guard case .bubble = MessageLayout.of(message(), kind: kind) else { Issue.record("not a bubble"); return }
}

@Test func theUsersOwnMessagesAreOnTheRight() {
    #expect(MessageLayout.of(message(mine: true), kind: "person") == .bubble(fromMe: true, compact: false))
}

@Test func someoneElsesMessageIsABubbleEvenWhenTheUserIsNotOnIt() {
    #expect(MessageLayout.of(message(), kind: nil) == .bubble(fromMe: false, compact: false))
}

@Test func feedsUseTheCompactBubble() {
    #expect(MessageLayout.of(message(), kind: "feed") == .bubble(fromMe: false, compact: true))
}

@Test func recipientsReadToThenCc() {
    #expect(Recipients.line(to: [nora], cc: [sam]) == "to Nora Lind, cc Sam Ito")
}

@Test func anEmptyToLeavesNoStrayComma() {
    #expect(Recipients.line(to: [], cc: [sam]) == "cc Sam Ito")
}

@Test func undisclosedRecipientsAreLeftOut() {
    #expect(Recipients.line(to: [Address(name: "", email: "undisclosed-recipients:;")], cc: [sam]) == "cc Sam Ito")
}

@Test func noVisibleRecipientsSaySo() {
    #expect(Recipients.line(to: [], cc: []) == "to undisclosed recipients")
}

@Test func blindCopiesAreListedLast() {
    #expect(Recipients.line(to: [nora], cc: [], bcc: [sam]) == "to Nora Lind, bcc Sam Ito")
}
