/// "Chat about this" payloads decode as mermozd sends them, and each proposal maps to one action.
import Foundation
import Testing
@testable import MermozKit

private let answerJSON = #"{"id":"cht_1","thread_id":"thr_1","role":"assistant","text":"Reply today.","actions":[{"kind":"status","label":"Move to Waiting","status":"waiting"},{"kind":"draft_reply","label":"Draft a reply","body":"Hi Marc"}],"searched":true,"research_unavailable":false,"created":1791700000000}"#

private func decode<T: Decodable>(_ s: String) throws -> T {
    try RPCCoding.decoder().decode(T.self, from: Data(s.utf8))
}

@Test func chatAnswerDecodesItsProposals() throws {
    let m: ChatMessage = try decode(answerJSON)
    #expect(m.actions.map(\.kind) == ["status", "draft_reply"])
}

@Test func chatAnswerDecodesSnakeCaseThreadId() throws {
    let m: ChatMessage = try decode(answerJSON)
    #expect(m.threadId == "thr_1")
}

@Test func chatAnswerIsFromTheAssistant() throws {
    let m: ChatMessage = try decode(answerJSON)
    #expect(m.isAssistant)
}

@Test func chatHistoryDecodesResearchAvailability() throws {
    let h: ChatHistory = try decode(#"{"messages":[],"research_available":true}"#)
    #expect(h.researchAvailable)
}

@Test(arguments: [
    (ChatAction(kind: "draft_reply", label: "Draft", body: "Hi"), ChatAction.Plan.draftReply("Hi")),
    (ChatAction(kind: "status", label: "Wait", status: "waiting"), .status("waiting")),
    (ChatAction(kind: "project", label: "File", project: "Finance"), .addProject("Finance")),
    (ChatAction(kind: "follow_up", label: "Remind", due: "2026-10-20"), .followUp("2026-10-20")),
    (ChatAction(kind: "status", label: "Broken"), .none),
    (ChatAction(kind: "send_now", label: "Send"), .none),
])
func chatActionPlans(action: ChatAction, plan: ChatAction.Plan) {
    #expect(action.plan == plan)
}

@Test func chatDeltaEventDecodes() {
    let line = Data(#"{"jsonrpc":"2.0","method":"event","params":{"kind":"chat_delta","thread_id":"thr_1","text":"Marc wants"}}"#.utf8)
    guard case let .event(e) = decodeIncoming(line) else { Issue.record("not an event"); return }
    #expect(e == .chatDelta(threadId: "thr_1", text: "Marc wants"))
}
