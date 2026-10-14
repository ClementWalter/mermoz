/// More PerfHarness steps (`MERMOZ_PERF_SCENARIO`): `views` clicks between statuses in
/// the sidebar, `layouts` cycles List, Board and Feed, `type` types into ⌘K one key
/// per step, `feedscroll` scrolls the Feed 240 pt.
import AppKit
import MermozKit
import SwiftUI

@MainActor
enum PerfScenarios {
    static func find<T: NSView>(_ type: T.Type, in v: NSView, where ok: (T) -> Bool = { _ in true }) -> T? {
        if let t = v as? T, ok(t) { return t }
        for s in v.subviews { if let t = find(type, in: s, where: ok) { return t } }
        return nil
    }

    /// The step for `name`; nil for a scenario PerfHarness runs itself.
    static func step(_ name: String, _ model: AppModel) async -> (() -> Void)? {
        switch name {
        case "views":
            let keys = ["todo", "backlog", "waiting", "done"]
            var i = 0
            return {
                i += 1
                model.select(.status(keys[i % keys.count]))
            }
        case "layouts":
            let modes: [LayoutMode] = [.list, .board, .feed]
            var i = 0
            return {
                i += 1
                model.layout = modes[i % modes.count]
                Task { await model.loadList() }
            }
        case "type":
            model.paletteMode = .search
            try? await Task.sleep(for: .milliseconds(500))
            let word = Array("archive older newsletters ")
            var i = 0
            return {
                guard let w = PerfHarness.mainWindow(), let root = w.contentView else { return }
                if !(w.firstResponder is NSTextView),
                   let f = find(NSTextField.self, in: root, where: { $0.isEditable }) {
                    w.makeFirstResponder(f)
                }
                guard let tv = w.firstResponder as? NSTextView else { return }
                // The word starts over once typed, so the query stays short.
                if i % word.count == 0 { tv.selectAll(nil); tv.deleteBackward(nil) }
                tv.insertText(String(word[i % word.count]), replacementRange: tv.selectedRange())
                i += 1
            }
        case "feedscroll":
            model.layout = .feed
            await model.loadList()
            try? await Task.sleep(for: .milliseconds(500))
            var down = true
            var n = 0
            return {
                guard let root = PerfHarness.mainWindow()?.contentView,
                      let sv = find(NSScrollView.self, in: root, where: { ($0.documentView?.frame.height ?? 0) > $0.frame.height * 2 })
                else { return }
                let clip = sv.contentView
                var o = clip.bounds.origin
                o.y += down ? 240 : -240
                n += 1
                if n % 8 == 0 { down.toggle() }
                clip.scroll(to: o)
                sv.reflectScrolledClipView(clip)
            }
        default:
            return nil
        }
    }
}
