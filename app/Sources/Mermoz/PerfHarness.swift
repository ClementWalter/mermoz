/// In-process timing of keyboard stepping, for profiling on a copied mailbox.
///
/// With `MERMOZ_PERF_STEPS=n` the app stays in the background (never activates, so it
/// takes no focus), waits for the list, then steps down it n times as `j` does. Each step
/// is charged the main thread's CPU time until the next step, minus the median of idle
/// windows of the same length. Results go to the log and, with `MERMOZ_PERF_OUT`, to a
/// JSON file; the app then quits.
import AppKit
import Darwin
import Foundation
import MermozKit
import os

private let log = Logger(subsystem: "ai.mermoz.app", category: "perf")

@MainActor
enum PerfHarness {
    static var steps: Int? {
        ProcessInfo.processInfo.environment["MERMOZ_PERF_STEPS"].flatMap(Int.init)
    }

    static var enabled: Bool { steps != nil }

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

    /// Lays out and draws every window now, even one another app covers.
    private static func flush() {
        for w in NSApp.windows where w.isVisible {
            w.contentView?.layoutSubtreeIfNeeded()
            w.displayIfNeeded()
        }
    }

    /// Main-thread CPU spent over one window: `action` and the frame drawn right after it
    /// (`first`), then everything until `gap` has passed (`total`, the frame included).
    private static func window(gap: Duration, _ action: () -> Void) async -> (action: Double, first: Double, total: Double) {
        let t0 = threadCPU()
        action()
        let acted = threadCPU() - t0
        flush()
        let first = threadCPU() - t0
        try? await Task.sleep(for: gap)
        flush()
        return (acted, first, threadCPU() - t0)
    }

    private static func pct(_ v: [Double], _ p: Double) -> Double {
        let s = v.sorted()
        return s.isEmpty ? 0 : s[Int(Double(s.count - 1) * p)]
    }

    static func run(_ model: AppModel) async {
        guard let n = steps else { return }
        let activity = ProcessInfo.processInfo.beginActivity(options: [.userInitiated, .latencyCritical],
                                                             reason: "perf harness")
        defer { ProcessInfo.processInfo.endActivity(activity) }
        let gapMs = ProcessInfo.processInfo.environment["MERMOZ_PERF_GAP_MS"].flatMap(Int.init) ?? 400
        let gap = Duration.milliseconds(gapMs)
        for _ in 0..<100 where model.threads.isEmpty { try? await Task.sleep(for: .milliseconds(100)) }
        try? await Task.sleep(for: .seconds(2))
        var idle: [Double] = []
        for _ in 0..<10 { idle.append(await window(gap: gap) {}.total) }
        let base = pct(idle, 0.5)
        var costs: [Double] = []
        var firsts: [Double] = []
        var actions: [Double] = []
        for _ in 0..<n {
            let c = await window(gap: gap) { model.moveSelection(1) }
            costs.append(max(0, c.total - base))
            firsts.append(c.first)
            actions.append(c.action)
        }
        let result: [String: Double] = [
            "steps": Double(n), "gap_ms": Double(gapMs), "idle_ms": base,
            "median_ms": pct(costs, 0.5), "p95_ms": pct(costs, 0.95), "mean_ms": costs.reduce(0, +) / Double(max(n, 1)),
            "first_median_ms": pct(firsts, 0.5), "first_p95_ms": pct(firsts, 0.95), "action_median_ms": pct(actions, 0.5),
            "bubble_size_hits": Double(BubbleTextCache.shared.sizeHits), "bubble_size_misses": Double(BubbleTextCache.shared.sizeMisses),
        ]
        log.info("perf: \(result.description, privacy: .public)")
        if let out = ProcessInfo.processInfo.environment["MERMOZ_PERF_OUT"],
           let data = try? JSONSerialization.data(withJSONObject: ["summary": result, "steps_ms": costs, "first_ms": firsts]) {
            try? data.write(to: URL(fileURLWithPath: out))
        }
        // `MERMOZ_PERF_HOLD_S`: stay up that long after the run, to capture the window.
        if let hold = ProcessInfo.processInfo.environment["MERMOZ_PERF_HOLD_S"].flatMap(Double.init) {
            try? await Task.sleep(for: .seconds(hold))
        }
        NSApp.terminate(nil)
    }
}
