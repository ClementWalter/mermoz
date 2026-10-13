/// The reading pane's chat drawn by AppKit: one scroll view whose rows are laid out by
/// hand from the cached TextKit layouts (BubbleTextCache), recycled from conversation to
/// conversation. Bubbles, avatars, names and times are drawn, not built from SwiftUI
/// views; the rare rows that need SwiftUI (the whole email opened in place, a thread
/// marker, the summary, a bubble with pasted images) are hosted.
import AppKit
import Quartz
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
    /// Who sent it, for the avatar color: the same for every variant of their name.
    public var senderKey: String
    public var time: String
    public var focused: Bool
    /// Files sent along, counted under the text (📎 n).
    public var files: Int
    /// A line above the bubble, such as how it differs from the previous message.
    public var note: String?
    /// Pasted images (attachment ids), shown as thumbnails under the text.
    public var thumbnails: [String]
    /// Photos sent along (image files), shown as a grid under the text.
    public var photos: [Attachment]
    /// A small line inside the bubble over the text, such as "Forwarded from Nora".
    public var caption: String?
    /// The message adds no words of its own: its text is a quote, drawn dimmed with a bar.
    public var quoted: Bool

    public init(id: String, content: BubbleContent, mine: Bool, firstOfRun: Bool, sender: String, senderKey: String? = nil,
                time: String, focused: Bool, files: Int = 0, note: String? = nil, thumbnails: [String] = [],
                photos: [Attachment] = [], caption: String? = nil, quoted: Bool = false) {
        self.id = id; self.content = content; self.mine = mine; self.firstOfRun = firstOfRun
        self.sender = sender; self.senderKey = senderKey ?? sender.lowercased(); self.time = time; self.focused = focused; self.files = files; self.note = note
        self.thumbnails = thumbnails
        self.photos = photos
        self.caption = caption
        self.quoted = quoted
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
    /// Shows photo files from the one at the index; nil: Quick Look.
    public var previewPhotos: (([URL], Int) -> Void)?

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
    /// Width of the photo grid, and the gap between its tiles.
    public static let photoGrid: CGFloat = 224
    public static let photoGap: CGFloat = 4
    /// The caption line inside a bubble, over its text.
    public static let caption: CGFloat = 18
    /// How strongly a quote-only bubble's text shows.
    public static let quotedAlpha: CGFloat = 0.6
    static var captionFont: NSFont { .systemFont(ofSize: 11, weight: .medium) }

    /// Width the caption takes, its arrow included.
    public static func captionWidth(_ caption: String) -> CGFloat {
        ceil(16 + (caption as NSString).size(withAttributes: [.font: captionFont]).width)
    }
    /// Photos shown; the last tile says how many more there are.
    public static let photosShown = 4

    /// The photo grid's tiles from its top left, as a messaging app lays them out: one
    /// wide photo, two side by side, one tall beside two, or four in a square.
    public static func photoTiles(_ count: Int) -> [CGRect] {
        let w = photoGrid, g = photoGap, half = (photoGrid - photoGap) / 2, small: CGFloat = 82
        switch count {
        case 0: return []
        case 1: return [CGRect(x: 0, y: 0, width: w, height: 168)]
        case 2: return [CGRect(x: 0, y: 0, width: half, height: 140), CGRect(x: half + g, y: 0, width: half, height: 140)]
        case 3: return [CGRect(x: 0, y: 0, width: half, height: 2 * small + g),
                        CGRect(x: half + g, y: 0, width: half, height: small),
                        CGRect(x: half + g, y: small + g, width: half, height: small)]
        default: return [CGRect(x: 0, y: 0, width: half, height: small), CGRect(x: half + g, y: 0, width: half, height: small),
                         CGRect(x: 0, y: small + g, width: half, height: small),
                         CGRect(x: half + g, y: small + g, width: half, height: small)]
        }
    }

    /// Height the photo grid takes, its gap above included.
    public static func photosHeight(_ count: Int) -> CGFloat {
        count == 0 ? 0 : thumbGap + (photoTiles(count).map(\.maxY).max() ?? 0)
    }

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
            + (spec.thumbnails.isEmpty ? 0 : thumbGap + thumb.height) + photosHeight(spec.photos.count)
            + (spec.caption == nil ? 0 : caption)
    }

    /// Height of a thread marker row.
    public static func marker(_ m: MarkerSpec) -> CGFloat { 12 + markerLine + (m.joined ? markerJoined + 5 : 0) }
}

// MARK: text

/// A bubble's selectable text: the view's own text stack, filled from the cache's layout.
/// A plain click (no selection, not on a link) is the bubble's click.
open class SelectableBubbleText: NSTextView {
    public var onClick: (() -> Void)?
    /// The cached layout on screen.
    public private(set) var drawn: BubbleTextLayout.Laid?
    /// Who had the keyboard before a click, given it back unless text was selected.
    private weak var previous: NSResponder?

    public static func make() -> Self {
        let v = Self(usingTextLayoutManager: true)
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
        drawn = laid
        if BubbleTextLayout.fill(self, from: laid) { setAccessibilityValue(laid.storage.string) }
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

/// Selectable text drawn from a cached layout: no text view while it is only read. The
/// pointer coming over it (or a click) shows a selectable text view filled with the same
/// text, holding its own text stack.
open class DrawnText: NSView {
    public let live = SelectableBubbleText.make()
    /// Layouts offered for the frame to come, by width; the frame picks one.
    private var offered: [Int: BubbleTextLayout.Laid] = [:]
    private var latest: BubbleTextLayout.Laid?
    public private(set) var laid: BubbleTextLayout.Laid?
    /// The text view is shown and holds `laid`; nothing is drawn here.
    public private(set) var isLive = false

    override open var isFlipped: Bool { true }

    public init() {
        super.init(frame: .zero)
        live.isHidden = true
        live.setAccessibilityElement(false)
        addSubview(live)
        setAccessibilityElement(true)
        setAccessibilityRole(.staticText)
    }

    public required init?(coder: NSCoder) { fatalError("not used") }

    /// A layout the frame may take (its size is the size answered for a proposal).
    public func offer(_ laid: BubbleTextLayout.Laid) {
        offered[Int(laid.size.width)] = laid
        latest = laid
        if self.laid == nil { take(laid) }
        // A frame of the same size lays nothing out by itself.
        needsLayout = true
    }

    override open func layout() {
        super.layout()
        if let pick = offered[Int(bounds.width)] ?? latest { take(pick) }
        offered.removeAll(keepingCapacity: true)
        live.frame = bounds
    }

    private func take(_ pick: BubbleTextLayout.Laid) {
        guard pick !== laid else { return }
        laid = pick
        setAccessibilityValue(pick.drawn.string)
        if isLive { goLive(false) }
        needsDisplay = true
    }

    /// Shows the selectable text view with this text, or hides it and draws again.
    public func goLive(_ on: Bool) {
        guard on != isLive || (on && live.drawn !== laid) else { return }
        if on, let laid { live.show(laid) }
        isLive = on
        if !on, window?.firstResponder === live { window?.makeFirstResponder(nil) }
        live.frame = bounds
        live.isHidden = !on
        needsDisplay = true
    }

    override open func updateTrackingAreas() {
        super.updateTrackingAreas()
        if trackingAreas.isEmpty {
            addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                           owner: self, userInfo: nil))
        }
    }

    override open func mouseEntered(with event: NSEvent) { goLive(true) }

    override open func mouseDown(with event: NSEvent) {
        goLive(true)
        live.mouseDown(with: event)
    }

    override open func draw(_ dirtyRect: NSRect) {
        if !isLive, let laid { laid.draw(in: CGRect(origin: .zero, size: laid.size)) }
    }
}

// MARK: rows

/// One compact bubble drawn by hand: avatar, name and time, rounded background and
/// border, its text, and for a clamped message its fade and "Show more". The text is
/// drawn from the cached layout; the selectable text view is filled and shown only once
/// the pointer comes over the bubble (or clicks it), so stepping through conversations
/// fills no text view.
final class BubbleRowView: NSView {
    let text = SelectableBubbleText.make()
    /// The layout this row shows.
    private var laid: BubbleTextLayout.Laid?
    /// The photo tiles, in this view.
    private(set) var photoRects: [CGRect] = []
    /// Opens the photos from the one at the index.
    var onPhoto: ((Int) -> Void)?
    /// The photo tile a press started on, and where: a drag takes the file out, a click opens it.
    private var pressed: (tile: Int, at: CGPoint)?
    /// The text view is shown and holds `laid`; the row draws no text of its own.
    private(set) var textIsLive = false
    private let fade = FadeView()
    /// "Show more", made the first time this view shows a clamped bubble.
    private var more: NSButton?
    private(set) var spec: BubbleSpec?
    private var colors = ChatColors.plain
    var onShowMore: (() -> Void)?
    /// The bubble's own rectangle, in this view.
    private(set) var bubbleRect: CGRect = .zero

    override var isFlipped: Bool { true }

    init() {
        super.init(frame: .zero)
        text.isHidden = true
        addSubview(text)
        addSubview(fade)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        toolTip = "Open the whole email"
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    @objc private func showMore() { onShowMore?() }

    /// The "Show more" button in the accent color, made once and retitled only when the
    /// color changes.
    private func showMoreButton(accent: NSColor) -> NSButton {
        let b = more ?? {
            let b = NSButton(title: "", target: self, action: #selector(showMore))
            b.isBordered = false
            b.setAccessibilityLabel("Show more")
            addSubview(b)
            more = b
            return b
        }()
        if moreAccent != accent {
            moreAccent = accent
            b.attributedTitle = NSAttributedString(string: "Show more", attributes: [
                .font: NSFont.systemFont(ofSize: 12), .foregroundColor: accent,
            ])
            b.sizeToFit()
        }
        return b
    }

    private var moreAccent: NSColor?

    func configure(_ spec: BubbleSpec, laid: BubbleTextLayout.Laid, width: CGFloat, colors: ChatColors, onClick: @escaping () -> Void) {
        self.spec = spec
        self.colors = colors
        text.onClick = onClick
        if laid !== self.laid {
            self.laid = laid
            if textIsLive { liveText(false) }
        }
        setAccessibilityValue(laid.drawn.string)
        let top = ChatMetrics.top(spec)
        // Wide enough for "Show more" and the file count under short text.
        let thumbs = CGFloat(spec.thumbnails.count) * (ChatMetrics.thumb.width + ChatMetrics.thumbGap) - ChatMetrics.thumbGap
        let floor: CGFloat = max(spec.content.clamped || spec.files > 0 ? 80 : 0, spec.thumbnails.isEmpty ? 0 : thumbs,
                                 spec.photos.isEmpty ? 0 : ChatMetrics.photoGrid,
                                 spec.caption.map { min(ChatMetrics.captionWidth($0), ChatMetrics.textWidth(width, mine: spec.mine)) } ?? 0)
        let bubbleSize = CGSize(width: max(laid.size.width, floor) + 2 * ChatMetrics.inset.width,
                                height: ChatMetrics.bubble(text: laid.size, spec))
        let x = spec.mine
            ? width - ChatMetrics.sidePadding - bubbleSize.width
            : ChatMetrics.sidePadding + ChatMetrics.avatar + ChatMetrics.avatarGap
        bubbleRect = CGRect(x: x, y: top, width: bubbleSize.width, height: bubbleSize.height)
        let captioned = spec.caption == nil ? 0 : ChatMetrics.caption
        text.frame = CGRect(x: x + ChatMetrics.inset.width, y: top + ChatMetrics.inset.height + captioned,
                            width: laid.size.width, height: laid.size.height)
        text.alphaValue = spec.quoted ? ChatMetrics.quotedAlpha : 1
        let gridTop = text.frame.maxY + (spec.content.clamped ? ChatMetrics.showMore : 0)
            + (spec.thumbnails.isEmpty ? 0 : ChatMetrics.thumbGap + ChatMetrics.thumb.height) + ChatMetrics.thumbGap
        photoRects = ChatMetrics.photoTiles(min(spec.photos.count, ChatMetrics.photosShown))
            .map { $0.offsetBy(dx: text.frame.minX, dy: gridTop) }
        fade.isHidden = !spec.content.clamped
        more?.isHidden = !spec.content.clamped
        if spec.content.clamped {
            fade.color = spec.mine ? colors.mineBackground : colors.theirsBackground
            fade.frame = CGRect(x: text.frame.minX, y: text.frame.maxY - 22, width: text.frame.width, height: 22)
            let more = showMoreButton(accent: colors.accent)
            more.frame.origin = CGPoint(x: text.frame.minX, y: text.frame.maxY + 3)
        }
        setAccessibilityLabel("Message from \(spec.mine ? "you" : spec.sender)"
            + (spec.files > 0 ? ", \(spec.files) file\(spec.files == 1 ? "" : "s") attached" : "")
            + (spec.photos.isEmpty ? "" : ", \(spec.photos.count) photo\(spec.photos.count == 1 ? "" : "s")")
            + (spec.caption.map { ". \($0)" } ?? "") + (spec.quoted ? ". Quotes an earlier message" : "")
            + (spec.note.map { ". \($0)" } ?? ""))
        setAccessibilityHelp("Opens the whole email")
        needsDisplay = true
    }

    /// Shows the selectable text view filled with this row's text, or hides it and draws
    /// the text again.
    func liveText(_ on: Bool) {
        guard on != textIsLive || (on && text.drawn !== laid) else { return }
        if on, let laid { text.show(laid) }
        textIsLive = on
        if !on, window?.firstResponder === text { window?.makeFirstResponder(superview) }
        text.isHidden = !on
        needsDisplay = true
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if trackingAreas.isEmpty {
            addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                           owner: self, userInfo: nil))
        }
    }

    /// The pointer over the bubble: the text becomes selectable, its links clickable.
    override func mouseEntered(with event: NSEvent) { liveText(true) }

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
                drawAvatar(spec.sender, key: spec.senderKey, in: CGRect(x: ChatMetrics.sidePadding, y: y, width: ChatMetrics.avatar, height: ChatMetrics.avatar))
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
            drawPhotos(spec.photos)
        }
        let shape = NSBezierPath(roundedRect: bubbleRect.insetBy(dx: 0.5, dy: 0.5), xRadius: ChatMetrics.radius, yRadius: ChatMetrics.radius)
        (spec.mine ? colors.mineBackground : colors.theirsBackground).setFill()
        shape.fill()
        (spec.focused ? colors.accent.withAlphaComponent(0.6) : colors.border).setStroke()
        shape.lineWidth = spec.focused ? 1.5 : 1
        shape.stroke()
        if let caption = spec.caption { drawCaption(caption) }
        if spec.quoted {
            // A quote bar in the bubble's left margin, beside the dimmed text.
            colors.secondary.withAlphaComponent(0.35).setFill()
            NSBezierPath(roundedRect: CGRect(x: bubbleRect.minX + 5, y: text.frame.minY, width: 2.5, height: text.frame.height),
                         xRadius: 1.25, yRadius: 1.25).fill()
        }
        if !textIsLive, let laid {
            if spec.quoted {
                NSGraphicsContext.current?.cgContext.saveGState()
                NSGraphicsContext.current?.cgContext.setAlpha(ChatMetrics.quotedAlpha)
                laid.draw(in: text.frame)
                NSGraphicsContext.current?.cgContext.restoreGState()
            } else {
                laid.draw(in: text.frame)
            }
        }
    }

    var file: (String) async -> URL? = { _ in nil }

    /// "↪ Forwarded from Nora" over the text, in the quieter color.
    private func drawCaption(_ caption: String) {
        let x = text.frame.minX, y = text.frame.minY - ChatMetrics.caption
        if let arrow = NSImage(systemSymbolName: "arrowshape.turn.up.right", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 10, weight: .regular)) {
            colors.secondary.set()
            arrow.draw(in: CGRect(x: x, y: y + 2, width: 12, height: 11), from: .zero, operation: .sourceOver, fraction: 0.8,
                       respectFlipped: true, hints: nil)
        }
        NSAttributedString(string: caption, attributes: [
            .font: ChatMetrics.captionFont, .foregroundColor: colors.secondary,
        ]).draw(with: CGRect(x: x + 16, y: y, width: max(0, bubbleRect.maxX - ChatMetrics.inset.width - x - 16), height: 15),
                options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }

    /// Photos past the grid, counted on its last tile ("+2").
    var hiddenPhotos: Int { max(0, (spec?.photos.count ?? 0) - ChatMetrics.photosShown) }

    /// Attached photos as a grid; past four, the last tile says how many more there are.
    private func drawPhotos(_ photos: [Attachment]) {
        for (i, r) in photoRects.enumerated() where i < photos.count {
            drawTile(photos[i].id, in: r)
            let more = hiddenPhotos
            guard i == ChatMetrics.photosShown - 1, more > 0 else { continue }
            NSColor.black.withAlphaComponent(0.45).setFill()
            NSBezierPath(roundedRect: r, xRadius: 10, yRadius: 10).fill()
            let label = NSAttributedString(string: "+\(more)", attributes: [
                .font: NSFont.systemFont(ofSize: 22, weight: .semibold), .foregroundColor: NSColor.white,
            ])
            let s = label.size()
            label.draw(at: CGPoint(x: r.midX - s.width / 2, y: r.midY - s.height / 2))
        }
    }

    /// One image tile: the image cropped to fill it once decoded, a soft placeholder until
    /// then (asking for it, off the main thread).
    private func drawTile(_ id: String, in r: CGRect) {
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

    /// Pasted images as thumbnails under the text, decoded small and off the main thread.
    private func drawThumbnails(_ ids: [String]) {
        guard !ids.isEmpty else { return }
        let y = text.frame.maxY + (spec?.content.clamped == true ? ChatMetrics.showMore : 0) + ChatMetrics.thumbGap
        for (i, id) in ids.enumerated() {
            drawTile(id, in: CGRect(x: text.frame.minX + CGFloat(i) * (ChatMetrics.thumb.width + ChatMetrics.thumbGap), y: y,
                                    width: ChatMetrics.thumb.width, height: ChatMetrics.thumb.height))
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

    private func drawAvatar(_ name: String, key: String, in rect: CGRect) {
        if !colors.avatars.isEmpty { colors.avatars[SenderIdentity.colorIndex(key, count: colors.avatars.count)].setFill() }
        NSBezierPath(ovalIn: rect).fill()
        let label = NSAttributedString(string: SenderIdentity.initials(name), attributes: [
            .font: NSFont.systemFont(ofSize: 11 * rect.width / 28, weight: .bold), .foregroundColor: NSColor.white,
        ])
        let s = label.size()
        label.draw(at: CGPoint(x: rect.midX - s.width / 2, y: rect.midY - s.height / 2))
    }

    /// A click on the text goes to the text view (made live first); clicks on the bubble
    /// outside its text open the whole email too.
    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if let tile = photoRects.firstIndex(where: { $0.contains(p) }) {
            pressed = (tile, p)
        } else if text.frame.contains(p) {
            liveText(true)
            text.mouseDown(with: event)
        } else if bubbleRect.contains(p) {
            text.onClick?()
        } else {
            super.mouseDown(with: event)
        }
    }
}

extension BubbleRowView: NSDraggingSource {
    override func mouseDragged(with event: NSEvent) {
        guard let (tile, at) = pressed, let spec, tile < spec.photos.count else { return super.mouseDragged(with: event) }
        let p = convert(event.locationInWindow, from: nil)
        guard hypot(p.x - at.x, p.y - at.y) > 4 else { return }
        pressed = nil
        // The file as it is stored, under its own name; not yet fetched, nothing to drag.
        guard let url = Thumbnails.shared.url(spec.photos[tile].id) else { return }
        let item = NSDraggingItem(pasteboardWriter: url as NSURL)
        let image = Thumbnails.shared.image(spec.photos[tile].id).map { NSImage(cgImage: $0, size: photoRects[tile].size) }
        item.setDraggingFrame(photoRects[tile], contents: image)
        beginDraggingSession(with: [item], event: event, source: self)
    }

    override func mouseUp(with event: NSEvent) {
        guard let (tile, _) = pressed else { return super.mouseUp(with: event) }
        pressed = nil
        onPhoto?(tile)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .outsideApplication ? .copy : []
    }

    /// Presses a photo tile, as a click would.
    func pressPhoto(_ tile: Int) { onPhoto?(tile) }
}

/// Quick Look over a chat's photos, driven from the chat's document view (the panel asks
/// the responder chain who controls it).
@MainActor
final class PhotoPreview: NSObject, @MainActor QLPreviewPanelDataSource {
    static let shared = PhotoPreview()
    var urls: [URL] = []

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { urls.count }

    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! { urls[index] as NSURL }

    /// Shows `urls` from the one at `index`, controlled by `controller`.
    func show(_ urls: [URL], at index: Int, from controller: NSView) {
        self.urls = urls
        controller.window?.makeFirstResponder(controller)
        guard let panel = QLPreviewPanel.shared() else { return }
        if panel.isVisible { panel.reloadData() } else { panel.makeKeyAndOrderFront(nil) }
        panel.currentPreviewItemIndex = index
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

/// A SwiftUI row inside the stream (the whole email opened in place, the summary). It is
/// measured by SwiftUI itself at the stream's width, and any change of its ideal size (an
/// email's HTML or images loading, a section unfolding) lays the stream out again.
final class HostedRowView: NSView {
    var key = ""
    let controller: NSHostingController<AnyView>
    private var watch: NSKeyValueObservation?

    override var isFlipped: Bool { true }

    init(rootView: AnyView) {
        controller = NSHostingController(rootView: rootView)
        // The controller publishes its ideal size; a change means this row grew or shrank.
        controller.sizingOptions = [.preferredContentSize]
        super.init(frame: .zero)
        let v = controller.view
        v.autoresizingMask = [.width, .height]
        v.frame = bounds
        addSubview(v)
        watch = controller.observe(\.preferredContentSize) { [weak self] _, _ in
            DispatchQueue.main.async { self?.superview?.needsLayout = true }
        }
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    var rootView: AnyView {
        get { controller.rootView }
        set { controller.rootView = newValue }
    }

    /// The row's height at `width`, asked of SwiftUI: the content's true height, not the
    /// frame the row last had.
    func height(at width: CGFloat) -> CGFloat {
        ceil(controller.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude)).height)
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

    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool {
        MainActor.assumeIsolated { !PhotoPreview.shared.urls.isEmpty }
    }

    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated { panel.dataSource = PhotoPreview.shared }
    }

    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) { MainActor.assumeIsolated { panel.dataSource = nil } }

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

    /// Bubble and marker views no row uses: hidden in the document, handed out before a
    /// new one is made, so stepping through conversations builds no views.
    private var spareBubbles: [BubbleRowView] = []
    private var spareMarkers: [MarkerRowView] = []
    /// Bubble views made ahead, in idle time, once the stream first shows.
    static let warmBubbles = 16
    private var warmed = false

    /// Rows whose kind changed get a view from the spares (or a new one); the others are
    /// reconfigured in place.
    private func rebuildViews() {
        var next: [NSView] = []
        // Bubble and marker views not kept at their place go to the spares first.
        for (i, v) in views.enumerated() {
            let kept: Bool
            switch i < rows.count ? rows[i] : nil {
            case .bubble?: kept = v is BubbleRowView
            case .marker?: kept = v is MarkerRowView
            default: kept = false
            }
            guard !kept else { continue }
            if let b = v as? BubbleRowView { spareBubbles.append(b) } else if let m = v as? MarkerRowView { spareMarkers.append(m) }
        }
        for (i, row) in rows.enumerated() {
            let old = i < views.count ? views[i] : nil
            switch row {
            case .earlier:
                next.append((old as? NSButton) ?? makeEarlierButton())
            case .bubble:
                next.append((old as? BubbleRowView) ?? spareBubbles.popLast() ?? BubbleRowView())
            case .marker:
                next.append((old as? MarkerRowView) ?? spareMarkers.popLast() ?? MarkerRowView())
            case let .hosted(key):
                if let h = old as? HostedRowView, h.key == key {
                    next.append(h)
                } else if let h = old as? HostedRowView {
                    h.rootView = actions.hosted(key)
                    h.key = key
                    next.append(h)
                } else {
                    let h = HostedRowView(rootView: actions.hosted(key))
                    h.key = key
                    next.append(h)
                }
            }
        }
        let used = Set(next.map(ObjectIdentifier.init))
        for v in views where !used.contains(ObjectIdentifier(v)) {
            if v is BubbleRowView || v is MarkerRowView { v.isHidden = true } else { v.removeFromSuperview() }
        }
        for v in next {
            if v.superview !== document { document.addSubview(v) }
            if v.isHidden { v.isHidden = false }
        }
        views = next
        warmUp()
    }

    /// Makes spare bubble views after the first conversation shows, off the step itself.
    private func warmUp() {
        guard !warmed else { return }
        warmed = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let missing = Self.warmBubbles - self.views.count { $0 is BubbleRowView } - self.spareBubbles.count
            for _ in 0..<max(0, missing) {
                let b = BubbleRowView()
                b.isHidden = true
                self.document.addSubview(b)
                self.spareBubbles.append(b)
            }
        }
    }

    /// Views kept hidden for later rows, for tests.
    public var spareCount: Int { spareBubbles.count + spareMarkers.count }

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
                (v as? BubbleRowView)?.onPhoto = { [weak self] i in self?.openPhotos(spec.photos, at: i) }
                y += h
            case let .marker(m):
                let h = ChatMetrics.marker(m)
                v.frame = CGRect(x: 0, y: y, width: width, height: h)
                (v as? MarkerRowView)?.configure(m, colors: colors) { [weak self] in self?.actions.split(m.threadId) }
                y += h
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

    /// Fetches the photos' files (already stored ones at once) and shows them from `index`.
    private func openPhotos(_ photos: [Attachment], at index: Int) {
        let file = actions.file
        let preview = actions.previewPhotos
        Task { @MainActor [weak self] in
            var urls: [URL] = []
            var at = 0
            for (i, p) in photos.enumerated() {
                let stored = Thumbnails.shared.url(p.id)
                guard let url = stored == nil ? await file(p.id) : stored else { continue }
                if i == index { at = urls.count }
                urls.append(url)
            }
            guard !urls.isEmpty, let self else { return }
            if let preview { preview(urls, at) } else { PhotoPreview.shared.show(urls, at: at, from: self.document) }
        }
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

    /// The spec a message's bubble is drawn from.
    public func bubbleSpec(_ id: String) -> BubbleSpec? {
        for (row, v) in zip(rows, views) {
            if case let .bubble(b) = row, b.id == id { return (v as? BubbleRowView)?.spec }
        }
        return nil
    }

    /// The avatar color a bubble row draws, if it shows one.
    public func avatarColor(_ id: String) -> NSColor? {
        guard let spec = bubbleSpec(id), spec.firstOfRun, !spec.mine, !colors.avatars.isEmpty else { return nil }
        return colors.avatars[SenderIdentity.colorIndex(spec.senderKey, count: colors.avatars.count)]
    }

    /// A message's selectable text view, made live as when the pointer comes over it.
    public func bubbleText(_ id: String) -> SelectableBubbleText? {
        guard let r = bubbleRow(id) else { return nil }
        r.liveText(true)
        return r.text
    }

    /// The row drawing a message's bubble.
    func bubbleRow(_ id: String) -> BubbleRowView? {
        for (row, v) in zip(rows, views) {
            if case let .bubble(b) = row, b.id == id { return v as? BubbleRowView }
        }
        return nil
    }

    /// Presses "Show more" on a message, as a click would.
    public func pressShowMore(_ id: String) {
        for (row, v) in zip(rows, views) {
            if case let .bubble(b) = row, b.id == id { (v as? BubbleRowView)?.onShowMore?() }
        }
    }

    /// Scrolls to the very end, as a reader dragging to the bottom would.
    public func scrollToEnd() {
        contentView.scroll(to: CGPoint(x: 0, y: max(0, document.frame.height - contentView.bounds.height)))
        reflectScrolledClipView(contentView)
    }

    /// The part of the chat on screen, in the document's coordinates.
    public var visibleArea: CGRect { contentView.documentVisibleRect }

    /// A hosted row's frame in the document, by key.
    public func hostedFrame(_ key: String) -> CGRect? {
        zip(rows, views).first { if case let .hosted(k) = $0.0 { k == key } else { false } }?.1.frame
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
    /// The color the link was last titled in.
    private var splitAccent: NSColor?

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
        if spec.joined, splitAccent != colors.accent {
            splitAccent = colors.accent
            split.attributedTitle = NSAttributedString(string: "Not the same conversation", attributes: [
                .font: NSFont.systemFont(ofSize: 11.5), .foregroundColor: colors.accent,
            ])
            split.sizeToFit()
        }
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
    /// Files already fetched, by attachment id: dragged out and shown in Quick Look as they are.
    private var files: [String: URL] = [:]
    private var loading: Set<String> = []
    private let decoder: @Sendable (URL) -> CGImage?

    init(decode: @escaping @Sendable (URL) -> CGImage? = Thumbnails.decode) { decoder = decode }

    func url(_ id: String) -> URL? {
        lock.lock()
        defer { lock.unlock() }
        return files[id]
    }

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
        let decoder = decoder
        Task {
            guard let url = await file(id) else { return }
            let image = await Task.detached(priority: .utility) { decoder(url) }.value
            self.store(id, image, url: url)
            done()
        }
    }

    private func store(_ id: String, _ image: CGImage?, url: URL) {
        lock.lock()
        defer { lock.unlock() }
        if images.count > 500 { images.removeAll(); files.removeAll() }
        images[id] = image
        files[id] = url
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
