/// A reaction ("❤️ Nora reacted to your message") is a badge on the message it reacts
/// to, not a bubble of its own; one to a message out of view is a small centred line.
/// Decorative images (signature logos) never reach a bubble.
import AppKit
import Testing
@testable import MermozKit

private func message(_ id: String, _ body: String = "Are the keys at the desk?", from: String = "elliot@x.com",
                     name: String = "Elliot", mine: Bool = false, date: Int64, reaction: Reaction? = nil,
                     attachments: [MermozKit.Attachment] = []) -> Message {
    var m = Message(id: id, threadId: "t", from: Address(name: name, email: from), to: [], cc: [], bcc: nil, date: date,
                    subject: "S", bodyText: body, bodyNew: body, hasQuoted: nil, bodyClean: nil, hasHtml: false,
                    isFromMe: mine, attachments: attachments, auth: nil)
    m.reaction = reaction
    return m
}

private let question = message("q", mine: true, date: 1)
private let heart = message("r1", "❤️", from: "nora@x.com", name: "Nora Lind", date: 2,
                            reaction: Reaction(emoji: "❤️", targetMessageId: "q"))

private func rows(_ messages: [Message], expanded: Set<String> = []) -> [ChatRowSpec] {
    ChatRows.build(items: messages.map(TimelineItem.message), hidden: 0, summary: nil, expanded: expanded, shownInFull: [],
                   focused: nil, time: { _ in "09:41" }).rows
}

private func bubbles(_ r: [ChatRowSpec]) -> [BubbleSpec] { r.compactMap { if case let .bubble(b) = $0 { b } else { nil } } }
private func markers(_ r: [ChatRowSpec]) -> [MarkerSpec] { r.compactMap { if case let .marker(m) = $0 { m } else { nil } } }

// MARK: badge

@Test func aReactionIsNoBubble() {
    #expect(bubbles(rows([question, heart])).map(\.id) == ["q"])
}

@Test func theReactedMessageCarriesTheBadge() {
    #expect(bubbles(rows([question, heart]))[0].reactions == [ReactionBadge(emoji: "❤️", by: "Nora Lind")])
}

@Test func aReactionByTheUserIsByYou() {
    let mine = message("r2", "👍", from: "me@x.com", mine: true, date: 3, reaction: Reaction(emoji: "👍", targetMessageId: "e"))
    let theirs = message("e", date: 1)
    #expect(bubbles(rows([theirs, mine]))[0].reactions.first?.by == "You")
}

@Test func theBadgeTooltipSaysWhoReacted() {
    #expect(ReactionBadge.tooltip([ReactionBadge(emoji: "❤️", by: "Nora"), ReactionBadge(emoji: "👍", by: "You")])
        == "❤️ by Nora, 👍 by You")
}

@Test func theSameEmojiTwiceIsCounted() {
    #expect(ReactionBadge.label([ReactionBadge(emoji: "❤️", by: "Nora"), ReactionBadge(emoji: "❤️", by: "Sam"),
                                 ReactionBadge(emoji: "👍", by: "You")]) == "❤️ 2 👍")
}

@Test func aReactionDoesNotBreakItsSendersRun() {
    let a = message("a", date: 1), b = message("b", "And the code is 4512.", date: 3)
    let r = message("r", "👍", from: "nora@x.com", name: "Nora", date: 2, reaction: Reaction(emoji: "👍", targetMessageId: "a"))
    #expect(bubbles(rows([a, r, b])).map(\.firstOfRun) == [true, false])
}

// MARK: fallback

@Test func aReactionToAMessageOutOfViewIsALine() {
    let lost = message("r3", "❤️", from: "nora@x.com", name: "Nora Lind", date: 2,
                       reaction: Reaction(emoji: "❤️", targetMessageId: "gone"))
    #expect(markers(rows([question, lost])).map(\.line) == ["❤️ Nora Lind reacted"])
}

@Test func aReactionToAnUnknownMessageIsALine() {
    let lost = message("r4", "😮", from: "nora@x.com", name: "Nora", date: 2, reaction: Reaction(emoji: "😮"))
    #expect(markers(rows([question, lost])).map(\.line) == ["😮 Nora reacted"])
}

@Test func aReactionToAnOpenedEmailIsALine() {
    #expect(markers(rows([question, heart], expanded: ["q"])).map(\.line) == ["❤️ Nora Lind reacted"])
}

// MARK: drawn

@MainActor
private func stream(_ messages: [Message], width: CGFloat = 640) -> ChatStreamView {
    let s = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: width, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView = s
    s.show(rows(messages), conversation: "c", messages: messages.count)
    s.layoutSubtreeIfNeeded()
    s.documentView?.layoutSubtreeIfNeeded()
    return s
}

@MainActor @Test func theBadgeSitsOnTheBubblesBottomEdge() {
    let row = stream([question, heart]).bubbleRow("q")!
    #expect(row.badgeRect.minY < row.bubbleRect.maxY && row.badgeRect.maxY > row.bubbleRect.maxY)
}

@MainActor @Test(arguments: [360.0, 640.0])
func theBadgeStaysInItsRow(width: Double) {
    let row = stream([question, heart], width: width).bubbleRow("q")!
    #expect(row.bounds.contains(row.badgeRect))
}

@MainActor @Test func rowsWithABadgeDoNotOverlapTheNext() {
    let s = stream([question, heart, message("n", "Yes!", date: 3)])
    let f = s.rowViews.map(\.frame)
    #expect(zip(f, f.dropFirst()).allSatisfy { $0.maxY <= $1.minY })
}

@MainActor @Test func theBadgeTooltipShowsOverTheBadge() {
    let row = stream([question, heart]).bubbleRow("q")!
    #expect(row.view(row, stringForToolTip: 0, point: CGPoint(x: row.badgeRect.midX, y: row.badgeRect.midY), userData: nil)
        == "❤️ by Nora Lind")
}

@MainActor @Test func theBubbleTellsVoiceOverItsReactions() {
    let row = stream([question, heart]).bubbleRow("q")!
    #expect(row.accessibilityLabel()?.hasSuffix("Reactions: ❤️ by Nora Lind") == true)
}

// MARK: decorative images

private let logo = MermozKit.Attachment(id: "logo", filename: "image001.png", mime: "image/png", size: 4_000, inline: true,
                                        decorative: true)
private let attachedLogo = MermozKit.Attachment(id: "logo2", filename: "image002.png", mime: "image/png", size: 4_000,
                                                decorative: true)

@Test func aSignatureLogoIsNoPastedImage() {
    #expect(bubbles(rows([message("s", date: 1, attachments: [logo])]))[0].thumbnails.isEmpty)
}

@Test func anAttachedSignatureLogoIsNoPhoto() {
    #expect(bubbles(rows([message("s", date: 1, attachments: [attachedLogo])]))[0].photos.isEmpty)
}

@Test func anAttachedSignatureLogoIsNoFile() {
    #expect(bubbles(rows([message("s", date: 1, attachments: [attachedLogo])]))[0].files == 0)
}
