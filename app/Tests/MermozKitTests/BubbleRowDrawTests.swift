/// A bubble row draws its own text from the cached layout, with no text view: the
/// selectable text view is filled and shown only once the pointer comes over it. The
/// drawn text breaks its lines where the measured layout does.
import AppKit
import Testing
@testable import MermozKit

private let paragraph = """
Thanks for the photos from Saturday. The kids loved the boat, and Léa still talks about the \
lighthouse. Shall we do the same trip in May, when the ferries run every hour? I can book \
the cabin if you tell me which weekend suits you best.
"""

private func message(_ id: String, _ body: String, mine: Bool = false, date: Int64 = 1_000) -> Message {
    Message(id: id, threadId: "t", from: Address(name: "Nora", email: "nora@friends.example"), to: [], cc: [], bcc: nil,
            date: date, subject: "S", bodyText: body, bodyNew: nil, hasQuoted: nil, bodyClean: nil, hasHtml: false,
            isFromMe: mine, attachments: [], auth: nil)
}

@MainActor
private final class Shown {
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 700, height: 700), styleMask: .borderless,
                          backing: .buffered, defer: false)

    init(_ messages: [Message], conversation: String = "c") {
        window.contentView = stream
        show(messages, conversation: conversation)
    }

    func show(_ messages: [Message], conversation: String) {
        let rows = ChatRows.build(items: messages.map(TimelineItem.message), hidden: 0, summary: nil, expanded: [],
                                  shownInFull: [], focused: nil, time: { _ in "09:41" }).rows
        stream.show(rows, conversation: conversation, messages: messages.count)
        stream.layoutSubtreeIfNeeded()
        stream.documentView?.layoutSubtreeIfNeeded()
    }

    func row(_ id: String) -> BubbleRowView { stream.bubbleRow(id)! }
}

/// Dark pixels inside the row's text frame, rendered offscreen through draw(_:).
@MainActor
private func textInk(_ row: BubbleRowView) -> Int {
    guard let rep = row.bitmapImageRepForCachingDisplay(in: row.bounds) else { return 0 }
    row.cacheDisplay(in: row.bounds, to: rep)
    let scale = CGFloat(rep.pixelsWide) / max(row.bounds.width, 1)
    let f = row.text.frame
    var ink = 0
    for x in Int(f.minX * scale)..<Int(f.maxX * scale) {
        for y in Int(f.minY * scale)..<Int(f.maxY * scale) {
            if let c = rep.colorAt(x: x, y: y), c.alphaComponent > 0.3, c.brightnessComponent < 0.6 { ink += 1 }
        }
    }
    return ink
}

@MainActor @Test func aRowDrawsItsTextWithoutATextView() {
    let s = Shown([message("a", paragraph)])
    #expect(textInk(s.row("a")) > 200)
}

@MainActor @Test func theTextViewStaysHiddenWhileStepping() {
    let s = Shown([message("a", paragraph)])
    #expect(s.row("a").text.isHidden)
}

@MainActor @Test func theTextViewHoldsNoTextWhileStepping() {
    let s = Shown([message("a", paragraph)])
    #expect(s.row("a").text.textStorage?.length == 0)
}

@MainActor @Test func thePointerOverTheBubbleMakesItsTextSelectable() {
    let s = Shown([message("a", paragraph)])
    let row = s.row("a")
    let enter = NSEvent.enterExitEvent(with: .mouseEntered, location: .zero, modifierFlags: [], timestamp: 0,
                                       windowNumber: s.window.windowNumber, context: nil, eventNumber: 0,
                                       trackingNumber: 0, userData: nil)!
    row.mouseEntered(with: enter)
    #expect(row.text.textStorage?.string == paragraph)
}

@MainActor @Test func aLiveTextViewIsShown() {
    let s = Shown([message("a", paragraph)])
    s.row("a").liveText(true)
    #expect(!s.row("a").text.isHidden)
}

@MainActor @Test func aLiveTextViewDrawsTheText() {
    let s = Shown([message("a", paragraph)])
    s.row("a").liveText(true)
    #expect(inkPixels(s.row("a").text) > 200)
}

@MainActor @Test func theNextConversationDrawsItsTextAgain() {
    let s = Shown([message("a", paragraph)])
    s.row("a").liveText(true)
    s.show([message("b", "See you at nine, by the station.")], conversation: "c2")
    #expect(!s.row("b").textIsLive)
}

@MainActor @Test func aRecycledRowDrawsTheNewText() {
    let s = Shown([message("a", paragraph)])
    s.show([message("b", "See you at nine, by the station.")], conversation: "c2")
    #expect(textInk(s.row("b")) > 50)
}

@MainActor @Test func aClampedRowDrawsItsText() {
    let long = (1...30).map { "Line \($0) of the minutes, with the decisions taken." }.joined(separator: "\n")
    let s = Shown([message("l", long)])
    #expect(textInk(s.row("l")) > 200)
}

@MainActor @Test func aRowSaysItsTextToAccessibility() {
    let s = Shown([message("a", paragraph)])
    #expect(s.row("a").accessibilityValue() as? String == paragraph)
}

// MARK: same lines as measured

@Test(arguments: [180.0, 260.0, 340.0, 520.0])
func theDrawnTextTakesTheMeasuredHeight(width: Double) {
    let laid = BubbleTextCache().layout(id: "p", text: paragraph, style: BubbleTextStyle(), width: width, maxLines: 0)
    let drawn = laid.drawn.boundingRect(with: CGSize(width: laid.container.size.width, height: .greatestFiniteMagnitude),
                                        options: [.usesLineFragmentOrigin])
    #expect(abs(ceil(drawn.height) - laid.size.height) <= 1)
}

@Test func aLinkIsUnderlinedInTheDrawnText() {
    let laid = BubbleTextCache().layout(id: "u", text: "The form is at https://example.com/form today.", style: BubbleTextStyle(),
                                        width: 400, maxLines: 0)
    let i = (laid.drawn.string as NSString).range(of: "https://example.com/form").location
    #expect(laid.drawn.attribute(.underlineStyle, at: i, effectiveRange: nil) as? Int == NSUnderlineStyle.single.rawValue)
}
