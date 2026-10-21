/// A draft in its conversation reads as a draft, with its own words only.
import Foundation
import Testing
@testable import MermozKit

private func draft(origin: String) -> Draft {
    Draft(id: "d", accountId: "a", threadId: "t", to: [], cc: [], subject: "", body: "", state: "draft",
          origin: origin, created: 0, updated: 0, attachments: [])
}

@Test func gmailDraftSaysWhereItWasWritten() {
    #expect(DraftPreview.caption(draft(origin: "gmail")) == "Draft from Gmail")
}

@Test func ownDraftIsJustADraft() {
    #expect(DraftPreview.caption(draft(origin: "app")) == "Draft")
}

@Test(arguments: [
    ("Bonjour,\n\nVoici les pièces.\n\nLe lun. 20 oct. 2026, DGFiP a écrit :\n> Merci", "Bonjour,\n\nVoici les pièces."),
    ("Signed.\n\nOn Mon, Oct 20, 2026 at 9:00 Marc wrote:\n> Please sign", "Signed."),
    ("Thanks\n> quoted", "Thanks"),
    ("Just this", "Just this"),
])
func draftPreviewKeepsItsOwnWords(body: String, expected: String) {
    #expect(DraftPreview.text(body) == expected)
}

@Test func threadDetailDecodesItsDrafts() throws {
    let s = #"{"thread":{"id":"t","number":"M-1","account_id":"a","subject":"s","snippet":"","participants":[],"last_from":"","last_date":0,"unread":false,"message_count":0,"has_attachments":false,"status":"todo","priority":0,"projects":[]},"messages":[],"drafts":[{"id":"d","account_id":"a","thread_id":"t","to":[],"cc":[],"subject":"","body":"Hi","state":"draft","origin":"gmail","created":0,"updated":0,"attachments":[]}]}"#
    #expect(try RPCCoding.decoder().decode(ThreadDetail.self, from: Data(s.utf8)).drafts?.first?.origin == "gmail")
}
