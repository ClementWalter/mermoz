/// While Google refuses an account's sign-in, the main window says so across its top with
/// a "Sign in again" button; with every account signed in, nothing is shown.
import AppKit
@testable import MermozKit
import SwiftUI
import Testing
@testable import Mermoz

private func account(_ id: String, _ email: String, expired: Bool) -> Account {
    Account(id: id, email: email, provider: "gmail", sync: SyncInfo(state: "idle", synced: 0, total: 0),
            needsReconnect: expired, signInExpired: expired)
}

/// The main window, offscreen, showing `accounts`.
@MainActor
private func window(_ accounts: [Account]) async -> NSHostingView<AnyView> {
    let model = AppModel()
    model.connection = .connected
    model.accounts = accounts
    model.layout = .list
    let host = NSHostingView(rootView: AnyView(RootView().environment(model)))
    let w = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 1_200, height: 800), styleMask: [.titled, .resizable],
                     backing: .buffered, defer: false)
    w.contentView = host
    host.layoutSubtreeIfNeeded()
    for _ in 0..<5 {
        try? await Task.sleep(for: .milliseconds(100))
        host.layoutSubtreeIfNeeded()
        host.display()
    }
    return host
}

/// How far down the window, in pixels, the sidebar's first drawn content starts: the
/// banner takes a strip above the panes.
@MainActor
private func sidebarTop(_ v: NSView) -> Int {
    let top = NSRect(x: 0, y: v.isFlipped ? 0 : v.bounds.height - 200, width: 300, height: 200)
    guard let rep = v.bitmapImageRepForCachingDisplay(in: top) else { return 0 }
    v.cacheDisplay(in: top, to: rep)
    for y in 0..<rep.pixelsHigh {
        for x in stride(from: rep.pixelsWide / 10, to: rep.pixelsWide, by: 2) {
            guard let c = rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
            if c.alphaComponent > 0.5, c.redComponent + c.greenComponent + c.blueComponent < 2.4 { return y }
        }
    }
    return rep.pixelsHigh
}

@MainActor @Test func refusedSignInMakesRoomForTheBannerAtTheTop() async {
    let signedIn = sidebarTop(await window([account("a", "me@example.com", expired: false)]))
    let refused = sidebarTop(await window([account("a", "me@example.com", expired: false),
                                           account("b", "you@gmail.com", expired: true)]))
    #expect(refused - signedIn > 40)
}

@MainActor @Test func theWindowNamesTheAccountToSignIn() {
    let model = AppModel()
    model.accounts = [account("a", "me@example.com", expired: false), account("b", "you@gmail.com", expired: true)]
    #expect(model.signInNotice?.accountId == "b")
}

@MainActor @Test func theSyncLineAsksToSignInAgain() {
    let model = AppModel()
    model.accounts = [account("b", "you@gmail.com", expired: true)]
    #expect(model.syncLine == "Sign in again to get new mail")
}
