/// JSON-RPC framing and protocol object decoding.
import Foundation
import Testing
@testable import MermozKit

@Test func lineBufferHoldsPartialLine() {
    var b = LineBuffer()
    #expect(b.append(Data("{\"a\":".utf8)).isEmpty)
}

@Test func lineBufferEmitsCompletedLine() {
    var b = LineBuffer()
    _ = b.append(Data("{\"a\":".utf8))
    #expect(b.append(Data("1}\n".utf8)) == [Data("{\"a\":1}".utf8)])
}

@Test func lineBufferSplitsSeveralLinesInOneChunk() {
    var b = LineBuffer()
    #expect(b.append(Data("x\ny\nz".utf8)).count == 2)
}

@Test func requestIsNewlineTerminated() {
    let line = encodeRequest(id: 7, method: "hello", params: Data("{}".utf8))
    #expect(line.last == 0x0A)
}

@Test func requestCarriesMethodAndId() throws {
    let line = encodeRequest(id: 7, method: "threads.get", params: Data("{\"id\":\"M-1\"}".utf8))
    let obj = try JSONSerialization.jsonObject(with: line) as? [String: Any]
    #expect(obj?["method"] as? String == "threads.get" && obj?["id"] as? Int == 7)
}

@Test func errorResponseDecodesApprovalRequired() {
    let line = Data(#"{"jsonrpc":"2.0","id":3,"error":{"code":-32010,"message":"approval_required","data":{"draft":"dft_1","thread":"M-2"}}}"#.utf8)
    guard case let .response(_, .failure(err)) = decodeIncoming(line) else {
        Issue.record("expected error response")
        return
    }
    #expect(err.code == RPCError.approvalRequired)
}

@Test func resultResponseKeepsRawJSON() throws {
    let line = Data(#"{"jsonrpc":"2.0","id":1,"result":{"version":"0.1.0"}}"#.utf8)
    guard case let .response(id, .success(data)) = decodeIncoming(line) else {
        Issue.record("expected result")
        return
    }
    let obj = try JSONSerialization.jsonObject(with: data) as? [String: String]
    #expect(id == 1 && obj?["version"] == "0.1.0")
}

@Test(arguments: [
    (#"{"jsonrpc":"2.0","method":"event","params":{"kind":"threads_changed","ids":["t1"]}}"#, DaemonEvent.threadsChanged(ids: ["t1"])),
    (#"{"jsonrpc":"2.0","method":"event","params":{"kind":"accounts_changed"}}"#, DaemonEvent.accountsChanged),
    (#"{"jsonrpc":"2.0","method":"event","params":{"kind":"sync_progress","account_id":"acc_1","synced":5,"total":9}}"#,
     DaemonEvent.syncProgress(accountId: "acc_1", synced: 5, total: 9)),
    (#"{"jsonrpc":"2.0","method":"event","params":{"kind":"drafts_changed"}}"#, DaemonEvent.draftsChanged),
])
func eventsDecode(_ raw: String, _ expected: DaemonEvent) {
    guard case let .event(e) = decodeIncoming(Data(raw.utf8)) else {
        Issue.record("expected event")
        return
    }
    #expect(e == expected)
}

@Test func threadSummaryDecodesSnakeCase() throws {
    let json = Data(#"""
    {"id":"thr_1","number":"M-1287","account_id":"acc_1","subject":"Audit","snippet":"s","participants":["Marc"],
     "last_from":"Marc","last_date":1791700000000,"unread":true,"message_count":3,"has_attachments":true,
     "status":"todo","priority":2,"project_id":null,"due":"2026-10-16","summary":null,"next_action":null}
    """#.utf8)
    let t = try RPCCoding.decoder().decode(ThreadSummary.self, from: json)
    #expect(t.accountId == "acc_1" && t.lastDate == 1791700000000 && t.due == "2026-10-16")
}

@Test func paramsEncodeExplicitNull() throws {
    let data = Params().with("ids", ["t1"]).null("project_id").data()
    let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any]
    #expect(obj?["project_id"] is NSNull)
}

@Test func paramsOmitNilValues() throws {
    let data = Params().with("status", Optional<String>.none).data()
    let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any]
    #expect(obj?.isEmpty == true)
}
