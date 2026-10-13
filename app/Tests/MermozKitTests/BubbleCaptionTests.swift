/// A forwarded email's bubble says who wrote the original ("Forwarded from Nora"); a
/// message with no words of its own reads as a quote, dimmed beside a bar.
import AppKit
import Testing
@testable import MermozKit

private let note = "FYI, see below. The keys are at the agency from Monday on, ask for Léa at the front desk."

private func message(_ id: String, forwarded: Bool = false, from original: String? = nil, quotedOnly: Bool = false) -> Message {
    var m = Message(id: id, threadId: "t", from: Address(name: "Sam", email: "sam@work.example"), to: [], cc: [], bcc: nil,
                    date: 1_000, subject: "Fwd: Keys", bodyText: note, bodyNew: note, hasQuoted: nil, bodyClean: nil,
                    hasHtml: false, isFromMe: false, attachments: [], auth: nil)
    m.forwarded = forwarded
    m.forwardedFrom = original
    m.quotedOnly = quotedOnly
    return m
}

private func bubble(_ m: Message) -> BubbleSpec? {
    ChatRows.build(items: [.message(m)], hidden: 0, summary: nil, expanded: [], shownInFull: [], focused: nil,
                   time: { _ in "09:41" }).rows.compactMap { if case let .bubble(b) = $0 { b } else { nil } }.first
}

// MARK: caption

@Test func aForwardSaysWhoWroteTheOriginal() {
    #expect(BubbleCaption.of(message("f", forwarded: true, from: "Nora Lind")) == "Forwarded from Nora Lind")
}

@Test func aForwardFromSomeoneUnknownSaysForwarded() {
    #expect(BubbleCaption.of(message("f", forwarded: true)) == "Forwarded")
}

@Test func aMessageThatForwardsNothingHasNoCaption() {
    #expect(BubbleCaption.of(message("m")) == nil)
}

@Test func theBubbleCarriesTheCaption() {
    #expect(bubble(message("f", forwarded: true, from: "Nora"))?.caption == "Forwarded from Nora")
}

@Test func aCaptionedBubbleIsTallerByTheCaption() {
    let plain = bubble(message("m"))!, forwarded = bubble(message("f", forwarded: true, from: "Nora"))!
    #expect(ChatMetrics.bubble(text: CGSize(width: 200, height: 40), forwarded)
        - ChatMetrics.bubble(text: CGSize(width: 200, height: 40), plain) == ChatMetrics.caption)
}

// MARK: quote only

@Test func aMessageWithNoWordsOfItsOwnIsAQuote() {
    #expect(bubble(message("q", quotedOnly: true))?.quoted == true)
}

@Test func aMessageWithItsOwnWordsIsNotAQuote() {
    #expect(bubble(message("m"))?.quoted == false)
}

// MARK: drawn

@MainActor
private final class Shown {
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 700, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)

    init(_ messages: [Message]) {
        window.contentView = stream
        stream.show(ChatRows.build(items: messages.map(TimelineItem.message), hidden: 0, summary: nil, expanded: [],
                                   shownInFull: [], focused: nil, time: { _ in "09:41" }).rows,
                    conversation: "c", messages: messages.count)
        stream.layoutSubtreeIfNeeded()
        stream.documentView?.layoutSubtreeIfNeeded()
    }
}

@MainActor @Test func theTextSitsUnderTheCaption() {
    let row = Shown([message("f", forwarded: true, from: "Nora")]).stream.bubbleRow("f")!
    #expect(row.text.frame.minY >= row.bubbleRect.minY + ChatMetrics.inset.height + ChatMetrics.caption)
}

@MainActor @Test func aShortForwardIsWideEnoughForItsCaption() {
    var m = message("f", forwarded: true, from: "Nora Lindqvist-Bergström")
    m.bodyNew = "FYI"
    m.bodyText = "FYI"
    let row = Shown([m]).stream.bubbleRow("f")!
    #expect(row.bubbleRect.width >= ChatMetrics.captionWidth("Forwarded from Nora Lindqvist-Bergström") + 2 * ChatMetrics.inset.width)
}

/// Dark pixels in the row's text frame.
@MainActor
private func darkText(_ row: BubbleRowView) -> Int {
    guard let rep = row.bitmapImageRepForCachingDisplay(in: row.bounds) else { return 0 }
    row.cacheDisplay(in: row.bounds, to: rep)
    let scale = CGFloat(rep.pixelsWide) / max(row.bounds.width, 1)
    let f = row.text.frame
    var ink = 0
    for x in Int(f.minX * scale)..<Int(f.maxX * scale) {
        for y in Int(f.minY * scale)..<Int(f.maxY * scale) {
            if let c = rep.colorAt(x: x, y: y), c.alphaComponent > 0.3, c.brightnessComponent < 0.35 { ink += 1 }
        }
    }
    return ink
}

@MainActor @Test func aQuoteIsDrawnLighterThanOwnWords() {
    let s = Shown([message("m"), message("q", quotedOnly: true)])
    #expect(darkText(s.stream.bubbleRow("q")!) < darkText(s.stream.bubbleRow("m")!))
}

@MainActor @Test func aQuotesSelectableTextIsDimmedToo() {
    let s = Shown([message("q", quotedOnly: true)])
    #expect(s.stream.bubbleText("q")?.alphaValue == ChatMetrics.quotedAlpha)
}
