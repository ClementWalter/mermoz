/// POSIX Unix-socket connection to mermozd with id-matched async request/response.
import Foundation
import os

private let log = Logger(subsystem: "ai.mermoz.app", category: "rpc")

/// One socket. A background thread reads lines and resumes the matching continuation,
/// so callers never block a Swift concurrency thread on I/O.
public final class Connection: @unchecked Sendable {
    private let fd: Int32
    private let lock = NSLock()
    private var nextId = 1
    private var pending: [Int: CheckedContinuation<Data, Error>] = [:]
    private var closed = false
    private let onEvent: (@Sendable (DaemonEvent) -> Void)?
    private let onClose: (@Sendable () -> Void)?

    /// Connects to the socket at `path`; throws a transport error if nothing listens there.
    public init(path: String, onEvent: (@Sendable (DaemonEvent) -> Void)? = nil, onClose: (@Sendable () -> Void)? = nil) throws {
        // Open before initializing self: a throwing init still runs deinit, which would close the fd twice.
        fd = try Connection.openSocket(path)
        self.onEvent = onEvent
        self.onClose = onClose
        let t = Thread { [weak self] in self?.readLoop() }
        t.name = "mermozd-reader"
        t.start()
    }

    private static func openSocket(_ path: String) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw RPCError(code: RPCError.transport, message: "socket() failed") }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: addr.sun_path) else {
            Darwin.close(fd)
            throw RPCError(code: RPCError.transport, message: "socket path too long")
        }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let rc = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard rc == 0 else {
            let reason = String(cString: strerror(errno))
            Darwin.close(fd)
            throw RPCError(code: RPCError.transport, message: "mermozd is not running (\(reason))")
        }
        // Writes to a socket the daemon closed must fail with EPIPE, not kill the app.
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        return fd
    }

    deinit { shutdownSocket() }

    /// Sends a request and awaits its raw JSON result.
    public func call(_ method: String, params: Data) async throws -> Data {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            lock.lock()
            if closed {
                lock.unlock()
                cont.resume(throwing: RPCError(code: RPCError.transport, message: "connection closed"))
                return
            }
            let id = nextId
            nextId += 1
            pending[id] = cont
            let line = encodeRequest(id: id, method: method, params: params)
            let ok = writeAll(line)
            if !ok { pending.removeValue(forKey: id) }
            lock.unlock()
            if !ok { cont.resume(throwing: RPCError(code: RPCError.transport, message: "write failed")) }
        }
    }

    public func close() { shutdownSocket() }

    private func writeAll(_ data: Data) -> Bool {
        data.withUnsafeBytes { raw -> Bool in
            var off = 0
            while off < raw.count {
                let n = Darwin.write(fd, raw.baseAddress! + off, raw.count - off)
                if n <= 0 {
                    if n < 0 && errno == EINTR { continue }
                    return false
                }
                off += n
            }
            return true
        }
    }

    private func readLoop() {
        var buf = LineBuffer()
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let n = chunk.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress!, $0.count) }
            if n < 0 && errno == EINTR { continue }
            if n <= 0 { break }
            for line in buf.append(Data(chunk[0..<n])) { dispatch(line) }
        }
        failAll()
    }

    private func dispatch(_ line: Data) {
        switch decodeIncoming(line) {
        case let .response(id, result):
            lock.lock()
            let cont = pending.removeValue(forKey: id)
            lock.unlock()
            switch result {
            case let .success(d): cont?.resume(returning: d)
            case let .failure(e): cont?.resume(throwing: e)
            }
        case let .event(e):
            onEvent?(e)
        case .malformed:
            log.error("malformed line from daemon")
        }
    }

    private func failAll() {
        lock.lock()
        let wasClosed = closed
        closed = true
        let conts = pending
        pending.removeAll()
        lock.unlock()
        for c in conts.values { c.resume(throwing: RPCError(code: RPCError.transport, message: "connection closed")) }
        if !wasClosed { onClose?() }
    }

    private func shutdownSocket() {
        lock.lock()
        let already = closed
        closed = true
        lock.unlock()
        if !already {
            Darwin.shutdown(fd, SHUT_RDWR)
            Darwin.close(fd)
        }
    }
}
