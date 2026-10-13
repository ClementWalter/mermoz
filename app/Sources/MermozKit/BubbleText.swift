/// Chat bubble text drawn by TextKit: attributed strings built once per message and
/// whole layouts (text storage, glyphs, line breaks) made once per message and width.
/// A bubble view draws straight from its cached layout, so stepping through
/// conversations neither measures nor typesets text again.
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
    public var semibold: Bool
    /// Web addresses become clickable links (bubbles), or stay plain text (titles).
    public var links: Bool
    /// sRGB components of the text color.
    public var red: CGFloat, green: CGFloat, blue: CGFloat, alpha: CGFloat

    public init(pointSize: CGFloat = 13.5, lineSpacing: CGFloat = 2.5, semibold: Bool = false, links: Bool = true,
                red: CGFloat = 0.05, green: CGFloat = 0.14, blue: CGFloat = 0.21, alpha: CGFloat = 0.92) {
        self.pointSize = pointSize; self.lineSpacing = lineSpacing; self.semibold = semibold; self.links = links
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
    private var layouts: [SizeKey: BubbleTextLayout.Laid] = [:]
    /// The width and style bubbles were last laid out at, to measure ahead at.
    private var last: (width: CGFloat, style: BubbleTextStyle)?
    /// Entries kept before the caches start over; a few conversations' worth many times.
    private let limit: Int
    public private(set) var sizeHits = 0
    public private(set) var sizeMisses = 0

    public init(limit: Int = 1500) { self.limit = limit }

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
        layout(id: id, text: text, style: style, width: width, maxLines: maxLines).size
    }

    /// The text laid out at `width`, ready to draw: made once, then reused.
    public func layout(id: String, text: String, style: BubbleTextStyle, width: CGFloat, maxLines: Int) -> BubbleTextLayout.Laid {
        let width = min(max(width, Self.minWidth), Self.idealWidth)
        let key = SizeKey(text: TextKey(id: id, text: text, style: style), width: Int(width.rounded(.down)), maxLines: maxLines)
        lock.lock()
        // Probes for the ideal (unbounded) or minimum size are not the bubble's width.
        if width > Self.minWidth, width < Self.idealWidth { last = (width, style) }
        if let s = layouts[key] { sizeHits += 1; lock.unlock(); return s }
        sizeMisses += 1
        lock.unlock()
        let s = Self.lay(attributed(id: id, text: text, style: style), width: CGFloat(key.width), maxLines: maxLines)
        lock.lock()
        if layouts.count >= limit { layouts.removeAll(keepingCapacity: true) }
        layouts[key] = s
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
        // One line break kind: a CRLF letter lays out and draws like any other.
        let text = text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        let p = NSMutableParagraphStyle()
        p.lineSpacing = style.lineSpacing
        p.lineBreakMode = .byWordWrapping
        let s = NSMutableAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: style.pointSize, weight: style.semibold ? .semibold : .regular),
            .foregroundColor: style.color,
            .paragraphStyle: p,
        ])
        guard style.links else { return s }
        let whole = NSRange(location: 0, length: (text as NSString).length)
        links?.enumerateMatches(in: text, range: whole) { match, _, _ in
            if let m = match, let url = m.url { s.addAttribute(.link, value: url, range: m.range) }
        }
        return s
    }

    /// TextKit 1 layout configured with `BubbleTextLayout.configure`; the text view that
    /// draws it takes this very container, so what is measured is what is drawn.
    static func lay(_ s: NSAttributedString, width: CGFloat, maxLines: Int) -> BubbleTextLayout.Laid {
        let storage = NSTextStorage(attributedString: s)
        let manager = NSLayoutManager()
        let container = NSTextContainer(size: CGSize(width: max(width, 1), height: .greatestFiniteMagnitude))
        BubbleTextLayout.configure(container, maxLines: maxLines)
        manager.addTextContainer(container)
        storage.addLayoutManager(manager)
        manager.ensureLayout(for: container)
        let r = manager.usedRect(for: container)
        return BubbleTextLayout.Laid(storage: storage, manager: manager, container: container,
                                     size: CGSize(width: ceil(r.width), height: ceil(r.height)))
    }
}

/// The text container settings shared by measuring and drawing.
public enum BubbleTextLayout {
    /// Shows a cached layout's text in a text view. The view keeps its own text stack and
    /// takes a copy of the text and the container settings (width, line limit): a view
    /// drawing a container it does not own stays blank on screen. Returns whether the text
    /// changed.
    @MainActor
    @discardableResult
    public static func fill(_ view: NSTextView, from laid: Laid) -> Bool {
        guard let storage = view.textStorage, let container = view.textContainer else { return false }
        container.lineFragmentPadding = 0
        container.maximumNumberOfLines = laid.container.maximumNumberOfLines
        container.lineBreakMode = laid.container.lineBreakMode
        container.widthTracksTextView = false
        container.heightTracksTextView = false
        container.size = CGSize(width: laid.container.size.width, height: .greatestFiniteMagnitude)
        guard !storage.isEqual(to: laid.storage) else { return false }
        storage.setAttributedString(laid.storage)
        view.needsDisplay = true
        return true
    }

    /// A finished TextKit stack: the storage, its laid-out glyphs and the container, plus
    /// the size the text takes. Measuring uses it; a text view copies its text and settings.
    public final class Laid: @unchecked Sendable {
        public let storage: NSTextStorage
        public let manager: NSLayoutManager
        public let container: NSTextContainer
        public let size: CGSize

        init(storage: NSTextStorage, manager: NSLayoutManager, container: NSTextContainer, size: CGSize) {
            self.storage = storage; self.manager = manager; self.container = container; self.size = size
        }
    }

    public static func configure(_ c: NSTextContainer, maxLines: Int) {
        c.lineFragmentPadding = 0
        c.maximumNumberOfLines = maxLines
        c.lineBreakMode = maxLines > 0 ? .byTruncatingTail : .byWordWrapping
        c.widthTracksTextView = false
        c.heightTracksTextView = false
    }
}
