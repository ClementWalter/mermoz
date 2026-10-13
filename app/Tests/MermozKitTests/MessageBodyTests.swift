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

// ---- compact chat bubble vs. the whole email ----------------------------------------

private let signed = "Here is the plan.\n\nNora Lind\nnora@agency.example\n+33 6 12 34 56 78"

private func emailWithSignature() -> Message {
    var m = message(text: signed, new: "Here is the plan.",
                    attachments: [MermozKit.Attachment(id: "b", filename: "notes.pdf", mime: "application/pdf", size: 1)])
    m.to = [nora]
    m.cc = [sam]
    return m
}

@Test func aCompactBubbleHasNoRecipients() {
    #expect(BubbleParts.compact(emailWithSignature()).recipients == nil)
}

@Test func aCompactBubbleHasNoSignature() {
    #expect(BubbleParts.compact(emailWithSignature()).text == "Here is the plan.")
}

@Test func aCompactBubbleHasNoControls() {
    #expect(!BubbleParts.compact(emailWithSignature()).controls)
}

@Test func aCompactBubbleCountsItsFilesInsteadOfChips() {
    #expect(BubbleParts.compact(emailWithSignature()).fileCount == 1)
}

@Test func aCompactBubbleShowsPastedImagesAsThumbnails() {
    let m = message(attachments: [MermozKit.Attachment(id: "a", filename: "chart.png", mime: "image/png", size: 1, inline: true)])
    #expect(BubbleParts.compact(m).thumbnails.count == 1)
}

@Test func theWholeEmailNamesItsRecipients() {
    #expect(BubbleParts.expanded(emailWithSignature()).recipients == "to Nora Lind, cc Sam Ito")
}

@Test func theWholeEmailKeepsItsSignature() {
    #expect(BubbleParts.expanded(emailWithSignature()).text == signed)
}

@Test func theWholeEmailHasItsControls() {
    #expect(BubbleParts.expanded(emailWithSignature()).controls)
}

@Test func theWholeEmailListsItsFiles() {
    #expect(BubbleParts.expanded(emailWithSignature()).chips.map(\.filename) == ["notes.pdf"])
}

@Test func shortMessagesAreNotClamped() {
    #expect(!BubbleParts.clampsCompact("See you Monday."))
}

@Test func messagesOverEightLinesAreClamped() {
    #expect(BubbleParts.clampsCompact((1...9).map { "line \($0)" }.joined(separator: "\n")))
}

@Test func quotedHistoryFoldsInTheWholeEmail() {
    #expect(MessageBody.withoutQuotes("See you.\n\nOn Sun, Oct 4, Nora wrote:\n> Lunch?") == "See you.")
}

@Test func frenchQuoteOpenersFoldToo() {
    #expect(MessageBody.withoutQuotes("D'accord.\nLe 4 oct. 2026, Nora a écrit :\n> Déjeuner ?") == "D'accord.")
}

// ---- runs ---------------------------------------------------------------------------

private func said(_ id: String, by email: String, at minutes: Int64, mine: Bool = false) -> TimelineItem {
    .message(Message(id: id, threadId: "t", from: Address(name: "", email: email), to: [], cc: [], bcc: nil,
                     date: minutes * 60_000, subject: "", bodyText: "x", bodyNew: "x", hasQuoted: nil, bodyClean: nil,
                     hasHtml: false, isFromMe: mine, attachments: [], auth: nil))
}

@Test func theFirstMessageStartsARun() {
    #expect(ChatRuns.startsRun([said("a", by: "nora@x", at: 0)]) == ["a"])
}

@Test func consecutiveMessagesFromOnePersonShareARun() {
    #expect(ChatRuns.startsRun([said("a", by: "nora@x", at: 0), said("b", by: "Nora@X", at: 5)]) == ["a"])
}

@Test func anotherSenderStartsARun() {
    #expect(ChatRuns.startsRun([said("a", by: "nora@x", at: 0), said("b", by: "me@x", at: 5, mine: true)]) == ["a", "b"])
}

@Test func aLongPauseStartsARun() {
    #expect(ChatRuns.startsRun([said("a", by: "nora@x", at: 0), said("b", by: "nora@x", at: 240)]) == ["a", "b"])
}

@Test func aThreadMarkerStartsARun() {
    let items = [said("a", by: "nora@x", at: 0), .marker(threadId: "t2", subject: "s", date: 1, joined: true), said("b", by: "nora@x", at: 5)]
    #expect(ChatRuns.startsRun(items) == ["a", "b"])
}

@Test func aBubbleNeverOpensOnABareLink() {
    let m = message(text: "https://us.list-manage.com/AIW_vCP29mP?e=1\n\nLe marché rouvre samedi", new: "")
    #expect(BubbleParts.compact(m).text == "Le marché rouvre samedi")
}

@Test func linksInsideSentencesStayInTheBubble() {
    #expect(BubbleParts.withoutLinkLines("Book on https://notary.example/rdv today") == "Book on https://notary.example/rdv today")
}

// ---- Markdown never shows as syntax -------------------------------------------------

private let sportsMail = "![Email header](https://static.club.example/emails/email-header.jpg)\n\n---\n\n[ ![](https://media2.club.example/team/42/logo/abc.png) ](https://club.example/team/42)\n\n## Match ce samedi à 10h\n\nBonjour, **le match** est confirmé. [Répondre](https://club.example/r/1) avant jeudi."

@Test(arguments: ["![", "](", "---", "**", "## "])
func compactTextHasNoMarkdownSyntax(mark: String) {
    #expect(!BubbleParts.compact(message(text: sportsMail, new: sportsMail)).text.contains(mark))
}

@Test func compactTextOpensOnTheHeadline() {
    #expect(BubbleParts.compact(message(text: sportsMail, new: sportsMail)).text.hasPrefix("Match ce samedi à 10h"))
}

@Test func markdownLinksReadAsTheirWords() {
    #expect(BubbleParts.plainMarkdown("[Répondre](https://club.example/r/1) avant jeudi.") == "Répondre avant jeudi.")
}

@Test func bracketsWithoutALinkStay() {
    #expect(BubbleParts.plainMarkdown("Prix [indicatif] : 10 €") == "Prix [indicatif] : 10 €")
}

// ---- a bubble is never empty --------------------------------------------------------

/// A town newsletter sent through Mailjet, with invisible and non-breaking blank lines.
private let townLetter = "<<Camille Durand - Maire de Villeneuve>> [https://x1abc.mjt.lu/img/x1abc/b/portrait.png]\n\u{00A0}\n\u{200B}\u{200C}\n\u{2007}\u{FEFF}\nChers Habitants,\n\u{00A0}\nLe nouveau marché ouvre samedi.\n \n<<logo>> [https://x1abc.mjt.lu/img/x1abc/b/logo.png]\n\u{00A0}\n"

@Test func compactTextsFirstLineIsNonBlank() {
    let first = BubbleParts.compact(message(text: townLetter, new: townLetter)).text.components(separatedBy: "\n")[0]
    #expect(!first.trimmingCharacters(in: .whitespaces).isEmpty)
}

@Test func aHeadlinePlaceholderReadsAsItsWords() {
    #expect(BubbleParts.compact(message(text: townLetter, new: townLetter)).text.hasPrefix("Camille Durand - Maire de Villeneuve\n"))
}

@Test func placeholderAddressesAreGone() {
    #expect(!BubbleParts.compact(message(text: townLetter, new: townLetter)).text.contains("mjt.lu"))
}

@Test func invisibleLinesFoldIntoOneBlank() {
    #expect(BubbleParts.tidy("Chers Habitants,\n\u{00A0}\n\u{200B}\n\u{2007}\nLe marché.") == "Chers Habitants,\n\nLe marché.")
}

@Test func anInvisibleNewPartFallsBackToTheWholeText() {
    #expect(BubbleParts.compact(message(text: "Le marché ouvre samedi.", new: "\u{00A0}\n\u{200B}")).text == "Le marché ouvre samedi.")
}

@Test func anEntirelyInvisibleMessageSaysItIsEmpty() {
    #expect(BubbleContent.compact(message(text: "\u{00A0}\n\u{200B}", new: "\u{FEFF}"), showAll: false).text == "(empty message)")
}

@Test(arguments: ["\u{00A0}\u{00A0}", "\u{200B}\u{200C}\u{FEFF}", "\u{2007}\u{202F}", "<<logo>> [https://x.mjt.lu/img/a.png]"])
func invisibleTextTidiesToNothing(text: String) {
    #expect(BubbleParts.tidy(text) == "")
}
