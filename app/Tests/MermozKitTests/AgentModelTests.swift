/// Directory and agent payloads decode exactly as mermozd sends them.
import Foundation
import Testing
@testable import MermozKit

private let contactJSON = #"{"email":"marc@acme.example","name":"Marc Petit","title":"CFO","company":"Acme","phones":["+33 6"],"address":"","website":"","socials":[],"last_seen":1791700000000}"#

private func decode<T: Decodable>(_ s: String) throws -> T {
    try RPCCoding.decoder().decode(T.self, from: Data(s.utf8))
}

@Test func contactDecodesSnakeCase() throws {
    let c: Contact = try decode(contactJSON)
    #expect(c.lastSeen == 1_791_700_000_000)
}

@Test func contactRoleJoinsTitleAndCompany() throws {
    let c: Contact = try decode(contactJSON)
    #expect(c.role == "CFO · Acme")
}

@Test func agentStateDecodes() throws {
    let s: AgentState = try decode(#"{"provider_ids":["*"],"digest_enabled":true,"done":3,"pending":1,"failed":0,"error":null}"#)
    #expect(s.providerIds == ["*"])
}

@Test func messageWithoutCleanBodyStillDecodes() throws {
    let m: Message = try decode(#"{"id":"m","from":{"name":"","email":"a@b.c"},"to":[],"cc":[],"date":0,"subject":"","body_text":"hi","has_html":false,"is_from_me":false,"attachments":[]}"#)
    #expect(m.bodyClean == nil)
}
