/// Bubble text drawn by TextKit: sizes are measured once per message, text and width;
/// compact bubbles and the whole email show what they showed before.
import AppKit
import Foundation
import Testing
@testable import MermozKit

private let style = BubbleTextStyle()
private let longText = (1...40).map { "Line \($0) of the inspection notes for the flat." }.joined(separator: "\n")

private func message(body: String, new: String? = nil) -> Message {
    Message(id: "m1", threadId: "t1", from: Address(name: "Nora Lind", email: "nora@agency.example"), to: [], cc: [],
            bcc: nil, date: 1, subject: "Flat sale", bodyText: body, bodyNew: new, hasQuoted: new != nil, bodyClean: nil,
            hasHtml: false, isFromMe: false, attachments: [], auth: nil)
}

// MARK: measurement cache

@Test func measuringTheSameBubbleAgainIsAHit() {
    let cache = BubbleTextCache()
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 300, maxLines: 0)
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 300, maxLines: 0)
    #expect(cache.sizeHits == 1)
}

@Test func anotherWidthIsMeasuredAgain() {
    let cache = BubbleTextCache()
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 300, maxLines: 0)
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 200, maxLines: 0)
    #expect(cache.sizeMisses == 2)
}

@Test func changedTextIsMeasuredAgain() {
    let cache = BubbleTextCache()
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 300, maxLines: 0)
    _ = cache.size(id: "m1", text: "See you at ten.", style: style, width: 300, maxLines: 0)
    #expect(cache.sizeMisses == 2)
}

@Test func aLargerFontIsMeasuredAgain() {
    let cache = BubbleTextCache()
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 300, maxLines: 0)
    _ = cache.size(id: "m1", text: "See you at nine.", style: BubbleTextStyle(pointSize: 18), width: 300, maxLines: 0)
    #expect(cache.sizeMisses == 2)
}

@Test func subPointWidthChangesShareAMeasurement() {
    let cache = BubbleTextCache()
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 300.2, maxLines: 0)
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 300.7, maxLines: 0)
    #expect(cache.sizeHits == 1)
}

@Test func aNarrowerBubbleIsTaller() {
    let cache = BubbleTextCache()
    let wide = cache.size(id: "m1", text: longText, style: style, width: 600, maxLines: 0)
    let narrow = cache.size(id: "m1", text: longText, style: style, width: 150, maxLines: 0)
    #expect(narrow.height > wide.height)
}

@Test func aClampedBubbleStopsAtEightLines() {
    let cache = BubbleTextCache()
    let whole = cache.size(id: "m1", text: longText, style: style, width: 600, maxLines: 0)
    let clamped = cache.size(id: "m1", text: longText, style: style, width: 600, maxLines: 8)
    #expect(clamped.height * 4 < whole.height)
}

@Test func preparedBubblesAreHitsWhenShown() {
    let cache = BubbleTextCache()
    // The view lays out once at its width; later conversations are prepared at it.
    _ = cache.size(id: "m0", text: "Hello", style: style, width: 320, maxLines: 0)
    cache.prepare([(id: "m1", content: BubbleContent(text: "See you at nine.", clamped: false))])
    _ = cache.size(id: "m1", text: "See you at nine.", style: style, width: 320, maxLines: 0)
    #expect(cache.sizeHits == 1)
}

@Test func linksInBubbleTextCanBeClicked() {
    let s = BubbleTextCache().attributed(id: "m1", text: "Plans at https://example.com/plans today", style: style)
    #expect(s.attribute(.link, at: 12, effectiveRange: nil) != nil)
}

// MARK: compact and expanded

@Test func aCompactBubbleShowsOnlyWhatTheMessageAdds() {
    let m = message(body: "Fine by me.\n\n> On Monday, Nora wrote: …", new: "Fine by me.")
    #expect(BubbleContent.compact(m, showAll: false).text == "Fine by me.")
}

@Test func theWholeEmailShowsTheQuotedHistoryToo() {
    let m = message(body: "Fine by me.\n\n> On Monday, Nora wrote: …", new: "Fine by me.")
    #expect(BubbleParts.expanded(m).text == m.bodyText)
}

@Test func aLongCompactBubbleIsClamped() {
    #expect(BubbleContent.compact(message(body: longText), showAll: false).clamped)
}

@Test func showMoreLetsTheWholeTextIn() {
    #expect(BubbleContent.compact(message(body: longText), showAll: true).text == longText)
}

@Test func aShortCompactBubbleIsNotClamped() {
    #expect(!BubbleContent.compact(message(body: "See you at nine."), showAll: false).clamped)
}

@Test func anEmptyMessageSaysSo() {
    #expect(BubbleContent.compact(message(body: "  "), showAll: false).text == "(empty message)")
}
