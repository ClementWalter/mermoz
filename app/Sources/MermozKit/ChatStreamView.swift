/// The reading pane's chat drawn by AppKit: one scroll view whose rows are laid out by
/// hand from the cached TextKit layouts (BubbleTextCache), recycled from conversation to
/// conversation. Bubbles, avatars, names and times are drawn, not built from SwiftUI
/// views; the rare rows that need SwiftUI (the whole email opened in place, a thread
/// marker, the summary, a bubble with pasted images) are hosted.
import AppKit
import SwiftUI

// MARK: model

/// A compact bubble as the stream draws it.
public struct BubbleSpec: Equatable, Sendable {
    public var id: String
    public var content: BubbleContent
    public var mine: Bool
    /// First of a run: shows the avatar, the name and the time.
    public var firstOfRun: Bool
    public var sender: String
    public var time: String
    public var focused: Bool
    /// Files sent along, counted under the text (📎 n).
    public var files: Int
    /// A line above the bubble, such as how it differs from the previous message.
    public var note: String?
    /// Pasted images (attachment ids), shown as thumbnails under the text.
    public var thumbnails: [String]

    public init(id: String, content: BubbleContent, mine: Bool, firstOfRun: Bool, sender: String, time: String,
                focused: Bool, files: Int = 0, note: String? = nil, thumbnails: [String] = []) {
        self.id = id; self.content = content; self.mine = mine; self.firstOfRun = firstOfRun
        self.sender = sender; self.time = time; self.focused = focused; self.files = files; self.note = note
        self.thumbnails = thumbnails
    }
}

/// Where another Gmail thread starts in the chat.
public struct MarkerSpec: Equatable, Sendable {
    public var threadId: String
    /// "date · subject".
    public var line: String
    /// The thread was added to the conversation, so it can be taken out.
    public var joined: Bool

    public init(threadId: String, line: String, joined: Bool) {
        self.threadId = threadId; self.line = line; self.joined = joined
    }
}

public enum ChatRowSpec: Equatable, Sendable {
    /// "Show N earlier messages".
    case earlier(Int)
    case bubble(BubbleSpec)
    case marker(MarkerSpec)
    /// A row the app builds in SwiftUI; `key` names it and changes when its content does.
    case hosted(key: String)
}

/// Colors the stream draws with, resolved by the app from its theme.
public struct ChatColors: Sendable {
    public var text: NSColor
    public var secondary: NSColor
    public var accent: NSColor
    public var mineBackground: NSColor
    public var theirsBackground: NSColor
    public var border: NSColor
    public var avatars: [NSColor]

    public init(text: NSColor, secondary: NSColor, accent: NSColor, mineBackground: NSColor, theirsBackground: NSColor,
                border: NSColor, avatars: [NSColor]) {
        self.text = text; self.secondary = secondary; self.accent = accent; self.mineBackground = mineBackground
        self.theirsBackground = theirsBackground; self.border = border; self.avatars = avatars
    }

    public static let plain = ChatColors(text: .labelColor, secondary: .secondaryLabelColor, accent: .controlAccentColor,
                                         mineBackground: .controlAccentColor.withAlphaComponent(0.15), theirsBackground: .white,
                                         border: .separatorColor, avatars: [.systemBlue, .systemTeal, .systemPurple])
}

/// What the stream asks the app to do.
@MainActor
public struct ChatActions {
    public var toggle: (String) -> Void = { _ in }
    public var showMore: (String) -> Void = { _ in }
    public var showEarlier: () -> Void = {}
    /// ↑ (-1) or ↓ (+1).
    public var moveFocus: (Int) -> Void = { _ in }
    /// Enter on the message in focus.
    public var activate: () -> Void = {}
    /// The SwiftUI view for a hosted row.
    public var hosted: (String) -> AnyView = { _ in AnyView(EmptyView()) }
    /// "Not the same conversation" on a marker: takes that thread out.
    public var split: (String) -> Void = { _ in }
    /// The file of an attachment, for its thumbnail.
    public var file: (String) async -> URL? = { _ in nil }

    public init() {}
}

/// Sizes of the drawn chat, matching the SwiftUI layout it replaces.
public enum ChatMetrics {
    public static let sidePadding: CGFloat = 22
    public static let verticalPadding: CGFloat = 16
    public static let rowSpacing: CGFloat = 4
    public static let avatar: CGFloat = 26
    public static let avatarGap: CGFloat = 8
    /// Room kept on the far side of a bubble, so a chat reads as two sides.
    public static let farSide: CGFloat = 72
    public static let inset = CGSize(width: 12, height: 8)
    public static let radius: CGFloat = 15
    public static let runGap: CGFloat = 6
    public static let header: CGFloat = 16
    public static let headerGap: CGFloat = 3
    public static let showMore: CGFloat = 22
    public static let earlier: CGFloat = 30
    public static let note: CGFloat = 19
    public static let files: CGFloat = 20
    public static let thumb = CGSize(width: 96, height: 120)
    public static let thumbGap: CGFloat = 6
    public static let markerLine: CGFloat = 20
    public static let markerJoined: CGFloat = 26

    /// Widest the text of a bubble can be in a chat `width` wide.
    public static func textWidth(_ width: CGFloat, mine: Bool) -> CGFloat {
        let side = mine ? farSide + avatarGap : avatar + avatarGap + farSide + avatarGap
        return max(BubbleTextCache.minWidth, width - 2 * sidePadding - side - 2 * inset.width)
    }

    /// Space above the bubble itself: the run's name and time, or a note.
    public static func top(_ spec: BubbleSpec) -> CGFloat {
        (spec.firstOfRun ? runGap + header + headerGap : 0) + (spec.note == nil ? 0 : note)
    }

    /// The bubble's own height with its text taking `text`.
    public static func bubble(text: CGSize, _ spec: BubbleSpec) -> CGFloat {
        text.height + 2 * inset.height + (spec.content.clamped ? showMore : 0) + (spec.files > 0 ? files : 0)
            + (spec.thumbnails.isEmpty ? 0 : thumbGap + thumb.height)
    }

    /// Height of a thread marker row.
    public static func marker(_ m: MarkerSpec) -> CGFloat { 12 + markerLine + (m.joined ? markerJoined + 5 : 0) }
}

// MARK: text

/// A bubble's selectable text: TextKit 1, drawing a container the cache already laid out.
/// A plain click (no selection, not on a link) is the bubble's click.
open class SelectableBubbleText: NSTextView {
    public var onClick: (() -> Void)?
    /// The cached layout on screen.
    public private(set) var drawn: BubbleTextLayout.Laid?
    /// Who had the keyboard before a click, given it back unless text was selected.
    private weak var previous: NSResponder?

    public static func make() -> Self {
        let v = Self(usingTextLayoutManager: false)
        v.isEditable = false
        v.isSelectable = true
        v.drawsBackground = false
        v.textContainerInset = .zero
        v.isVerticallyResizable = false
        v.isHorizontallyResizable = false
        v.linkTextAttributes = [.underlineStyle: NSUnderlineStyle.single.rawValue, .cursor: NSCursor.pointingHand]
        return v
    }

    /// Shows a cached layout: no measuring or typesetting here.
    public func show(_ laid: BubbleTextLayout.Laid) {
        guard laid !== drawn else { return }
        drawn = BubbleTextLayout.attach(laid, to: self, replacing: drawn)
        setAccessibilityValue(laid.storage.string)
    }

    override open func mouseDown(with event: NSEvent) {
        previous = window?.firstResponder
        let point = convert(event.locationInWindow, from: nil)
        let i = characterIndexForInsertion(at: point)
        let onLink = i < (textStorage?.length ?? 0) && textStorage?.attribute(.link, at: i, effectiveRange: nil) != nil
        // Tracks the drag (selection) or the link click until the mouse is released.
        super.mouseDown(with: event)
        guard !onLink, selectedRange().length == 0 else { return }
        restoreKeyboard()
        onClick?()
    }

    /// ↑/↓, Enter and Esc keep stepping through messages even after text was selected.
    override open func keyDown(with event: NSEvent) {
        let keys: Set<UInt16> = [125, 126, 36, 76, 53]
        if keys.contains(event.keyCode), event.modifierFlags.intersection([.command, .shift, .option, .control]).isEmpty {
            setSelectedRange(NSRange(location: 0, length: 0))
            restoreKeyboard()
            window?.sendEvent(event)
            return
        }
        super.keyDown(with: event)
    }

    private func restoreKeyboard() {
        guard window?.firstResponder === self else { return }
        window?.makeFirstResponder(previous === self ? nil : previous)
    }
}

// MARK: rows

/// One compact bubble drawn by hand: avatar, name and time, rounded background and
/// border, the text view, and for a clamped message its fade and "Show more".
final class BubbleRowView: NSView {
    let text = SelectableBubbleText.make()
    private let fade = FadeView()
    private let more = NSButton(title: "Show more", target: nil, action: nil)
    private(set) var spec: BubbleSpec?
    private var colors = ChatColors.plain
    var onShowMore: (() -> Void)?
    /// The bubble's own rectangle, in this view.
    private(set) var bubbleRect: CGRect = .zero

    override var isFlipped: Bool { true }

    init() {
        super.init(frame: .zero)
        addSubview(text)
        addSubview(fade)
        more.isBordered = false
        more.target = self
        more.action = #selector(showMore)
        more.setAccessibilityLabel("Show more")
        addSubview(more)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        toolTip = "Open the whole email"
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    @objc private func showMore() { onShowMore?() }

    func configure(_ spec: BubbleSpec, laid: BubbleTextLayout.Laid, width: CGFloat, colors: ChatColors, onClick: @escaping () -> Void) {
        self.spec = spec
        self.colors = colors
        text.onClick = onClick
        text.show(laid)
        let top = ChatMetrics.top(spec)
        // Wide enough for "Show more" and the file count under short text.
        let thumbs = CGFloat(spec.thumbnails.count) * (ChatMetrics.thumb.width + ChatMetrics.thumbGap) - ChatMetrics.thumbGap
        let floor: CGFloat = max(spec.content.clamped || spec.files > 0 ? 80 : 0, spec.thumbnails.isEmpty ? 0 : thumbs)
        let bubbleSize = CGSize(width: max(laid.size.width, floor) + 2 * ChatMetrics.inset.width,
                                height: ChatMetrics.bubble(text: laid.size, spec))
        let x = spec.mine
            ? width - ChatMetrics.sidePadding - bubbleSize.width
            : ChatMetrics.sidePadding + ChatMetrics.avatar + ChatMetrics.avatarGap
        bubbleRect = CGRect(x: x, y: top, width: bubbleSize.width, height: bubbleSize.height)
        text.frame = CGRect(x: x + ChatMetrics.inset.width, y: top + ChatMetrics.inset.height,
                            width: laid.size.width, height: laid.size.height)
        fade.isHidden = !spec.content.clamped
        more.isHidden = !spec.content.clamped
        if spec.content.clamped {
            fade.color = spec.mine ? colors.mineBackground : colors.theirsBackground
            fade.frame = CGRect(x: text.frame.minX, y: text.frame.maxY - 22, width: text.frame.width, height: 22)
            more.attributedTitle = NSAttributedString(string: "Show more", attributes: [
                .font: NSFont.systemFont(ofSize: 12), .foregroundColor: colors.accent,
            ])
            more.sizeToFit()
            more.frame.origin = CGPoint(x: text.frame.minX, y: text.frame.maxY + 3)
        }
        setAccessibilityLabel("Message from \(spec.mine ? "you" : spec.sender)"
            + (spec.files > 0 ? ", \(spec.files) file\(spec.files == 1 ? "" : "s") attached" : "")
            + (spec.note.map { ". \($0)" } ?? ""))
        setAccessibilityHelp("Opens the whole email")
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let spec else { return }
        if spec.firstOfRun {
            let nameFont = NSFont.systemFont(ofSize: 12, weight: .semibold)
            let name = NSAttributedString(string: spec.mine ? "You" : spec.sender,
                                          attributes: [.font: nameFont, .foregroundColor: colors.text])
            let time = NSAttributedString(string: spec.time, attributes: [
                .font: NSFont.systemFont(ofSize: 11), .foregroundColor: colors.secondary,
            ])
            let y = ChatMetrics.runGap
            if spec.mine {
                let w = name.size().width + 6 + time.size().width
                let x = bubbleRect.maxX - w
                name.draw(at: CGPoint(x: x, y: y))
                time.draw(at: CGPoint(x: x + name.size().width + 6, y: y + 1))
            } else {
                name.draw(at: CGPoint(x: bubbleRect.minX, y: y))
                time.draw(at: CGPoint(x: bubbleRect.minX + name.size().width + 6, y: y + 1))
                drawAvatar(spec.sender, in: CGRect(x: ChatMetrics.sidePadding, y: y, width: ChatMetrics.avatar, height: ChatMetrics.avatar))
            }
        }
        if let note = spec.note {
            let line = NSAttributedString(string: note, attributes: [
                .font: NSFont.systemFont(ofSize: 11), .foregroundColor: colors.secondary,
            ])
            let x = spec.mine ? bubbleRect.maxX - line.size().width : bubbleRect.minX
            line.draw(at: CGPoint(x: x, y: bubbleRect.minY - ChatMetrics.note + 2))
        }
        defer {
            if spec.files > 0 { drawFiles(spec.files) }
            drawThumbnails(spec.thumbnails)
        }
        let shape = NSBezierPath(roundedRect: bubbleRect.insetBy(dx: 0.5, dy: 0.5), xRadius: ChatMetrics.radius, yRadius: ChatMetrics.radius)
        (spec.mine ? colors.mineBackground : colors.theirsBackground).setFill()
        shape.fill()
        (spec.focused ? colors.accent.withAlphaComponent(0.6) : colors.border).setStroke()
        shape.lineWidth = spec.focused ? 1.5 : 1
        shape.stroke()
    }

    var file: (String) async -> URL? = { _ in nil }

    /// Pasted images as thumbnails under the text, decoded small and off the main thread.
    private func drawThumbnails(_ ids: [String]) {
        guard !ids.isEmpty else { return }
        let y = text.frame.maxY + (spec?.content.clamped == true ? ChatMetrics.showMore : 0) + ChatMetrics.thumbGap
        for (i, id) in ids.enumerated() {
            let r = CGRect(x: text.frame.minX + CGFloat(i) * (ChatMetrics.thumb.width + ChatMetrics.thumbGap), y: y,
                           width: ChatMetrics.thumb.width, height: ChatMetrics.thumb.height)
            let clip = NSBezierPath(roundedRect: r, xRadius: 10, yRadius: 10)
            colors.mineBackground.withAlphaComponent(0.6).setFill()
            clip.fill()
            if let image = Thumbnails.shared.image(id) {
                NSGraphicsContext.saveGraphicsState()
                clip.addClip()
                // Fill the tile, cropping the longer side.
                let scale = max(r.width / CGFloat(image.width), r.height / CGFloat(image.height))
                let size = CGSize(width: CGFloat(image.width) * scale, height: CGFloat(image.height) * scale)
                NSImage(cgImage: image, size: size)
                    .draw(in: CGRect(x: r.midX - size.width / 2, y: r.midY - size.height / 2, width: size.width, height: size.height),
                          from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
                NSGraphicsContext.restoreGraphicsState()
            } else {
                Thumbnails.shared.load(id, file: file) { [weak self] in self?.needsDisplay = true }
            }
        }
    }

    /// "📎 n" at the bottom of the bubble: the files are in the whole email.
    private func drawFiles(_ n: Int) {
        let y = bubbleRect.maxY - ChatMetrics.inset.height - 14
        let x = bubbleRect.minX + ChatMetrics.inset.width
        if let clip = NSImage(systemSymbolName: "paperclip", accessibilityDescription: nil) {
            colors.secondary.set()
            clip.withSymbolConfiguration(.init(pointSize: 11, weight: .regular))?
                .draw(in: CGRect(x: x, y: y, width: 12, height: 13), from: .zero, operation: .sourceOver, fraction: 0.7,
                      respectFlipped: true, hints: nil)
        }
        NSAttributedString(string: "\(n)", attributes: [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: colors.secondary])
            .draw(at: CGPoint(x: x + 16, y: y))
    }

    private func drawAvatar(_ name: String, in rect: CGRect) {
        // A sum of code points, not hashValue: the color stays the same across launches.
        let hash = name.unicodeScalars.reduce(0) { $0 &+ Int($1.value) }
        colors.avatars[hash % max(colors.avatars.count, 1)].setFill()
        NSBezierPath(ovalIn: rect).fill()
        let initials = name.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined().uppercased()
        let label = NSAttributedString(string: initials.isEmpty ? "?" : initials, attributes: [
            .font: NSFont.systemFont(ofSize: 11 * rect.width / 28, weight: .bold), .foregroundColor: NSColor.white,
        ])
        let s = label.size()
        label.draw(at: CGPoint(x: rect.midX - s.width / 2, y: rect.midY - s.height / 2))
    }

    /// Clicks on the bubble outside its text open the whole email too.
    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if bubbleRect.contains(p) { text.onClick?() } else { super.mouseDown(with: event) }
    }
}

/// The bottom of a clamped bubble fading into its background.
final class FadeView: NSView {
    var color: NSColor = .white { didSet { needsDisplay = true } }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        NSGradient(starting: color.withAlphaComponent(0), ending: color)?.draw(in: bounds, angle: 90)
    }
}

/// A SwiftUI row inside the stream; its size changes (an email's HTML loading) make the
/// stream lay out again.
final class HostedRowView: NSHostingView<AnyView> {
    var key = ""

    override func invalidateIntrinsicContentSize() {
        super.invalidateIntrinsicContentSize()
        superview?.needsLayout = true
    }

    /// The row's height when it is `width` wide.
    func height(at width: CGFloat) -> CGFloat {
        let c = widthAnchor.constraint(equalToConstant: width)
        c.isActive = true
        defer { c.isActive = false }
        return ceil(fittingSize.height)
    }
}

// MARK: stream

/// The document view: rows stacked top to bottom, laid out by hand.
final class ChatDocumentView: NSView {
    override var isFlipped: Bool { true }
    weak var stream: ChatStreamView?

    override var acceptsFirstResponder: Bool { true }

    override func layout() {
        super.layout()
        stream?.layoutRows()
    }

    override func mouseDown(with event: NSEvent) {
        // A click in the chat gives it the keyboard: ↑/↓ and Enter move between messages.
        window?.makeFirstResponder(self)
    }

    override func keyDown(with event: NSEvent) {
        guard event.modifierFlags.intersection([.command, .shift, .option, .control]).isEmpty,
              let stream else { return super.keyDown(with: event) }
        switch event.keyCode {
        case 125: stream.actions.moveFocus(1)
        case 126: stream.actions.moveFocus(-1)
        case 36, 76: stream.actions.activate()
        default: super.keyDown(with: event)
        }
    }
}

public final class ChatStreamView: NSScrollView {
    public var actions = ChatActions()
    public var colors = ChatColors.plain
    public var style = BubbleTextStyle()
    private let document = ChatDocumentView()
    private(set) var rows: [ChatRowSpec] = []
    /// Row views by position; reused for the next conversation when their kind matches.
    private(set) var views: [NSView] = []
    private var conversation = ""
    /// Scroll to the bottom once the next layout is done (new conversation or new mail).
    private var pinBottom = false
    private var focusedRow: Int?
    /// The focused row to bring into view after the next layout.
    private var pendingReveal: Int?
    /// The width rows were last laid out at.
    private var laidWidth: CGFloat = 0

    public init() {
        super.init(frame: .zero)
        document.stream = self
        documentView = document
        drawsBackground = false
        hasVerticalScroller = true
        autohidesScrollers = true
        contentView.postsBoundsChangedNotifications = false
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// The chat for `conversation`; `messages` changes when mail arrives in it.
    public func show(_ new: [ChatRowSpec], conversation: String, messages: Int) {
        let key = "\(conversation)/\(messages)"
        if key != self.conversation { pinBottom = true }
        self.conversation = key
        let oldFocus = focusedRow
        rows = new
        focusedRow = new.firstIndex { if case let .bubble(b) = $0 { b.focused } else { false } }
        rebuildViews()
        if let f = focusedRow, f != oldFocus, !pinBottom { pendingReveal = f }
        // One layout pass, when AppKit lays the window out, whatever asked for it.
        document.needsLayout = true
    }

    /// Rows whose kind changed get a new view; the others are reconfigured in place.
    private func rebuildViews() {
        var next: [NSView] = []
        // Bubble views not kept at their place are handed to the rows that need one.
        var spare = views.enumerated().compactMap { i, v -> BubbleRowView? in
            guard let b = v as? BubbleRowView else { return nil }
            if i < rows.count, case .bubble = rows[i] { return nil }
            return b
        }
        for (i, row) in rows.enumerated() {
            let old = i < views.count ? views[i] : nil
            switch row {
            case .earlier:
                next.append((old as? NSButton) ?? makeEarlierButton())
            case .bubble:
                next.append((old as? BubbleRowView) ?? (spare.isEmpty ? BubbleRowView() : spare.removeFirst()))
            case .marker:
                next.append((old as? MarkerRowView) ?? MarkerRowView())
            case let .hosted(key):
                if let h = old as? HostedRowView, h.key == key {
                    next.append(h)
                } else if let h = old as? HostedRowView {
                    h.rootView = actions.hosted(key)
                    h.key = key
                    next.append(h)
                } else {
                    let h = HostedRowView(rootView: actions.hosted(key))
                    h.sizingOptions = [.intrinsicContentSize]
                    h.key = key
                    next.append(h)
                }
            }
        }
        for v in views where !next.contains(where: { $0 === v }) { v.removeFromSuperview() }
        for v in next where v.superview !== document { document.addSubview(v) }
        views = next
    }

    private func makeEarlierButton() -> NSButton {
        let b = NSButton(title: "", target: self, action: #selector(showEarlier))
        b.isBordered = false
        return b
    }

    @objc private func showEarlier() { actions.showEarlier() }

    /// Lays every row out at the current width from cached sizes.
    func layoutRows() {
        let width = max(contentView.bounds.width, 1)
        laidWidth = width
        // A chat read at its bottom stays there when rows or the pane change size.
        let visible = contentView.bounds
        let wasAtBottom = visible.height > 0 && visible.maxY >= document.frame.height - 2
        var y = ChatMetrics.verticalPadding
        for (i, row) in rows.enumerated() where i < views.count {
            let v = views[i]
            switch row {
            case let .earlier(n):
                let b = v as? NSButton
                b?.attributedTitle = NSAttributedString(string: "Show \(n) earlier message\(n == 1 ? "" : "s")", attributes: [
                    .font: NSFont.systemFont(ofSize: 12), .foregroundColor: colors.accent,
                ])
                v.frame = CGRect(x: 0, y: y, width: width, height: ChatMetrics.earlier)
                y += ChatMetrics.earlier
            case let .bubble(spec):
                let laid = BubbleTextCache.shared.layout(id: spec.id, text: spec.content.text, style: style,
                                                         width: ChatMetrics.textWidth(width, mine: spec.mine),
                                                         maxLines: spec.content.maxLines)
                let h = ChatMetrics.top(spec) + ChatMetrics.bubble(text: laid.size, spec)
                v.frame = CGRect(x: 0, y: y, width: width, height: h)
                (v as? BubbleRowView)?.configure(spec, laid: laid, width: width, colors: colors) { [weak self] in
                    self?.actions.toggle(spec.id)
                }
                (v as? BubbleRowView)?.onShowMore = { [weak self] in self?.actions.showMore(spec.id) }
                (v as? BubbleRowView)?.file = actions.file
                y += h
            case let .marker(m):
                let h = ChatMetrics.marker(m)
                v.frame = CGRect(x: 0, y: y, width: width, height: h)
                (v as? MarkerRowView)?.configure(m, colors: colors) { [weak self] in self?.actions.split(m.threadId) }
            case .hosted:
                let inner = width - 2 * ChatMetrics.sidePadding
                let h = (v as? HostedRowView)?.height(at: inner) ?? 0
                v.frame = CGRect(x: ChatMetrics.sidePadding, y: y, width: inner, height: h)
                y += h
            }
            y += ChatMetrics.rowSpacing
        }
        y += ChatMetrics.verticalPadding
        let height = max(y, contentView.bounds.height)
        if document.frame.size != CGSize(width: width, height: height) {
            document.frame = CGRect(x: 0, y: 0, width: width, height: height)
        }
        if (pinBottom || wasAtBottom), contentView.bounds.height > 0 {
            pinBottom = false
            contentView.scroll(to: CGPoint(x: 0, y: max(0, height - contentView.bounds.height)))
            reflectScrolledClipView(contentView)
        } else if let r = pendingReveal {
            pendingReveal = nil
            reveal(row: r)
        }
    }

    /// Scrolls so the row sits in the middle, as ↑/↓ do.
    private func reveal(row: Int) {
        guard row < views.count else { return }
        let f = views[row].frame
        let target = max(0, min(f.midY - contentView.bounds.height / 2, document.frame.height - contentView.bounds.height))
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.12
            contentView.animator().setBoundsOrigin(CGPoint(x: 0, y: target))
        }
        reflectScrolledClipView(contentView)
    }

    override public func layout() {
        super.layout()
        // The pane was resized: the rows follow, in the document's own layout pass.
        if contentView.bounds.width != laidWidth { document.needsLayout = true }
    }

    // MARK: for tests

    /// The bubble row drawn for a message, if shown.
    public func bubbleFrame(_ id: String) -> CGRect? {
        for (row, v) in zip(rows, views) {
            if case let .bubble(b) = row, b.id == id, let r = v as? BubbleRowView {
                return r.bubbleRect.offsetBy(dx: r.frame.minX, dy: r.frame.minY)
            }
        }
        return nil
    }

    /// The text view drawing a message's bubble.
    public func bubbleText(_ id: String) -> SelectableBubbleText? {
        for (row, v) in zip(rows, views) {
            if case let .bubble(b) = row, b.id == id { return (v as? BubbleRowView)?.text }
        }
        return nil
    }

    /// Presses "Show more" on a message, as a click would.
    public func pressShowMore(_ id: String) {
        for (row, v) in zip(rows, views) {
            if case let .bubble(b) = row, b.id == id { (v as? BubbleRowView)?.onShowMore?() }
        }
    }

    /// Presses "Show N earlier messages", as a click would.
    public func pressEarlier() { views.compactMap { $0 as? NSButton }.first?.performClick(nil) }

    /// Presses "Not the same conversation" on the first joined thread's marker.
    public func pressSplit() {
        for v in views { if let m = v as? MarkerRowView, m.pressSplit() { return } }
    }

    /// The title on the "Show N earlier messages" row.
    public var earlierTitle: String? { views.compactMap { $0 as? NSButton }.first?.attributedTitle.string }

    /// Sends a key to the chat as if it had the keyboard.
    public func press(keyCode: UInt16) {
        guard let e = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                                       windowNumber: window?.windowNumber ?? 0, context: nil, characters: "",
                                       charactersIgnoringModifiers: "", isARepeat: false, keyCode: keyCode)
        else { return }
        document.keyDown(with: e)
    }

    /// The row views on screen, in order.
    public var rowViews: [NSView] { views }
}

/// Where another Gmail thread starts: "date · subject" in a thin capsule, and for a thread
/// joined to the conversation, "Added from another email · Not the same conversation".
final class MarkerRowView: NSView {
    private var spec: MarkerSpec?
    private var colors = ChatColors.plain
    private let split = NSButton(title: "Not the same conversation", target: nil, action: nil)
    private var onSplit: (() -> Void)?

    override var isFlipped: Bool { true }

    init() {
        super.init(frame: .zero)
        split.isBordered = false
        split.target = self
        split.action = #selector(splitThread)
        split.toolTip = "Show this email on its own again"
        addSubview(split)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    @objc private func splitThread() { onSplit?() }

    /// Presses the link if this marker has it.
    func pressSplit() -> Bool {
        guard !split.isHidden else { return false }
        split.performClick(nil)
        return true
    }

    func configure(_ spec: MarkerSpec, colors: ChatColors, onSplit: @escaping () -> Void) {
        self.spec = spec
        self.colors = colors
        self.onSplit = onSplit
        split.isHidden = !spec.joined
        split.attributedTitle = NSAttributedString(string: "Not the same conversation", attributes: [
            .font: NSFont.systemFont(ofSize: 11.5), .foregroundColor: colors.accent,
        ])
        split.sizeToFit()
        setAccessibilityElement(true)
        setAccessibilityRole(.staticText)
        setAccessibilityLabel((spec.joined ? "Added from another email. " : "") + spec.line)
        needsDisplay = true
    }

    override func layout() {
        super.layout()
        guard let spec, spec.joined else { return }
        let added = NSAttributedString(string: "Added from another email", attributes: [.font: NSFont.systemFont(ofSize: 11.5)])
        let total = added.size().width + 8 + split.frame.width
        split.frame.origin = CGPoint(x: bounds.midX - total / 2 + added.size().width + 8, y: 6 + 4)
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let spec else { return }
        var y: CGFloat = 6
        if spec.joined {
            let added = NSAttributedString(string: "Added from another email", attributes: [
                .font: NSFont.systemFont(ofSize: 11.5), .foregroundColor: colors.secondary,
            ])
            let total = added.size().width + 8 + split.frame.width
            let pill = CGRect(x: bounds.midX - total / 2 - 10, y: y, width: total + 20, height: ChatMetrics.markerJoined)
            colors.mineBackground.withAlphaComponent(0.7).setFill()
            NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
            added.draw(at: CGPoint(x: pill.minX + 10, y: y + 5))
            y += ChatMetrics.markerJoined + 5
        }
        let line = NSAttributedString(string: spec.line, attributes: [
            .font: NSFont.systemFont(ofSize: 11), .foregroundColor: colors.secondary,
        ])
        let w = min(line.size().width, bounds.width - 80)
        let capsule = CGRect(x: bounds.midX - w / 2 - 10, y: y, width: w + 20, height: ChatMetrics.markerLine)
        let path = NSBezierPath(roundedRect: capsule.insetBy(dx: 0.5, dy: 0.5), xRadius: capsule.height / 2, yRadius: capsule.height / 2)
        colors.border.setStroke()
        path.stroke()
        line.draw(with: CGRect(x: capsule.minX + 10, y: y + 3, width: w, height: 14),
                  options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }
}

/// Small decoded thumbnails of pasted images, by attachment id. Files are read and
/// decoded off the main thread, at the size a tile shows.
final class Thumbnails: @unchecked Sendable {
    static let shared = Thumbnails()
    private let lock = NSLock()
    private var images: [String: CGImage] = [:]
    private var loading: Set<String> = []

    func image(_ id: String) -> CGImage? {
        lock.lock()
        defer { lock.unlock() }
        return images[id]
    }

    @MainActor
    func load(_ id: String, file: @escaping (String) async -> URL?, done: @escaping @MainActor () -> Void) {
        lock.lock()
        guard !loading.contains(id), images[id] == nil else { lock.unlock(); return }
        loading.insert(id)
        lock.unlock()
        Task {
            guard let url = await file(id) else { return }
            let image = await Task.detached(priority: .utility) { Self.decode(url) }.value
            self.store(id, image)
            done()
        }
    }

    private func store(_ id: String, _ image: CGImage?) {
        lock.lock()
        defer { lock.unlock() }
        if images.count > 500 { images.removeAll() }
        images[id] = image
    }

    static func decode(_ url: URL) -> CGImage? {
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        return CGImageSourceCreateThumbnailAtIndex(src, 0, [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceThumbnailMaxPixelSize: 320,
            kCGImageSourceCreateThumbnailWithTransform: true,
        ] as CFDictionary)
    }
}
