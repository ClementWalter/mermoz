/// Keeps the perf harness off the user's screen: no Dock icon, no activation, no
/// restored windows, and no window ever ordered in. The windows still exist, lay out and
/// draw (the harness flushes them by hand); the window server never shows them.
import AppKit
import ObjectiveC

@MainActor
enum PerfStealth {
    /// Runs before any window exists (from the app's `init`).
    static func install() {
        NSApplication.shared.setActivationPolicy(.prohibited)
        UserDefaults.standard.register(defaults: ["ApplePersistenceIgnoreState": true, "NSQuitAlwaysKeepsWindows": false])
        NSWindow.allowsAutomaticWindowTabbing = false
        for name in ["orderWindow:relativeTo:", "orderFront:", "makeKeyAndOrderFront:", "orderFrontRegardless",
                     "makeKeyWindow", "makeMainWindow"] {
            swap(NSSelectorFromString(name))
        }
    }

    /// Replaces an ordering method of NSWindow with one that only keeps the window
    /// unrestorable, so nothing reaches the window server.
    private static func swap(_ sel: Selector) {
        guard let m = class_getInstanceMethod(NSWindow.self, sel) else { return }
        let block: @convention(block) (NSWindow) -> Void = { w in
            w.isRestorable = false
        }
        method_setImplementation(m, imp_implementationWithBlock(block))
    }

    /// The app's windows the window server shows, from its own window list.
    static func onScreenWindows() -> Int {
        let pid = ProcessInfo.processInfo.processIdentifier
        let all = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
        return all.filter {
            ($0[kCGWindowOwnerPID as String] as? Int32) == pid && ($0[kCGWindowIsOnscreen as String] as? Bool) == true
        }.count
    }
}
