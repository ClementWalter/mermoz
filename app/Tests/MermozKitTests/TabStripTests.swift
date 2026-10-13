/// The conversation tabs drawn as one view: laid out left to right, a count widens its
/// tab, a click or an accessibility press selects one.
import AppKit
import Testing
@testable import MermozKit

@MainActor private let colors = TabStrip.Colors(text: .gray, count: .lightGray, selected: .blue, highlight: .blue.withAlphaComponent(0.2))
private let tabs = [TabStrip.Tab(title: "Messages"), TabStrip.Tab(title: "People", count: 3), TabStrip.Tab(title: "Docs", count: 0)]

@MainActor
private func strip(selected: Int = 0) -> TabStrip {
    let s = TabStrip()
    s.show(tabs, selected: selected, colors: colors)
    s.frame = CGRect(origin: .zero, size: s.intrinsicContentSize)
    return s
}

@MainActor @Test func tabsRunLeftToRightWithoutOverlap() {
    let f = strip().frames
    #expect(zip(f, f.dropFirst()).allSatisfy { $0.maxX < $1.minX })
}

@MainActor @Test func aCountWidensItsTab() {
    #expect(TabStrip.width(TabStrip.Tab(title: "People", count: 3)) > TabStrip.width(TabStrip.Tab(title: "People")))
}

@MainActor @Test func aZeroCountIsNotShown() {
    #expect(TabStrip.width(TabStrip.Tab(title: "Docs", count: 0)) == TabStrip.width(TabStrip.Tab(title: "Docs")))
}

@MainActor @Test func theStripIsAsWideAsItsTabs() {
    let s = strip()
    #expect(s.intrinsicContentSize.width == s.frames.last!.maxX)
}

@MainActor @Test func pressingATabSelectsIt() {
    let s = strip()
    var chosen: Int?
    s.onSelect = { chosen = $0 }
    s.press(1)
    #expect(chosen == 1)
}

@MainActor @Test func everyTabIsAnAccessibilityElement() {
    #expect(strip().accessibilityChildren()?.count == 3)
}

@MainActor @Test func theSelectedTabSaysSoToAccessibility() {
    let e = strip(selected: 1).accessibilityChildren()?[1] as? NSAccessibilityElement
    #expect(e?.accessibilityValue() as? Int == 1)
}

@MainActor @Test func anAccessibilityPressSelectsTheTab() {
    let s = strip()
    var chosen: Int?
    s.onSelect = { chosen = $0 }
    _ = (s.accessibilityChildren()?[2] as? NSAccessibilityElement)?.accessibilityPerformPress()
    #expect(chosen == 2)
}

@MainActor @Test func theSelectedTabIsHighlighted() {
    let s = strip(selected: 1)
    guard let rep = s.bitmapImageRepForCachingDisplay(in: s.bounds) else { return }
    s.cacheDisplay(in: s.bounds, to: rep)
    let scale = CGFloat(rep.pixelsWide) / s.bounds.width
    let corner = s.frames[1].insetBy(dx: 3, dy: 3).origin
    let c = rep.colorAt(x: Int(corner.x * scale), y: Int(corner.y * scale))
    #expect((c?.alphaComponent ?? 0) > 0.1)
}
