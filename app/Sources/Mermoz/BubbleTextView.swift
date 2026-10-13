/// A chat bubble's text as an AppKit text view: drawn by TextKit from the shared caches
/// (BubbleTextCache), selectable and copyable, with clickable links. A plain click (no
/// selection, not on a link) opens the whole email, as clicking the bubble does.
import AppKit
import MermozKit
import SwiftUI

/// The rounded background and border a plain bubble draws itself.
struct BubbleChrome: Equatable {
    var background: Color
    var border: Color
    var borderWidth: CGFloat
    /// Text inset from the bubble's edge.
    static let inset = CGSize(width: 12, height: 8)
    static let radius: CGFloat = 15
}

struct BubbleTextView: NSViewRepresentable {
    let id: String
    let content: BubbleContent
    /// Nil: the text alone, inside a SwiftUI bubble.
    var chrome: BubbleChrome?
    let onClick: () -> Void

    @Environment(\.dynamicTypeSize) private var typeSize

    /// The text color resolved for the current appearance; dark mode gets its own key.
    private func style(_ env: EnvironmentValues) -> BubbleTextStyle {
        let c = Theme.text.resolve(in: env)
        return BubbleTextStyle(pointSize: (13.5 * Self.scale(typeSize)).rounded(toPlaces: 1), lineSpacing: 2.5,
                               red: CGFloat(c.red), green: CGFloat(c.green), blue: CGFloat(c.blue), alpha: 0.92)
    }

    /// Larger text settings scale the bubble font like SwiftUI's body text.
    static func scale(_ s: DynamicTypeSize) -> CGFloat {
        switch s {
        case .xSmall: 0.82
        case .small: 0.88
        case .medium: 0.94
        case .large: 1
        case .xLarge: 1.12
        case .xxLarge: 1.24
        case .xxxLarge: 1.36
        default: 1.6
        }
    }

    func makeNSView(context: Context) -> BubbleNSTextView {
        // TextKit 1: the view adopts containers laid out by the cache.
        let v = BubbleNSTextView(usingTextLayoutManager: false)
        v.isEditable = false
        v.isSelectable = true
        v.drawsBackground = false
        v.textContainerInset = .zero
        v.isVerticallyResizable = false
        v.isHorizontallyResizable = false
        v.linkTextAttributes = [.underlineStyle: NSUnderlineStyle.single.rawValue, .cursor: NSCursor.pointingHand]
        v.setAccessibilityRole(.staticText)
        return v
    }

    func updateNSView(_ v: BubbleNSTextView, context: Context) {
        let style = style(context.environment)
        v.onClick = onClick
        v.setAccessibilityLabel(content.text)
        v.source = (id, content, style)
        let inset = chrome == nil ? .zero : BubbleChrome.inset
        if v.textContainerInset != inset { v.textContainerInset = inset }
        if let chrome {
            let env = context.environment
            v.wantsLayer = true
            v.layer?.cornerRadius = BubbleChrome.radius
            v.layer?.backgroundColor = Color(chrome.background.resolve(in: env)).cgColor ?? .clear
            v.layer?.borderColor = Color(chrome.border.resolve(in: env)).cgColor ?? .clear
            v.layer?.borderWidth = chrome.borderWidth
        } else if v.layer?.borderWidth != 0 {
            v.layer?.backgroundColor = nil
            v.layer?.borderWidth = 0
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView v: BubbleNSTextView, context: Context) -> CGSize? {
        let inset = chrome == nil ? .zero : BubbleChrome.inset
        let width = proposal.width.map { $0.isFinite ? $0 - 2 * inset.width : BubbleTextCache.idealWidth } ?? BubbleTextCache.idealWidth
        let laid = BubbleTextCache.shared.layout(id: id, text: content.text, style: style(context.environment),
                                                 width: width, maxLines: content.maxLines)
        let size = CGSize(width: laid.size.width + 2 * inset.width, height: laid.size.height + 2 * inset.height)
        // The frame SwiftUI settles on is one of the sizes answered here: remember its layout.
        v.offered[Int(size.width)] = laid
        return size
    }
}

final class BubbleNSTextView: NSTextView {
    var onClick: (() -> Void)?
    /// What to draw: message id, text and style, to find layouts for a width.
    var source: (id: String, content: BubbleContent, style: BubbleTextStyle)?
    /// Layouts answered to SwiftUI by width; the final frame picks one.
    var offered: [Int: BubbleTextLayout.Laid] = [:]
    /// The cached layout on screen.
    private var drawn: BubbleTextLayout.Laid?

    /// Draws the cached layout that matches the frame: no measuring, no typesetting.
    override func layout() {
        super.layout()
        guard let source else { return }
        let laid = offered[Int(bounds.width)] ?? BubbleTextCache.shared.layout(
            id: source.id, text: source.content.text, style: source.style,
            width: bounds.width - 2 * textContainerInset.width, maxLines: source.content.maxLines)
        offered.removeAll(keepingCapacity: true)
        guard laid !== drawn else { return }
        drawn = laid
        textContainer = laid.container
        needsDisplay = true
    }
    /// Who had the keyboard before a click, given it back unless text was selected.
    private weak var previous: NSResponder?

    override func mouseDown(with event: NSEvent) {
        previous = window?.firstResponder
        let point = convert(event.locationInWindow, from: nil)
        let onLink = characterIndexForInsertion(at: point) < (textStorage?.length ?? 0)
            && textStorage?.attribute(.link, at: characterIndexForInsertion(at: point), effectiveRange: nil) != nil
        // Tracks the drag (selection) or the link click until the mouse is released.
        super.mouseDown(with: event)
        guard !onLink, selectedRange().length == 0 else { return }
        restoreKeyboard()
        onClick?()
    }

    /// ↑/↓, Enter and Esc keep stepping through messages even after text was selected.
    override func keyDown(with event: NSEvent) {
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

private extension CGFloat {
    func rounded(toPlaces p: Int) -> CGFloat {
        let m = pow(10, CGFloat(p))
        return (self * m).rounded() / m
    }
}
