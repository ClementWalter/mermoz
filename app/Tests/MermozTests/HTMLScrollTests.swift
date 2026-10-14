/// A long HTML email opened in the chat scrolls with the chat to its very end, even when
/// it grows after showing (late images): the web view reports its whole height and never
/// keeps a scroll of its own.
import AppKit
@testable import MermozKit
import SwiftUI
import Testing
import WebKit
@testable import Mermoz

/// A transactional email: cards, a table, and a picture whose box grows once "loaded".
private let fixture: String = {
    let cards = (1...25).map { "<div style=\"padding:16px;margin:8px 0;border:1px solid #ddd\"><h3>Step \($0)</h3><p>Scan the QR code, then follow the instructions on your phone to install the eSIM profile.</p></div>" }
        .joined()
    return cards + "<div id=\"late\" style=\"height:0\"></div>"
        + "<table><tr><td>Access Point Name (APN)</td><td id=\"end\">mobimatter</td></tr></table>"
}()

/// The email as the chat hosts it: the web view sized by the height it reports.
private struct Email: View {
    @State private var height: CGFloat = 40
    var body: some View { HTMLMessageView(html: fixture, height: $height).frame(height: height) }
}

@MainActor
private final class Chat {
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 700, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)

    init() {
        window.contentView = stream
        stream.actions.hosted = { _ in AnyView(Email()) }
        stream.show([.hosted(key: "email")], conversation: "c", messages: 1)
        settle()
    }

    func settle() {
        stream.layoutSubtreeIfNeeded()
        stream.documentView?.layoutSubtreeIfNeeded()
    }

    var row: CGRect { stream.hostedFrame("email") ?? .zero }

    var web: WKWebView? {
        func find(_ v: NSView) -> WKWebView? {
            if let w = v as? WKWebView { return w }
            for s in v.subviews { if let w = find(s) { return w } }
            return nil
        }
        return find(stream)
    }

    /// Waits (at most `seconds`) until `ok` holds, laying the chat out as it goes.
    func wait(_ seconds: Double = 20, until ok: () -> Bool) async {
        let end = Date().addingTimeInterval(seconds)
        while !ok(), Date() < end {
            try? await Task.sleep(for: .milliseconds(50))
            settle()
        }
    }

    /// The page's own height, as the page measures it.
    func pageHeight() async -> CGFloat {
        (try? await web?.evaluateJavaScript("document.body.getBoundingClientRect().height") as? Double).map { CGFloat($0) } ?? 0
    }

    /// A late image arrives: the box before the end grows by `px`.
    func grow(_ px: Int) async {
        _ = try? await web?.evaluateJavaScript("document.getElementById('late').style.height='\(px)px'; 0")
    }
}

/// The email loaded and measured once, then grown by 800 px.
@MainActor
private func grownEmail() async -> Chat {
    let c = Chat()
    await c.wait { c.row.height > 1_000 }
    let before = c.row.height
    await c.grow(800)
    await c.wait { c.row.height >= before + 790 }
    return c
}

/// One web view at a time: their pages load in a shared web process, slow under load.
@Suite(.serialized) @MainActor struct HTMLScroll {
@Test func theEmailIsAsTallAsItsWholePage() async {
    let c = Chat()
    await c.wait { c.row.height > 1_000 }
    let page = await c.pageHeight()
    #expect(c.row.height >= page - 1)
}

@Test func theEmailGrowsWhenALateImageArrives() async {
    let c = await grownEmail()
    let page = await c.pageHeight()
    #expect(c.row.height >= page - 1)
}

@Test func theEndOfAGrownEmailCanBeScrolledTo() async {
    let c = await grownEmail()
    c.stream.scrollToEnd()
    #expect(c.stream.visibleArea.maxY >= c.row.maxY)
}

@Test func theWebViewNeverScrollsInside() async {
    let c = await grownEmail()
    let inner = (try? await c.web?.evaluateJavaScript("window.scrollY") as? Double) ?? -1
    #expect(inner == 0)
}

@Test func aScrollOverTheEmailScrollsTheChat() async {
    let c = Chat()
    await c.wait { c.row.height > 1_000 }
    // The chat opens on its end; a trackpad scroll up over the email, in the middle of
    // the pane, goes to whatever view the window finds there.
    let before = c.stream.contentView.bounds.minY
    guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: .line, wheelCount: 1, wheel1: 5, wheel2: 0, wheel3: 0),
          let e = NSEvent(cgEvent: cg) else { Issue.record("no event"); return }
    let inWindow = c.stream.convert(CGPoint(x: c.stream.bounds.midX, y: c.stream.bounds.midY), to: nil)
    c.window.contentView?.hitTest(inWindow)?.scrollWheel(with: e)
    #expect(c.stream.contentView.bounds.minY < before)
}

@Test func theWindowHandsAScrollOverTheEmailToTheWebView() async {
    let c = Chat()
    await c.wait { c.row.height > 1_000 }
    let inWindow = c.stream.convert(CGPoint(x: c.stream.bounds.midX, y: c.stream.bounds.midY), to: nil)
    guard let web = c.web else { Issue.record("no web view"); return }
    let hit = c.window.contentView?.hitTest(inWindow)
    #expect(hit.map { $0 === web || $0.isDescendant(of: web) } == true)
}
}
