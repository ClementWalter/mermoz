/// A bubble's text view draws its cached layout. The view and its container must be
/// linked both ways, or the bubble stays blank on screen until a click forces a redraw.
import AppKit
import Testing
@testable import MermozKit

/// Dark pixels in the view's drawing, rendered offscreen through draw(_:).
@MainActor
private func inkPixels(_ v: NSView) -> Int {
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

/// A bubble text view showing the cache's layout, as the reading pane does.
@MainActor
private func shown(_ cache: BubbleTextCache) -> (NSTextView, BubbleTextLayout.Laid) {
    let laid = cache.layout(id: "m1", text: text, style: BubbleTextStyle(), width: 300, maxLines: 0)
    let v = NSTextView(usingTextLayoutManager: false)
    v.drawsBackground = false
    v.textContainerInset = .zero
    v.frame = CGRect(origin: .zero, size: laid.size)
    let drawn = BubbleTextLayout.attach(laid, to: v, replacing: nil)
    return (v, drawn)
}

@MainActor @Test func aShownBubbleDrawsItsText() {
    #expect(inkPixels(shown(BubbleTextCache()).0) > 50)
}

@MainActor @Test func theContainerKnowsTheViewItDrawsInto() {
    let (v, _) = shown(BubbleTextCache())
    #expect(v.textContainer?.textView === v)
}

@MainActor @Test func theLayoutManagerDrawsIntoTheBubble() {
    let (v, drawn) = shown(BubbleTextCache())
    #expect(drawn.manager.firstTextView === v)
}

@MainActor @Test func aSecondViewOfTheSameMessageGetsItsOwnContainer() {
    let cache = BubbleTextCache()
    let (first, _) = shown(cache)
    let (second, _) = shown(cache)
    #expect(first.textContainer !== second.textContainer)
}

@MainActor @Test func theFirstViewKeepsDrawingWhenASecondShowsTheSameMessage() {
    let cache = BubbleTextCache()
    let (first, _) = shown(cache)
    _ = shown(cache)
    #expect(first.textContainer?.textView === first)
}

@MainActor @Test func aSecondViewOfTheSameMessageDrawsToo() {
    let cache = BubbleTextCache()
    _ = shown(cache)
    #expect(inkPixels(shown(cache).0) > 50)
}
