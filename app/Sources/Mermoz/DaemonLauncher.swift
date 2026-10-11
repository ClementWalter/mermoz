/// Finds or starts mermozd, then connects to its socket.
import Foundation
import MermozKit
import os

private let log = Logger(subsystem: "ai.mermoz.app", category: "launcher")

enum DaemonLauncher {
    /// Connects, spawning the bundled daemon if nothing listens yet; retries for ~3 s while it boots.
    static func connect() async throws -> Connection {
        if let c = try? Connection(path: DaemonPaths.socket) { return c }
        try spawn()
        var delay: UInt64 = 50
        var waited: UInt64 = 0
        while waited < 3000 {
            try await Task.sleep(nanoseconds: delay * 1_000_000)
            waited += delay
            if let c = try? Connection(path: DaemonPaths.socket) { return c }
            delay = min(delay * 2, 400)
        }
        throw RPCError(code: RPCError.transport, message: "Mermoz couldn't start its background service.")
    }

    static func daemonURL() -> URL? {
        if let p = ProcessInfo.processInfo.environment["MERMOZ_DAEMON"], FileManager.default.isExecutableFile(atPath: p) {
            return URL(fileURLWithPath: p)
        }
        if let r = Bundle.main.resourceURL?.appendingPathComponent("mermozd"),
           FileManager.default.isExecutableFile(atPath: r.path) {
            return r
        }
        return nil
    }

    private static func spawn() throws {
        guard let url = daemonURL() else {
            throw RPCError(code: RPCError.transport, message: "Mermoz's background service is missing from the app.")
        }
        try FileManager.default.createDirectory(at: DaemonPaths.home, withIntermediateDirectories: true)
        let p = Process()
        p.executableURL = url
        p.environment = ProcessInfo.processInfo.environment
        // Detached stdio so the daemon outlives the app; its log goes to mermozd.log for support.
        p.standardInput = FileHandle.nullDevice
        p.standardOutput = FileHandle.nullDevice
        let logURL = DaemonPaths.home.appendingPathComponent("mermozd.log")
        if !FileManager.default.fileExists(atPath: logURL.path) {
            FileManager.default.createFile(atPath: logURL.path, contents: nil)
        }
        if let h = try? FileHandle(forWritingTo: logURL) {
            h.seekToEndOfFile()
            p.standardError = h
        } else {
            p.standardError = FileHandle.nullDevice
        }
        try p.run()
        log.info("spawned mermozd pid \(p.processIdentifier)")
    }
}
