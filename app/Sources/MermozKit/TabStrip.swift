/// A row of tabs drawn as one view: each tab's title, an optional count after it, and a
/// soft rounded highlight on the selected one. A SwiftUI row of buttons rebuilt its
/// views on every conversation step; this draws a few strings.
import AppKit

@MainActor
public final class TabStrip: NSView {
    public struct Tab: Equatable, Sendable {
        public var title: String
        /// Shown after the title in a quieter color when above zero.
        public var count: Int?

        public init(title: String, count: Int? = nil) { self.title = title; self.count = count }
    }

    public struct Colors: Equatable {
        public var text: NSColor
        public var count: NSColor
        public var selected: NSColor
        public var highlight: NSColor

        public init(text: NSColor, count: NSColor, selected: NSColor, highlight: NSColor) {
            self.text = text; self.count = count; self.selected = selected; self.highlight = highlight
        }
    }

    public static let height: CGFloat = 24
    static let padding: CGFloat = 9
    static let spacing: CGFloat = 2
    static let countGap: CGFloat = 4
    static let font = NSFont.systemFont(ofSize: 12, weight: .medium)

    public private(set) var tabs: [Tab] = []
    public private(set) var selected = 0
    private var colors = Colors(text: .secondaryLabelColor, count: .tertiaryLabelColor, selected: .controlAccentColor,
                                highlight: .controlAccentColor.withAlphaComponent(0.15))
    /// Each tab's rectangle.
    public private(set) var frames: [CGRect] = []
    public var onSelect: (Int) -> Void = { _ in }
    private var elements: [NSAccessibilityElement] = []

    override public var isFlipped: Bool { true }

    public init() {
        super.init(frame: .zero)
        setAccessibilityElement(true)
        setAccessibilityRole(.tabGroup)
    }

    public required init?(coder: NSCoder) { fatalError("not used") }

    /// Shows these tabs with one selected; redraws only when something changed.
    public func show(_ tabs: [Tab], selected: Int, colors: Colors) {
        guard tabs != self.tabs || selected != self.selected || colors != self.colors else { return }
        let relaid = tabs != self.tabs
        self.tabs = tabs
        self.selected = selected
        self.colors = colors
        if relaid {
            frames = Self.frames(tabs)
            invalidateIntrinsicContentSize()
        }
        updateAccessibility()
        needsDisplay = true
    }

    /// Tab rectangles laid out left to right.
    static func frames(_ tabs: [Tab]) -> [CGRect] {
        var x: CGFloat = 0
        return tabs.map { tab in
            let w = width(tab)
            defer { x += w + spacing }
            return CGRect(x: x, y: 0, width: w, height: height)
        }
    }

    static func width(_ tab: Tab) -> CGFloat {
        let title = (tab.title as NSString).size(withAttributes: [.font: font]).width
        let count = tab.count.flatMap { $0 > 0 ? $0 : nil }
            .map { countGap + ("\($0)" as NSString).size(withAttributes: [.font: font]).width } ?? 0
        return ceil(2 * padding + title + count)
    }

    override public var intrinsicContentSize: NSSize {
        NSSize(width: (frames.last?.maxX ?? 0), height: Self.height)
    }

    override public func draw(_ dirtyRect: NSRect) {
        for (i, (tab, r)) in zip(tabs, frames).enumerated() {
            let on = i == selected
            if on {
                colors.highlight.setFill()
                NSBezierPath(roundedRect: r, xRadius: 7, yRadius: 7).fill()
            }
            let title = NSAttributedString(string: tab.title, attributes: [
                .font: Self.font, .foregroundColor: on ? colors.selected : colors.text,
            ])
            let s = title.size()
            let y = r.midY - s.height / 2
            title.draw(at: CGPoint(x: r.minX + Self.padding, y: y))
            if let n = tab.count, n > 0 {
                NSAttributedString(string: "\(n)", attributes: [.font: Self.font, .foregroundColor: colors.count])
                    .draw(at: CGPoint(x: r.minX + Self.padding + s.width + Self.countGap, y: y))
            }
        }
    }

    override public func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if let i = frames.firstIndex(where: { $0.contains(p) }) { press(i) }
    }

    /// Selects a tab as a click would.
    public func press(_ i: Int) {
        guard tabs.indices.contains(i) else { return }
        onSelect(i)
    }

    private func updateAccessibility() {
        while elements.count < tabs.count {
            let i = elements.count
            let e = TabElement()
            e.strip = self
            e.index = i
            e.setAccessibilityRole(.radioButton)
            e.setAccessibilityParent(self)
            elements.append(e)
        }
        elements.removeLast(elements.count - tabs.count)
        for (i, e) in elements.enumerated() {
            let tab = tabs[i]
            e.setAccessibilityLabel(tab.title + (tab.count.flatMap { $0 > 0 ? ", \($0)" : nil } ?? ""))
            e.setAccessibilityValue(i == selected ? 1 : 0)
            e.setAccessibilityFrameInParentSpace(frames[i])
        }
        setAccessibilityChildren(elements)
    }
}

/// One tab for accessibility: pressing it selects the tab.
final class TabElement: NSAccessibilityElement {
    weak var strip: TabStrip?
    var index = 0

    override func accessibilityPerformPress() -> Bool {
        let (s, i) = (strip, index)
        MainActor.assumeIsolated { s?.press(i) }
        return true
    }
}
