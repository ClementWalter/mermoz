/// Gmail's own search results, as the fallback of an empty local search lists them.
import Foundation
import Testing
@testable import MermozKit

private let answer = """
{"results": [{"account_id": "a1", "id": "m9", "thread_id": "t9", "from": "Frenchbee <noreply@frenchbee.example>",
  "subject": "Your flight to Papeete", "date": "Tue, 13 Oct 2026 09:00:00 +0000", "in_mermoz": false}]}
"""

private func hit() throws -> GmailHit {
    try RPCCoding.decoder().decode(GmailSearch.self, from: Data(answer.utf8)).results[0]
}

@Test func aGmailHitKeepsItsSubject() throws {
    #expect(try hit().subject == "Your flight to Papeete")
}

@Test func aGmailHitSaysWhetherMermozHasIt() throws {
    #expect(try hit().inMermoz == false)
}
