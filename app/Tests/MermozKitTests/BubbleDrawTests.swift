/// A bubble's text view draws its text: it keeps its own text stack, filled from the
/// cache's layout (text, width, line limit), and renders ink in its text area.
import AppKit
import Testing
@testable import MermozKit

/// Dark pixels in the view's drawing, rendered offscreen through draw(_:).
@MainActor
func inkPixels(_ v: NSView) -> Int {
    guard let rep = v.bitmapImageRepForCachingDisplay(in: v.bounds) else { return 0 }
    v.cacheDisplay(in: v.bounds, to: rep)
    var ink = 0
    for x in 0..<rep.pixelsWide {
        for y in 0..<rep.pixelsHigh {
            if let c = rep.colorAt(x: x, y: y), c.alphaComponent > 0.3, c.brightnessComponent < 0.6 { ink += 1 }
        }
    }
    return ink
}

private let text = "See you at nine, by the station."

@MainActor
private func shown(_ cache: BubbleTextCache, maxLines: Int = 0) -> SelectableBubbleText {
    let laid = cache.layout(id: "m1", text: text, style: BubbleTextStyle(), width: 300, maxLines: maxLines)
    let v = SelectableBubbleText.make()
    v.frame = CGRect(origin: .zero, size: laid.size)
    v.show(laid)
    return v
}

@MainActor @Test func aShownBubbleDrawsItsText() {
    #expect(inkPixels(shown(BubbleTextCache())) > 50)
}

@MainActor @Test func aShownBubbleHoldsItsText() {
    #expect(shown(BubbleTextCache()).textStorage?.string == text)
}

@MainActor @Test func theViewDrawsItsOwnContainer() {
    let v = shown(BubbleTextCache())
    #expect(v.textContainer?.textView === v)
}

@MainActor @Test func aClampedBubbleDrawsItsText() {
    #expect(inkPixels(shown(BubbleTextCache(), maxLines: 8)) > 50)
}

@MainActor @Test func twoViewsOfOneMessageBothDraw() {
    let cache = BubbleTextCache()
    let first = shown(cache)
    _ = shown(cache)
    #expect(inkPixels(first) > 50)
}

@MainActor @Test func twoViewsOfOneMessageHaveTheirOwnContainers() {
    let cache = BubbleTextCache()
    #expect(shown(cache).textContainer !== shown(cache).textContainer)
}
