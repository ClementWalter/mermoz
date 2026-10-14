/// Slide to reply on a bubble: a sideways swipe past the threshold, or ↩ on hover, pins
/// the reply to that message; up and down scrolls stay with the chat.
import AppKit
import Testing
@testable import MermozKit

// MARK: the swipe

@Test func aShortSwipePinsNothing() {
    var s = SwipeToReply()
    s.move(20)
    let pinned = s.end()
    #expect(!pinned)
}

@Test func aSwipePastTheThresholdPins() {
    var s = SwipeToReply()
    s.move(30)
    s.move(30)
    let pinned = s.end()
    #expect(pinned)
}

@Test func aSwipeTheOtherWayPinsToo() {
    var s = SwipeToReply()
    s.move(-70)
    let pinned = s.end()
    #expect(pinned)
}

@Test func theBubbleFollowsTheFingersOnlySoFar() {
    var s = SwipeToReply()
    #expect(s.move(300) == SwipeToReply.maxOffset)
}

@Test func anUpAndDownScrollIsNoSwipe() {
    #expect(!SwipeToReply().takes(dx: 3, dy: 12))
}

@Test func aSidewaysScrollIsASwipe() {
    #expect(SwipeToReply().takes(dx: 12, dy: 2))
}

@Test func aSwipeStartsOverAfterLifting() {
    var s = SwipeToReply()
    s.move(70)
    _ = s.end()
    s.move(10)
    let pinned = s.end()
    #expect(!pinned)
}

// MARK: on a bubble

private func message(_ id: String, _ body: String, mine: Bool = false, date: Int64) -> Message {
    Message(id: id, threadId: "t", from: Address(name: mine ? "Me" : "Hélène Adam", email: mine ? "me@x.com" : "adam@x.com"),
            to: [], cc: [], bcc: nil, date: date, subject: "S", bodyText: body, bodyNew: body, hasQuoted: nil, bodyClean: nil,
            hasHtml: false, isFromMe: mine, attachments: [], auth: nil)
}

@MainActor
private final class Shown {
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 640, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
    var replied: [String] = []

    init() {
        window.contentView = stream
        stream.actions.replyTo = { [weak self] in self?.replied.append($0) }
        let items: [TimelineItem] = [.message(message("a", "Cordialement, le service des ventes.", date: 1)),
                                     .message(message("b", "Merci !", mine: true, date: 2))]
        stream.show(ChatRows.build(items: items, hidden: 0, summary: nil, expanded: [], shownInFull: [], focused: nil,
                                   time: { _ in "09:41" }).rows, conversation: "c", messages: 2)
        stream.layoutSubtreeIfNeeded()
        stream.documentView?.layoutSubtreeIfNeeded()
    }

    func row(_ id: String) -> BubbleRowView { stream.bubbleRow(id)! }

    var entered: NSEvent {
        NSEvent.enterExitEvent(with: .mouseEntered, location: .zero, modifierFlags: [], timestamp: 0,
                               windowNumber: window.windowNumber, context: nil, eventNumber: 0, trackingNumber: 0, userData: nil)!
    }
}

@MainActor @Test func swipingABubbleFarEnoughPinsTheReplyToIt() {
    let s = Shown()
    s.row("a").swipeStep(dx: 35, ended: false)
    s.row("a").swipeStep(dx: 35, ended: false)
    s.row("a").swipeStep(dx: 0, ended: true)
    #expect(s.replied == ["a"])
}

@MainActor @Test func aShortSwipeOnABubblePinsNothing() {
    let s = Shown()
    s.row("a").swipeStep(dx: 20, ended: false)
    s.row("a").swipeStep(dx: 0, ended: true)
    #expect(s.replied.isEmpty)
}

@MainActor @Test func theBubbleComesBackWhenTheFingersLift() {
    let s = Shown()
    s.row("a").swipeStep(dx: 40, ended: false)
    s.row("a").swipeStep(dx: 0, ended: true)
    #expect(s.row("a").bounds.origin == .zero)
}

@MainActor @Test func theBubbleFollowsASwipe() {
    let s = Shown()
    s.row("a").swipeStep(dx: 40, ended: false)
    #expect(s.row("a").swipeOffset == 40)
}

@MainActor @Test func hoveringABubbleShowsReply() {
    let s = Shown()
    s.row("a").mouseEntered(with: s.entered)
    #expect(s.row("a").replyButton?.isHidden == false)
}

@MainActor @Test func theReplyButtonSitsBesideTheirBubble() {
    let s = Shown()
    let row = s.row("a")
    row.mouseEntered(with: s.entered)
    #expect(row.replyButton!.frame.minX >= row.bubbleRect.maxX)
}

@MainActor @Test func theReplyButtonSitsBesideMyBubble() {
    let s = Shown()
    let row = s.row("b")
    row.mouseEntered(with: s.entered)
    #expect(row.replyButton!.frame.maxX <= row.bubbleRect.minX)
}

@MainActor @Test func pressingReplyPinsTheReplyToThatMessage() {
    let s = Shown()
    s.row("a").mouseEntered(with: s.entered)
    s.row("a").replyButton?.performClick(nil)
    #expect(s.replied == ["a"])
}

@MainActor @Test func leavingTheBubbleHidesReply() {
    let s = Shown()
    s.row("a").mouseEntered(with: s.entered)
    s.row("a").mouseExited(with: s.entered)
    #expect(s.row("a").replyButton?.isHidden == true)
}

// MARK: the preview

@Test func thePreviewNamesTheSenderAndQuotesTheStart() {
    #expect(ReplyQuote.preview(message("a", "Cordialement,\nle service des ventes.", date: 1), sender: "Mme Adam")
        == "Replying to Mme Adam: “Cordialement, le service des ventes.”")
}

@Test func aLongMessageIsCutInThePreview() {
    let long = String(repeating: "word ", count: 40)
    #expect(ReplyQuote.preview(message("a", long, date: 1), sender: "X").hasSuffix("…”"))
}

@Test func myOwnMessageIsQuotedAsMine() {
    #expect(ReplyQuote.preview(message("b", "Merci !", mine: true, date: 1), sender: "Me") == "Replying to You: “Merci !”")
}
