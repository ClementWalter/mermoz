/// Connection round-trips against a stub Unix-socket server speaking the daemon framing.
import Foundation
import Testing
@testable import MermozKit

/// Minimal one-client server: answers each request line with `reply(id)` and can push an event.
final class StubDaemon: @unchecked Sendable {
    let path: String
    private let listenFd: Int32
    private let reply: @Sendable (Int, String) -> String

    init(reply: @escaping @Sendable (Int, String) -> String) throws {
        self.reply = reply
        path = NSTemporaryDirectory() + "mz-\(UUID().uuidString.prefix(8)).sock"
        listenFd = socket(AF_UNIX, SOCK_STREAM, 0)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        _ = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(listenFd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        listen(listenFd, 4)
        let fd = listenFd
        Thread { [reply] in
            let c = accept(fd, nil, nil)
            var buf = LineBuffer()
            var chunk = [UInt8](repeating: 0, count: 4096)
            while true {
                let n = chunk.withUnsafeMutableBytes { read(c, $0.baseAddress!, $0.count) }
                if n <= 0 { break }
                for line in buf.append(Data(chunk[0..<n])) {
                    let obj = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any] ?? [:]
                    let out = reply(obj["id"] as? Int ?? 0, obj["method"] as? String ?? "") + "\n"
                    _ = out.withCString { write(c, $0, strlen($0)) }
                }
            }
            close(c)
        }.start()
    }

    deinit {
        close(listenFd)
        unlink(path)
    }
}

@Test func callReturnsResult() async throws {
    let stub = try StubDaemon { id, _ in #"{"jsonrpc":"2.0","id":\#(id),"result":{"version":"0.1.0"}}"# }
    defer { withExtendedLifetime(stub) {} }
    let client = DaemonClient(connection: try Connection(path: stub.path))
    #expect(try await client.hello() == "0.1.0")
}

@Test func callSurfacesDaemonError() async throws {
    let stub = try StubDaemon { id, _ in #"{"jsonrpc":"2.0","id":\#(id),"error":{"code":-32010,"message":"approval_required"}}"# }
    defer { withExtendedLifetime(stub) {} }
    let client = DaemonClient(connection: try Connection(path: stub.path))
    await #expect(throws: RPCError(code: RPCError.approvalRequired, message: "approval_required")) {
        _ = try await client.sendDraft("dft_1")
    }
}

@Test func eventBeforeResponseIsDelivered() async throws {
    let stub = try StubDaemon { id, _ in
        #"{"jsonrpc":"2.0","method":"event","params":{"kind":"accounts_changed"}}"# + "\n" + #"{"jsonrpc":"2.0","id":\#(id),"result":{}}"#
    }
    defer { withExtendedLifetime(stub) {} }
    let got = EventBox()
    let conn = try Connection(path: stub.path, onEvent: { got.set($0) })
    try await DaemonClient(connection: conn).subscribe()
    #expect(got.value == .accountsChanged)
}

@Test func connectToMissingSocketThrows() {
    #expect(throws: RPCError.self) { _ = try Connection(path: NSTemporaryDirectory() + "nope-\(UUID()).sock") }
}

final class EventBox: @unchecked Sendable {
    private let lock = NSLock()
    private var e: DaemonEvent?
    func set(_ v: DaemonEvent) { lock.lock(); e = v; lock.unlock() }
    var value: DaemonEvent? { lock.lock(); defer { lock.unlock() }; return e }
}
