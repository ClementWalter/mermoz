/// Formatted-message renderer: a script-free WKWebView per message, remote loads off.
import AppKit
import SwiftUI
import WebKit

/// Sized to its content inside the thread's scroll view, so wheel events belong to that
/// scroll view; without forwarding, scrolling stops while the pointer is over a message.
final class PassthroughWebView: WKWebView {
    override func scrollWheel(with event: NSEvent) {
        nextResponder?.scrollWheel(with: event)
    }
}

/// One configuration for every message view: shared process and no persistent storage.
@MainActor
enum WebConfig {
    static let shared: WKWebViewConfiguration = {
        let config = WKWebViewConfiguration()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        config.websiteDataStore = .nonPersistent()
        // The email's own scripts never run; this one, the app's, runs in its own world and
        // reports the page's full height whenever it changes (images, fonts, reflow).
        config.userContentController.addUserScript(WKUserScript(source: heightScript, injectionTime: .atDocumentEnd,
                                                                forMainFrameOnly: true, in: .defaultClient))
        config.userContentController.add(HeightRelay.shared, contentWorld: .defaultClient, name: HeightRelay.name)
        return config
    }()

    /// Posts the content height now, on every resize of the page, as each late image
    /// loads, once fonts are ready and on window load.
    static let heightScript = """
    (function () {
      function post() {
        // The body's own height, not the page's: the page is never shorter than the view,
        // so measuring it would never let the view shrink.
        var b = document.body;
        var h = b ? Math.ceil(Math.max(b.scrollHeight, b.getBoundingClientRect().height))
                  : document.documentElement.scrollHeight;
        window.webkit.messageHandlers.\(HeightRelay.name).postMessage(h);
      }
      var ro = new ResizeObserver(post);
      ro.observe(document.documentElement);
      if (document.body) ro.observe(document.body);
      Array.prototype.forEach.call(document.images, function (i) {
        if (!i.complete) { i.addEventListener('load', post); i.addEventListener('error', post); }
      });
      if (document.fonts && document.fonts.ready) document.fonts.ready.then(post);
      window.addEventListener('load', post);
      post();
    })();
    """

    /// CSP blocks every remote fetch (trackers, scripts, fonts); remote images are let
    /// through only when `images` (the reader asked, or the sender is verified).
    static func page(_ html: String, images: Bool = false) -> String {
        let img = images ? "data: cid: https: http:" : "data: cid:"
        let csp = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src \(img); style-src 'unsafe-inline'; font-src data:\">"
        let base = "<style>html,body{background:transparent}body{font:14px -apple-system,system-ui;color:#1A3346;margin:0;word-wrap:break-word;overflow:hidden}img{max-width:100%;height:auto}table{max-width:100%}</style>"
        return "<!doctype html><html><head><meta charset=\"utf-8\">\(csp)\(base)</head><body>\(html)</body></html>"
    }
}

/// The one handler for every message view's height reports (the configuration is
/// shared): each report goes to the coordinator of the web view that sent it.
@MainActor
final class HeightRelay: NSObject, WKScriptMessageHandler {
    static let shared = HeightRelay()
    static let name = "mermozHeight"
    private var coordinators: [ObjectIdentifier: () -> HTMLMessageView.Coordinator?] = [:]

    func register(_ webView: WKWebView, _ c: HTMLMessageView.Coordinator) {
        coordinators[ObjectIdentifier(webView)] = { [weak c] in c }
    }

    func unregister(_ webView: WKWebView) { coordinators[ObjectIdentifier(webView)] = nil }

    func userContentController(_ ucc: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let wv = message.webView, let h = (message.body as? NSNumber).map({ CGFloat($0.doubleValue) }), h > 0,
              let c = coordinators[ObjectIdentifier(wv)]?() else { return }
        c.onHeight?(h)
    }
}

struct HTMLMessageView: NSViewRepresentable {
    let html: String
    var images = false
    @Binding var height: CGFloat

    func makeNSView(context: Context) -> PassthroughWebView {
        let wv = PassthroughWebView(frame: .zero, configuration: WebConfig.shared)
        wv.setValue(false, forKey: "drawsBackground")
        wv.navigationDelegate = context.coordinator
        context.coordinator.webView = wv
        HeightRelay.shared.register(wv, context.coordinator)
        return wv
    }

    static func dismantleNSView(_ wv: PassthroughWebView, coordinator: Coordinator) {
        HeightRelay.shared.unregister(wv)
    }

    func updateNSView(_ wv: PassthroughWebView, context: Context) {
        context.coordinator.onHeight = { h in if abs(height - h) > 1 { height = max(h, 20) } }
        if context.coordinator.loaded != html {
            context.coordinator.loaded = html
            wv.loadHTMLString(WebConfig.page(html, images: images), baseURL: nil)
        } else {
            // Width changes reflow the text; re-measure so nothing gets clipped.
            context.coordinator.measure()
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded: String?
        var onHeight: ((CGFloat) -> Void)?
        weak var webView: WKWebView?

        func measure() {
            // App-evaluated JS still runs with page scripts disabled; used only to size the view.
            webView?.evaluateJavaScript("document.body.scrollHeight") { [weak self] v, _ in
                if let h = v as? CGFloat { self?.onHeight?(h) }
            }
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { measure() }

        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
            // Clicked links open in the user's browser, never inside the message pane.
            if action.navigationType == .linkActivated, let url = action.request.url {
                NSWorkspace.shared.open(url)
                return .cancel
            }
            return .allow
        }
    }
}
