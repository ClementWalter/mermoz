/// The composer's body: a plain-text NSTextView that leaves file drops to the
/// composer (so a drop anywhere attaches) and turns pasted files or images into
/// attachments instead of text.
import AppKit
import SwiftUI

struct ComposeBodyEditor: NSViewRepresentable {
    @Binding var text: String
    var focusOnAppear: Bool
    var onFiles: ([URL]) -> Void
    var onImage: (Data) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        let tv = BodyTextView(frame: .zero)
        tv.isRichText = false
        tv.importsGraphics = false
        tv.allowsUndo = true
        tv.drawsBackground = false
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = false
        tv.autoresizingMask = [.width]
        tv.textContainer?.widthTracksTextView = true
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        let style = NSMutableParagraphStyle()
        style.lineSpacing = 4
        let font = NSFont.systemFont(ofSize: 14)
        tv.font = font
        tv.defaultParagraphStyle = style
        tv.typingAttributes = [.font: font, .paragraphStyle: style, .foregroundColor: NSColor(Theme.text)]
        tv.textColor = NSColor(Theme.text)
        tv.string = text
        tv.delegate = context.coordinator
        tv.onFiles = onFiles
        tv.onImage = onImage
        tv.updateDragTypeRegistration()
        scroll.documentView = tv
        if focusOnAppear {
            DispatchQueue.main.async { tv.window?.makeFirstResponder(tv) }
        }
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let tv = scroll.documentView as? BodyTextView else { return }
        if tv.string != text { tv.string = text }
        tv.onFiles = onFiles
        tv.onImage = onImage
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var text: Binding<String>
        init(text: Binding<String>) { self.text = text }

        func textDidChange(_ n: Notification) {
            guard let tv = n.object as? NSTextView else { return }
            text.wrappedValue = tv.string
        }
    }
}

final class BodyTextView: NSTextView {
    var onFiles: (([URL]) -> Void)?
    var onImage: ((Data) -> Void)?

    /// Text only: file drags fall through to the composer, which attaches them.
    override var acceptableDragTypes: [NSPasteboard.PasteboardType] { [.string] }

    override func paste(_ sender: Any?) {
        let pb = NSPasteboard.general
        if let files = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
           !files.isEmpty {
            onFiles?(files)
            return
        }
        if pb.availableType(from: [.string]) == nil,
           let type = pb.availableType(from: [.png, .tiff]),
           let data = pb.data(forType: type) {
            onImage?(data)
            return
        }
        super.paste(sender)
    }
}
