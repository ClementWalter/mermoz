/// Chat bubble text drawn by TextKit: attributed strings built once per message and
/// sizes measured once per message and width, so stepping through conversations reuses
/// both instead of laying text out again.
///
/// Everything here runs on any thread (TextKit 1 objects not attached to a view), so a
/// conversation's bubbles are prepared off the main thread before it is shown.
import AppKit
import Foundation

/// How bubble text looks; part of every cache key, so a new font size or color scheme
/// builds and measures again.
public struct BubbleTextStyle: Hashable, Sendable {
    public var pointSize: CGFloat
    public var lineSpacing: CGFloat
    /// sRGB components of the text color.
    public var red: CGFloat, green: CGFloat, blue: CGFloat, alpha: CGFloat

    public init(pointSize: CGFloat = 13.5, lineSpacing: CGFloat = 2.5,
                red: CGFloat = 0.05, green: CGFloat = 0.14, blue: CGFloat = 0.21, alpha: CGFloat = 0.92) {
        self.pointSize = pointSize; self.lineSpacing = lineSpacing
        self.red = red; self.green = green; self.blue = blue; self.alpha = alpha
    }

    public var color: NSColor { NSColor(srgbRed: red, green: green, blue: blue, alpha: alpha) }
}

/// What a compact bubble shows of a message, and whether it stops at eight lines.
public struct BubbleContent: Hashable, Sendable {
    public var text: String
    public var clamped: Bool

    /// Lines a clamped bubble shows before "Show more".
    public static let clampLines = 8

    public static func compact(_ m: Message, showAll: Bool) -> BubbleContent {
        let text = BubbleParts.compact(m).text
        let clamped = !showAll && BubbleParts.clampsCompact(text)
        let shown = clamped ? BubbleParts.opening(text) : text
        return BubbleContent(text: shown.isEmpty ? "(empty message)" : shown, clamped: clamped)
    }

    public var maxLines: Int { clamped ? Self.clampLines : 0 }
}

public final class BubbleTextCache: @unchecked Sendable {
    public static let shared = BubbleTextCache()

    struct TextKey: Hashable {
        var id: String
        var text: String
        var style: BubbleTextStyle
    }

    struct SizeKey: Hashable {
        var text: TextKey
        /// Whole points: sub-point width changes do not move a line break.
        var width: Int
        var maxLines: Int
    }

    private let lock = NSLock()
    private var strings: [TextKey: NSAttributedString] = [:]
    private var sizes: [SizeKey: CGSize] = [:]
    /// The width and style bubbles were last laid out at, to measure ahead at.
    private var last: (width: CGFloat, style: BubbleTextStyle)?
    /// Entries kept before the caches start over; a few conversations' worth many times.
    private let limit: Int
    public private(set) var sizeHits = 0
    public private(set) var sizeMisses = 0

    public init(limit: Int = 4000) { self.limit = limit }

    private static let links = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    /// The bubble's attributed text: font, color, line spacing and clickable links.
    public func attributed(id: String, text: String, style: BubbleTextStyle) -> NSAttributedString {
        let key = TextKey(id: id, text: text, style: style)
        lock.lock()
        if let s = strings[key] { lock.unlock(); return s }
        lock.unlock()
        let s = Self.build(text, style: style)
        lock.lock()
        if strings.count >= limit { strings.removeAll(keepingCapacity: true) }
        strings[key] = s
        lock.unlock()
        return s
    }

    /// Size of the text laid out at `width` (at most `maxLines` lines; 0: no limit).
    public func size(id: String, text: String, style: BubbleTextStyle, width: CGFloat, maxLines: Int) -> CGSize {
        let width = min(max(width, Self.minWidth), Self.idealWidth)
        let key = SizeKey(text: TextKey(id: id, text: text, style: style), width: Int(width.rounded(.down)), maxLines: maxLines)
        lock.lock()
        // Probes for the ideal (unbounded) or minimum size are not the bubble's width.
        if width > Self.minWidth, width < Self.idealWidth { last = (width, style) }
        if let s = sizes[key] { sizeHits += 1; lock.unlock(); return s }
        sizeMisses += 1
        lock.unlock()
        let s = Self.measure(attributed(id: id, text: text, style: style), width: CGFloat(key.width), maxLines: maxLines)
        lock.lock()
        if sizes.count >= limit { sizes.removeAll(keepingCapacity: true) }
        sizes[key] = s
        lock.unlock()
        return s
    }

    /// Builds and measures these bubbles at the last width and style used, so opening
    /// their conversation finds both ready. Call off the main thread.
    public func prepare(_ items: [(id: String, content: BubbleContent)]) {
        lock.lock()
        let at = last
        lock.unlock()
        guard let at else { return }
        for item in items {
            for w in [at.width, Self.idealWidth] {
                _ = size(id: item.id, text: item.content.text, style: at.style, width: w, maxLines: item.content.maxLines)
            }
        }
    }

    /// The width asked when SwiftUI wants the text's natural, unwrapped size.
    public static let idealWidth: CGFloat = 10_000
    /// Narrower proposals are measured at this width: a 1-point-wide layout of a long
    /// email costs a line per character and is never shown.
    public static let minWidth: CGFloat = 40

    static func build(_ text: String, style: BubbleTextStyle) -> NSAttributedString {
        let p = NSMutableParagraphStyle()
        p.lineSpacing = style.lineSpacing
        p.lineBreakMode = .byWordWrapping
        let s = NSMutableAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: style.pointSize),
            .foregroundColor: style.color,
            .paragraphStyle: p,
        ])
        let whole = NSRange(location: 0, length: (text as NSString).length)
        links?.enumerateMatches(in: text, range: whole) { match, _, _ in
            if let m = match, let url = m.url { s.addAttribute(.link, value: url, range: m.range) }
        }
        return s
    }

    /// TextKit 1 layout configured exactly like `BubbleTextLayout.configure`, so the size
    /// measured here is the size the text view draws.
    static func measure(_ s: NSAttributedString, width: CGFloat, maxLines: Int) -> CGSize {
        let storage = NSTextStorage(attributedString: s)
        let manager = NSLayoutManager()
        let container = NSTextContainer(size: CGSize(width: max(width, 1), height: .greatestFiniteMagnitude))
        BubbleTextLayout.configure(container, maxLines: maxLines)
        manager.addTextContainer(container)
        storage.addLayoutManager(manager)
        manager.ensureLayout(for: container)
        let r = manager.usedRect(for: container)
        return CGSize(width: ceil(r.width), height: ceil(r.height))
    }
}

/// The text container settings shared by measuring and drawing.
public enum BubbleTextLayout {
    public static func configure(_ c: NSTextContainer, maxLines: Int) {
        c.lineFragmentPadding = 0
        c.maximumNumberOfLines = maxLines
        c.lineBreakMode = maxLines > 0 ? .byTruncatingTail : .byWordWrapping
        c.widthTracksTextView = false
        c.heightTracksTextView = false
    }
}
