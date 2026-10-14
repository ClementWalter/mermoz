/// In-process timing of keyboard stepping, for profiling on a copied mailbox.
///
/// With `MERMOZ_PERF_STEPS=n` the app never shows a window, activates or takes focus
/// (see PerfStealth); its windows lay out and draw off screen. It waits for the list,
/// opens `MERMOZ_PERF_VIEW` (a status key) when set, then steps n times as `j` does. Each step
/// is charged the main thread's CPU time until the next step, minus the median of idle
/// windows of the same length. Results go to the log and, with `MERMOZ_PERF_OUT`, to a
/// JSON file; the app then quits.
import AppKit
import QuartzCore
import Darwin
import Foundation
import MermozKit
import os
import SwiftUI

private let log = Logger(subsystem: "ai.mermoz.app", category: "perf")

@MainActor
enum PerfHarness {
    static var steps: Int? {
        ProcessInfo.processInfo.environment["MERMOZ_PERF_STEPS"].flatMap(Int.init)
    }

    static var enabled: Bool { steps != nil }

    /// The model whose `detail` the windows watch.
    private static var watched: AppModel?

    /// CPU time (user + system) the calling thread has used, in milliseconds.
    static func threadCPU() -> Double {
        var info = thread_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<thread_basic_info>.size / MemoryLayout<integer_t>.size)
        let port = mach_thread_self()
        defer { mach_port_deallocate(mach_task_self_, port) }
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { thread_info(port, thread_flavor_t(THREAD_BASIC_INFO), $0, &count) }
        }
        guard kr == KERN_SUCCESS else { return 0 }
        let u = Double(info.user_time.seconds) * 1000 + Double(info.user_time.microseconds) / 1000
        let s = Double(info.system_time.seconds) * 1000 + Double(info.system_time.microseconds) / 1000
        return u + s
    }

    private typealias SelfCounts = @convention(c) (Int32, UnsafeMutableRawPointer, Int) -> Int32
    private static let selfCounts: SelfCounts? = dlsym(dlopen(nil, RTLD_NOW), "thread_selfcounts")
        .map { unsafeBitCast($0, to: SelfCounts.self) }

    /// Instructions the calling thread has retired, in millions: the same work counts the
    /// same on an efficiency or a performance core, unlike CPU time.
    static func threadInstructions() -> Double {
        guard let selfCounts else { return 0 }
        var counts: (UInt64, UInt64) = (0, 0)
        guard withUnsafeMutableBytes(of: &counts, { selfCounts(1, $0.baseAddress!, 16) }) == 0 else { return 0 }
        return Double(counts.0) / 1e6
    }

    /// The main window: never on screen, so found by size rather than visibility.
    static func mainWindow() -> NSWindow? {
        NSApp.windows.first { $0.contentView != nil && $0.frame.height > 300 }
    }

    /// Lays out and draws every window now; none is ever on screen.
    private static func flush() {
        for w in NSApp.windows where w.contentView != nil {
            w.contentView?.layoutSubtreeIfNeeded()
            w.displayIfNeeded()
        }
    }

    /// Main-thread CPU spent over one window: `action` and the frame drawn right after it
    /// (`first`), then everything until `gap` has passed (`total`, the frame included).
    /// Main-thread CPU at the moment the opened conversation reached the pane, in a window.
    private static var detailAt: Double?

    /// Wall time from the step until the pane held the selected thread's messages.
    private static var paneReady: Double?

    private static func window(gap: Duration, _ action: () -> Void) async -> (action: Double, first: Double, total: Double, minstr: Double) {
        let t0 = threadCPU()
        let i0 = threadInstructions()
        detailAt = nil
        paneReady = nil
        if let model = watched {
            withObservationTracking { _ = model.detail } onChange: { MainActor.assumeIsolated { detailAt = threadCPU() - t0 } }
        }
        let wall = ContinuousClock.now
        action()
        let acted = threadCPU() - t0
        flush()
        let first = threadCPU() - t0
        // Polled every millisecond until the pane shows what was stepped to (the focused
        // Feed card's mail, else the selection), then the rest of the gap.
        if let model = watched {
            let target = model.feedFocus.flatMap { model.feed?.card($0) }.map(FeedSelection.thread) ?? model.selectedThreadId
            while ContinuousClock.now - wall < gap {
                if let d = model.detail, d.thread.id == target || d.messages.contains(where: { $0.threadId == target }),
                   !d.messages.isEmpty {
                    let e = ContinuousClock.now - wall
                    paneReady = Double(e.components.seconds) * 1000 + Double(e.components.attoseconds) / 1e15
                    break
                }
                try? await Task.sleep(for: .milliseconds(1))
            }
        }
        let left = gap - (ContinuousClock.now - wall)
        if left > .zero { try? await Task.sleep(for: left) }
        flush()
        return (acted, first, threadCPU() - t0, threadInstructions() - i0)
    }

    /// Renders the main window's layer tree (what SwiftUI drew) to a PNG, without capturing
    /// the screen: works while another app is in front or the screen is locked.
    private static func snapshotWindow(to path: String) {
        guard let view = mainWindow()?.contentView,
              let layer = view.layer else { return }
        let scale: CGFloat = 2
        let size = view.bounds.size
        guard let ctx = CGContext(data: nil, width: Int(size.width * scale), height: Int(size.height * scale),
                                  bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return }
        // Core Animation draws top-down; the bitmap context is bottom-up.
        ctx.translateBy(x: 0, y: size.height * scale)
        ctx.scaleBy(x: scale, y: -scale)
        layer.render(in: ctx)
        if let image = ctx.makeImage() {
            try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?
                .write(to: URL(fileURLWithPath: path))
        }
    }

    /// Whether the List's selected row is fully inside the list's visible area.
    private static func selectedRowShown() -> Bool {
        func find(_ v: NSView) -> PassiveTableView? {
            if let t = v as? PassiveTableView { return t }
            for s in v.subviews { if let t = find(s) { return t } }
            return nil
        }
        guard let root = mainWindow()?.contentView,
              let table = find(root), let c = table.delegate as? ThreadTable.Coordinator,
              let id = c.model.selectedThreadId, let row = ListItems.row(of: id, in: c.items),
              let clip = table.enclosingScrollView?.contentView else { return true }
        return clip.bounds.contains(table.rect(ofRow: row))
    }

    private static func pct(_ v: [Double], _ p: Double) -> Double {
        let s = v.sorted()
        return s.isEmpty ? 0 : s[Int(Double(s.count - 1) * p)]
    }

    /// `MERMOZ_PERF_SCENARIO`: what one step does. `jk` (default) steps down the List,
    /// `board` down a Board column, `feed` to the next Feed card, `palette` opens or closes
    /// ⌘K, `resize` widens or narrows the window; more in PerfScenarios.
    private static func scenario(_ model: AppModel) async -> () -> Void {
        switch ProcessInfo.processInfo.environment["MERMOZ_PERF_SCENARIO"] ?? "jk" {
        case "board":
            model.layout = .board
            await model.loadList()
            return { model.moveBoard(dx: 0, dy: 1) }
        case "feed":
            model.layout = .feed
            await model.loadList()
            return { model.feedMove(1) }
        case "palette":
            return { model.paletteMode = model.paletteMode == nil ? .search : nil }
        case "highlight":
            // The selection highlight alone moves (no thread opens): the cheapest visible
            // change, the window's per-frame floor.
            let ids = model.displayOrder.map(\.id)
            var i = 0
            return {
                i += 1
                model.selectionCells.select([ids[i % max(ids.count, 1)]])
            }
        case "idle":
            return {}
        case "resize":
            var wider = true
            return {
                guard let w = mainWindow() else { return }
                var f = w.frame
                f.size.width += wider ? 60 : -60
                wider.toggle()
                w.setFrame(f, display: true)
            }
        case let name:
            if let s = await PerfScenarios.step(name, model) { return s }
            model.layout = .list
            return { model.moveSelection(1) }
        }
    }

    static func run(_ model: AppModel) async {
        guard let n = steps else { return }
        let activity = ProcessInfo.processInfo.beginActivity(options: [.userInitiated, .latencyCritical],
                                                             reason: "perf harness")
        defer { ProcessInfo.processInfo.endActivity(activity) }
        let gapMs = ProcessInfo.processInfo.environment["MERMOZ_PERF_GAP_MS"].flatMap(Int.init) ?? 400
        let gap = Duration.milliseconds(gapMs)
        // A fixed size off every display, whatever frame a former run saved.
        mainWindow()?.setFrame(CGRect(x: -20_000, y: -20_000, width: 1440, height: 900), display: false)
        if PerfStealth.onScreenWindows() > 0 {
            log.error("perf: a window is on screen; stopping")
            NSApp.terminate(nil)
            return
        }
        for _ in 0..<100 where model.threads.isEmpty { try? await Task.sleep(for: .milliseconds(100)) }
        if let v = ProcessInfo.processInfo.environment["MERMOZ_PERF_VIEW"] {
            model.select(.status(v))
            try? await Task.sleep(for: .milliseconds(300))
        }
        let step = await scenario(model)
        try? await Task.sleep(for: .seconds(2))
        var idle: [Double] = []
        var idleInstr: [Double] = []
        for _ in 0..<10 {
            let w = await window(gap: gap) {}
            idle.append(w.total)
            idleInstr.append(w.minstr)
        }
        let base = pct(idle, 0.5)
        let baseInstr = pct(idleInstr, 0.5)
        var instrs: [Double] = []
        var costs: [Double] = []
        var firsts: [Double] = []
        var actions: [Double] = []
        var beforeDetail: [Double] = []
        var ready: [Double] = []
        // List steps that ended with the selected row not fully shown.
        var hidden = 0
        watched = model
        for _ in 0..<n {
            let c = await window(gap: gap, step)
            costs.append(max(0, c.total - base))
            instrs.append(max(0, c.minstr - baseInstr))
            firsts.append(c.first)
            actions.append(c.action)
            if let d = detailAt { beforeDetail.append(d) }
            if let r = paneReady { ready.append(r) }
            if model.layout == .list, !selectedRowShown() { hidden += 1 }
        }
        let result: [String: Double] = [
            "steps": Double(n), "gap_ms": Double(gapMs), "idle_ms": base,
            "before_detail_median_ms": pct(beforeDetail, 0.5),
            "pane_ready_median_ms": pct(ready, 0.5), "pane_ready_p95_ms": pct(ready, 0.95), "pane_ready_count": Double(ready.count),
            "median_ms": pct(costs, 0.5), "p95_ms": pct(costs, 0.95), "mean_ms": costs.reduce(0, +) / Double(max(n, 1)),
            "minstr_median": pct(instrs, 0.5), "minstr_p95": pct(instrs, 0.95),
            "first_median_ms": pct(firsts, 0.5), "first_p95_ms": pct(firsts, 0.95), "action_median_ms": pct(actions, 0.5),
            "bubble_size_hits": Double(BubbleTextCache.shared.sizeHits), "bubble_size_misses": Double(BubbleTextCache.shared.sizeMisses),
            "bubble_main_misses": Double(BubbleTextCache.shared.mainMisses),
            "visible_cards": Double(model.feedFullyVisible.count), "selection_hidden_steps": Double(hidden),
            "onscreen_windows": Double(PerfStealth.onScreenWindows()),
        ]
        log.info("perf: \(result.description, privacy: .public)")
        if let out = ProcessInfo.processInfo.environment["MERMOZ_PERF_OUT"],
           let data = try? JSONSerialization.data(withJSONObject: ["summary": result, "steps_ms": costs, "first_ms": firsts]) {
            try? data.write(to: URL(fileURLWithPath: out))
        }
        // `MERMOZ_PERF_OPEN` + `MERMOZ_PERF_SHOT`: open one thread and save the window.
        if let id = ProcessInfo.processInfo.environment["MERMOZ_PERF_OPEN"],
           let shot = ProcessInfo.processInfo.environment["MERMOZ_PERF_SHOT"] {
            model.open(id)
            try? await Task.sleep(for: .seconds(2))
            flush()
            snapshotWindow(to: shot)
        }
        // `MERMOZ_PERF_HEADER_SHOTS`: a folder to save the window into at several List widths.
        if let dir = ProcessInfo.processInfo.environment["MERMOZ_PERF_HEADER_SHOTS"] {
            for width in [420, 360, 300] {
                UserDefaults.standard.set(Double(width), forKey: "layout.listWidth")
                try? await Task.sleep(for: .milliseconds(800))
                flush()
                snapshotWindow(to: "\(dir)/list-\(width).png")
            }
        }
        // `MERMOZ_PERF_HOLD_S`: stay up that long after the run, to capture the window.
        if let hold = ProcessInfo.processInfo.environment["MERMOZ_PERF_HOLD_S"].flatMap(Double.init) {
            try? await Task.sleep(for: .seconds(hold))
        }
        NSApp.terminate(nil)
    }
}
