/// Stepping through conversations does its work ahead: bubbles are measured off the main
/// thread at every width the chat lays them out at (mine and theirs differ), a subject
/// title does not move those widths, and row views are kept for the next conversation.
import AppKit
import Testing
@testable import MermozKit

private let style = BubbleTextStyle()
private let title = BubbleTextStyle(semibold: true, links: false)
private let text = "See you Friday at the station, the train leaves at 9:12 from platform 4."

private func content(_ s: String = text) -> BubbleContent { BubbleContent(text: s, clamped: false) }

/// A cache that has laid bubbles out at 300 (mine) and 340 (theirs).
private func warmed() -> BubbleTextCache {
    let c = BubbleTextCache()
    _ = c.layout(id: "a", text: text, style: style, width: 300, maxLines: 0)
    _ = c.layout(id: "b", text: text, style: style, width: 340, maxLines: 0)
    return c
}

@Test func preparedBubblesAreReadyAtTheMostRecentWidth() {
    let c = warmed()
    c.prepare([(id: "n", content: content())])
    let hits = c.sizeHits
    _ = c.layout(id: "n", text: text, style: style, width: 340, maxLines: 0)
    #expect(c.sizeHits == hits + 1)
}

@Test func preparedBubblesAreReadyAtTheOtherSidesWidth() {
    let c = warmed()
    c.prepare([(id: "n", content: content())])
    let hits = c.sizeHits
    _ = c.layout(id: "n", text: text, style: style, width: 300, maxLines: 0)
    #expect(c.sizeHits == hits + 1)
}

@Test func aTitleLayoutKeepsTheBubbleWidths() {
    let c = warmed()
    _ = c.layout(id: BubbleTextCache.titleId, text: "Trip", style: title, width: 900, maxLines: 2, use: .title)
    c.prepare([(id: "n", content: content())])
    let hits = c.sizeHits
    _ = c.layout(id: "n", text: text, style: style, width: 300, maxLines: 0)
    #expect(c.sizeHits == hits + 1)
}

@Test func aPreparedTitleIsReadyAtTheTitlesWidth() {
    let c = BubbleTextCache()
    _ = c.layout(id: BubbleTextCache.titleId, text: "Trip", style: title, width: 900, maxLines: 2, use: .title)
    c.prepareTitle("School trip on Friday")
    let hits = c.sizeHits
    _ = c.layout(id: BubbleTextCache.titleId, text: "School trip on Friday", style: title, width: 900, maxLines: 2, use: .title)
    #expect(c.sizeHits == hits + 1)
}

// MARK: row views

private func message(_ id: String, date: Int64) -> Message {
    Message(id: id, threadId: "t", from: Address(name: "Aude", email: "aude@example.com"), to: [], cc: [], bcc: nil,
            date: date, subject: "S", bodyText: "Message \(id).", bodyNew: nil, hasQuoted: nil, bodyClean: nil,
            hasHtml: false, isFromMe: false, attachments: [], auth: nil)
}

private func rows(_ n: Int, prefix: String) -> [ChatRowSpec] {
    ChatRows.build(items: (0..<n).map { .message(message("\(prefix)\($0)", date: Int64($0) * 1_000)) }, hidden: 0,
                   summary: nil, expanded: [], shownInFull: [], focused: nil, time: { _ in "09:41" }).rows
}

@MainActor
private func stream() -> ChatStreamView {
    let s = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 640, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView = s
    return s
}

@MainActor @Test func aShorterConversationKeepsTheOtherBubbleViewsAsSpares() {
    let s = stream()
    s.show(rows(6, prefix: "a"), conversation: "a", messages: 6)
    s.show(rows(2, prefix: "b"), conversation: "b", messages: 2)
    #expect(s.spareCount >= 4)
}

@MainActor @Test func aLongerConversationAfterAShortOneTakesTheSpareViews() {
    let s = stream()
    s.show(rows(6, prefix: "a"), conversation: "a", messages: 6)
    let first = Set(s.rowViews.map(ObjectIdentifier.init))
    s.show(rows(2, prefix: "b"), conversation: "b", messages: 2)
    s.show(rows(6, prefix: "c"), conversation: "c", messages: 6)
    #expect(Set(s.rowViews.map(ObjectIdentifier.init)) == first)
}

@MainActor @Test func spareViewsAreHidden() {
    let s = stream()
    s.show(rows(6, prefix: "a"), conversation: "a", messages: 6)
    s.show(rows(2, prefix: "b"), conversation: "b", messages: 2)
    let shown = Set(s.rowViews.map(ObjectIdentifier.init))
    let visible = s.documentView?.subviews.filter { !$0.isHidden && !shown.contains(ObjectIdentifier($0)) } ?? []
    #expect(visible.isEmpty)
}

@MainActor @Test func aSpareViewShownAgainIsVisible() {
    let s = stream()
    s.show(rows(6, prefix: "a"), conversation: "a", messages: 6)
    s.show(rows(2, prefix: "b"), conversation: "b", messages: 2)
    s.show(rows(6, prefix: "c"), conversation: "c", messages: 6)
    #expect(s.rowViews.allSatisfy { !$0.isHidden })
}
