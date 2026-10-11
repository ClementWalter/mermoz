/// Formatted-message renderer: a script-free WKWebView per expanded message, remote loads off.
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
        return config
    }()

    /// CSP blocks every remote fetch (trackers, images) — only inline styles and data: images render.
    static func page(_ html: String) -> String {
        let csp = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'; font-src data:\">"
        let base = "<style>html,body{background:transparent}body{font:14px -apple-system,system-ui;color:#1A3346;margin:0;word-wrap:break-word;overflow:hidden}img{max-width:100%;height:auto}table{max-width:100%}</style>"
        return "<!doctype html><html><head><meta charset=\"utf-8\">\(csp)\(base)</head><body>\(html)</body></html>"
    }
}

struct HTMLMessageView: NSViewRepresentable {
    let html: String
    @Binding var height: CGFloat

    func makeNSView(context: Context) -> PassthroughWebView {
        let wv = PassthroughWebView(frame: .zero, configuration: WebConfig.shared)
        wv.setValue(false, forKey: "drawsBackground")
        wv.navigationDelegate = context.coordinator
        context.coordinator.webView = wv
        return wv
    }

    func updateNSView(_ wv: PassthroughWebView, context: Context) {
        context.coordinator.onHeight = { h in if abs(height - h) > 1 { height = max(h, 20) } }
        if context.coordinator.loaded != html {
            context.coordinator.loaded = html
            wv.loadHTMLString(WebConfig.page(html), baseURL: nil)
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
