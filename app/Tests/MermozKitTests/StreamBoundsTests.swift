/// Nothing in the chat goes past the pane's edge, at any width: bubbles, their photo grid
/// and pasted images stay inside the stream; the bubble's tooltip shows only over the
/// bubble where it is on screen.
import AppKit
import Testing
@testable import MermozKit

private func photo(_ id: String) -> MermozKit.Attachment { MermozKit.Attachment(id: id, filename: "\(id).jpg", mime: "image/jpeg", size: 1) }

private func message(_ id: String, _ body: String, mine: Bool = false, attachments: [MermozKit.Attachment] = [],
                     forwardedFrom: String? = nil, date: Int64) -> Message {
    var m = Message(id: id, threadId: "t", from: Address(name: mine ? "Me" : "Elliot", email: mine ? "me@x.com" : "elliot@x.com"),
                    to: [], cc: [], bcc: nil, date: date, subject: "S", bodyText: body, bodyNew: body, hasQuoted: nil,
                    bodyClean: nil, hasHtml: false, isFromMe: mine, attachments: attachments, auth: nil)
    if let forwardedFrom { m.forwarded = true; m.forwardedFrom = forwardedFrom }
    return m
}

private let letter = (1...30).map { "Line \($0) of the minutes, with what was decided." }.joined(separator: "\n")

/// Every kind of wide content: photos (theirs, mine), pasted images, a long caption on a
/// short note, a clamped letter.
private let items: [TimelineItem] = [
    .message(message("p5", "Photos!", attachments: (0..<5).map { photo("a\($0)") }, date: 1)),
    .message(message("p3", "Mine", mine: true, attachments: (0..<3).map { photo("b\($0)") }, date: 2)),
    .message(message("i4", "Pasted", attachments: (0..<4).map {
        MermozKit.Attachment(id: "c\($0)", filename: "image\($0).png", mime: "image/png", size: 1, inline: true)
    }, date: 3)),
    .message(message("fw", "FYI", forwardedFrom: "Nora Lindqvist-Bergström from the property management office", date: 4)),
    .message(message("cl", letter, date: 5)),
]

@MainActor
private func stream(width: CGFloat) -> ChatStreamView {
    let s = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: width, height: 900), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView = s
    s.show(ChatRows.build(items: items, hidden: 0, summary: nil, expanded: [], shownInFull: [], focused: nil,
                          time: { _ in "09:41" }).rows, conversation: "c", messages: items.count)
    s.layoutSubtreeIfNeeded()
    s.documentView?.layoutSubtreeIfNeeded()
    return s
}

@MainActor
private func bubbleRows(_ s: ChatStreamView) -> [BubbleRowView] { s.rowViews.compactMap { $0 as? BubbleRowView } }

@MainActor @Test(arguments: [360.0, 640.0, 1_100.0])
func noRowGoesPastTheStream(width: Double) {
    let s = stream(width: width)
    let bounds = s.documentView!.bounds
    #expect(s.rowViews.allSatisfy { bounds.contains($0.frame) })
}

@MainActor @Test(arguments: [360.0, 640.0, 1_100.0])
func noBubbleGoesPastTheStream(width: Double) {
    let s = stream(width: width)
    let w = s.contentView.bounds.width
    #expect(bubbleRows(s).allSatisfy { $0.bubbleRect.minX >= 0 && $0.bubbleRect.maxX <= w })
}

@MainActor @Test(arguments: [360.0, 640.0, 1_100.0])
func thePhotoGridStaysInItsBubble(width: Double) {
    let s = stream(width: width)
    #expect(bubbleRows(s).allSatisfy { r in r.photoRects.allSatisfy { r.bubbleRect.contains($0) } })
}

@MainActor @Test(arguments: [360.0, 640.0, 1_100.0])
func pastedImagesStayInTheirBubble(width: Double) {
    let s = stream(width: width)
    #expect(bubbleRows(s).allSatisfy { r in r.thumbRects.allSatisfy { r.bubbleRect.contains($0) } })
}

@MainActor @Test(arguments: [360.0, 640.0, 1_100.0])
func everyRowHasRoomForItsBubble(width: Double) {
    let s = stream(width: width)
    #expect(bubbleRows(s).allSatisfy { $0.bounds.contains($0.bubbleRect) })
}

@Test func aNarrowGridScalesDown() {
    #expect(ChatMetrics.photoTiles(4, width: 150).map(\.maxX).max() == 150)
}

@Test func pastedImagesWrapInANarrowBubble() {
    #expect(ChatMetrics.thumbTiles(4, width: 200).map(\.minY).max()! > 0)
}

// MARK: tooltip

@MainActor @Test func aRowHasNoTooltipOfItsOwn() {
    #expect(bubbleRows(stream(width: 640)).allSatisfy { $0.toolTip == nil })
}

@Test func theTooltipShowsOverTheVisibleBubble() {
    #expect(BubbleRowView.tooltip(at: CGPoint(x: 50, y: 50), bubble: CGRect(x: 0, y: 0, width: 200, height: 100),
                                  visible: CGRect(x: 0, y: 0, width: 640, height: 400)) == "Open the whole email")
}

@Test func theTooltipNeverShowsWhereTheBubbleIsScrolledAway() {
    #expect(BubbleRowView.tooltip(at: CGPoint(x: 50, y: 50), bubble: CGRect(x: 0, y: 0, width: 200, height: 100),
                                  visible: CGRect(x: 0, y: 80, width: 640, height: 400)) == "")
}

@Test func theTooltipNeverShowsBesideTheBubble() {
    #expect(BubbleRowView.tooltip(at: CGPoint(x: 300, y: 50), bubble: CGRect(x: 0, y: 0, width: 200, height: 100),
                                  visible: CGRect(x: 0, y: 0, width: 640, height: 400)) == "")
}

@MainActor @Test func aSpareRowShowsNoTooltip() {
    let s = stream(width: 640)
    let rows = bubbleRows(s)
    s.show(ChatRows.build(items: [items[0]], hidden: 0, summary: nil, expanded: [], shownInFull: [], focused: nil,
                          time: { _ in "09:41" }).rows, conversation: "c2", messages: 1)
    let spare = rows.first { $0.isHidden }!
    #expect(spare.view(spare, stringForToolTip: 0, point: CGPoint(x: spare.bubbleRect.midX, y: spare.bubbleRect.midY),
                       userData: nil) == "")
}
