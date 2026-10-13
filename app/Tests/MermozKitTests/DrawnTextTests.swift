/// The conversation title draws its cached layout and fills a selectable text view only
/// when the pointer comes over it; a new title of the same size replaces the old one.
import AppKit
import Testing
@testable import MermozKit

private let style = BubbleTextStyle(pointSize: 17, lineSpacing: 0, semibold: true, links: false)

private func laid(_ text: String) -> BubbleTextLayout.Laid {
    BubbleTextCache().layout(id: BubbleTextCache.titleId, text: text, style: style, width: 600, maxLines: 2, use: .title)
}

@MainActor
private func shown(_ text: String) -> (DrawnText, NSWindow) {
    let v = DrawnText()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 640, height: 200), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView?.addSubview(v)
    let l = laid(text)
    v.offer(l)
    v.frame = CGRect(origin: CGPoint(x: 10, y: 10), size: l.size)
    v.layoutSubtreeIfNeeded()
    return (v, window)
}

@MainActor @Test func aTitleDrawsItsText() {
    let (v, w) = shown("Flat sale: the buyers signed")
    #expect(inkPixels(v) > 100)
    _ = w
}

@MainActor @Test func aTitleFillsNoTextViewWhileStepping() {
    let (v, w) = shown("Flat sale: the buyers signed")
    #expect(v.live.textStorage?.length == 0)
    _ = w
}

@MainActor @Test func aNewTitleOfTheSameSizeReplacesTheOld() {
    // Two titles of nearly one width: the frame barely changes, the text must.
    let (v, w) = shown("Keys at the desk")
    let next = laid("Keys at the door")
    v.offer(next)
    v.setFrameSize(next.size)
    v.layoutSubtreeIfNeeded()
    #expect(v.laid === next)
    _ = w
}

@MainActor @Test func thePointerOverATitleMakesItSelectable() {
    let (v, w) = shown("Flat sale: the buyers signed")
    let enter = NSEvent.enterExitEvent(with: .mouseEntered, location: .zero, modifierFlags: [], timestamp: 0,
                                       windowNumber: w.windowNumber, context: nil, eventNumber: 0, trackingNumber: 0,
                                       userData: nil)!
    v.mouseEntered(with: enter)
    #expect(v.live.textStorage?.string == "Flat sale: the buyers signed")
}

@MainActor @Test func aLiveTitleGoesBackToDrawingForTheNextConversation() {
    let (v, w) = shown("Flat sale: the buyers signed")
    v.goLive(true)
    v.offer(laid("Keys at the desk"))
    v.layoutSubtreeIfNeeded()
    #expect(!v.isLive)
    _ = w
}

@MainActor @Test func aTitleSaysItsTextToAccessibility() {
    let (v, w) = shown("Flat sale: the buyers signed")
    #expect(v.accessibilityValue() as? String == "Flat sale: the buyers signed")
    _ = w
}
