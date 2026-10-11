/// Formatted-message renderer: one shared, pre-warmed WKWebView with scripts and remote loads off.
import AppKit
import SwiftUI
import WebKit

/// Owns the single web view; creating WKWebViews is slow, so it is built once and reused.
@MainActor
final class SharedWebView: NSObject, WKNavigationDelegate {
    static let shared = SharedWebView()
    let webView: WKWebView
    var onHeight: ((CGFloat) -> Void)?

    private override init() {
        let config = WKWebViewConfiguration()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        config.websiteDataStore = .nonPersistent()
        webView = WKWebView(frame: .zero, configuration: config)
        super.init()
        webView.navigationDelegate = self
        webView.setValue(false, forKey: "drawsBackground")
        webView.loadHTMLString("<html></html>", baseURL: nil)
    }

    /// CSP blocks every remote fetch (trackers, images) — only inline styles and data: images render.
    func load(_ html: String) {
        let csp = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'; font-src data:\">"
        let base = "<style>body{font:14px -apple-system,system-ui;color:#1A3346;margin:0;word-wrap:break-word}img{max-width:100%;height:auto}</style>"
        webView.loadHTMLString("<!doctype html><html><head><meta charset=\"utf-8\">\(csp)\(base)</head><body>\(html)</body></html>", baseURL: nil)
    }

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        // Clicked links open in the user's browser, never inside the message pane.
        if action.navigationType == .linkActivated, let url = action.request.url {
            NSWorkspace.shared.open(url)
            return .cancel
        }
        return .allow
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // App-evaluated JS still runs with page scripts disabled; used only to size the view.
        webView.evaluateJavaScript("document.documentElement.scrollHeight") { [weak self] v, _ in
            if let h = v as? CGFloat { self?.onHeight?(h) }
        }
    }
}

struct HTMLMessageView: NSViewRepresentable {
    let html: String
    @Binding var height: CGFloat

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        attach(to: container)
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        attach(to: container)
        if context.coordinator.loaded != html {
            context.coordinator.loaded = html
            SharedWebView.shared.onHeight = { h in height = max(h, 40) }
            SharedWebView.shared.load(html)
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator { var loaded: String? }

    private func attach(to container: NSView) {
        let wv = SharedWebView.shared.webView
        guard wv.superview !== container else { return }
        wv.removeFromSuperview()
        wv.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(wv)
        NSLayoutConstraint.activate([
            wv.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            wv.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            wv.topAnchor.constraint(equalTo: container.topAnchor),
            wv.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])
    }
}
