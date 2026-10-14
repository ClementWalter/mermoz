/// No-reply addresses and the composer's warning: one verdict per case.
import Foundation
import Testing
@testable import MermozKit

private let franck = Address(name: "Franck ADAM", email: "franck@plomberie.example")

@Test(arguments: [
    "no-reply@invoices.example", "noreply@shop.example", "no_reply@bank.example", "DoNotReply@service.example",
    "do-not-reply@service.example", "ne-pas-repondre@impots.example", "ne-pas-répondre@ameli.example",
    "notifications@github.example", "MAILER-DAEMON@mx.example", "billing-noreply@saas.example",
])
func noReplyAddressIsFlagged(_ email: String) {
    #expect(NoReply.isNoReply(email))
}

@Test(arguments: ["franck@plomberie.example", "notification.team.lead@acme.example", "reply@acme.example", "no-reply"])
func personAddressIsNotFlagged(_ email: String) {
    #expect(!NoReply.isNoReply(email))
}

@Test func noWarningWhenEveryoneReadsReplies() {
    #expect(NoReplyWarning(to: ["franck@plomberie.example"], suggestion: nil) == nil)
}

@Test func warningNamesTheDeadAddress() {
    let w = NoReplyWarning(to: ["no-reply@invoices.example"], suggestion: nil)
    #expect(w?.detail == "no-reply@invoices.example won't reach anyone.")
}

@Test func warningTitleIsPlainWords() {
    #expect(NoReplyWarning.title == "This address doesn't accept replies")
}

@Test func warningAsksAboutTheSuggestion() {
    let w = NoReplyWarning(to: ["no-reply@invoices.example"], suggestion: franck)
    #expect(w?.question == "Did you mean franck@plomberie.example?")
}

@Test func warningWithoutSuggestionAsksNothing() {
    let w = NoReplyWarning(to: ["no-reply@invoices.example"], suggestion: nil)
    #expect(w?.question == nil)
}

@Test func suggestionAlreadyInToIsNotOffered() {
    let w = NoReplyWarning(to: ["no-reply@invoices.example", "Franck@plomberie.example"], suggestion: franck)
    #expect(w?.suggestion == nil)
}

@Test func applyingTheSuggestionSwapsTheDeadAddress() {
    let w = NoReplyWarning(to: ["lea@acme.example", "no-reply@invoices.example"], suggestion: franck)!
    #expect(w.applied(to: ["lea@acme.example", "no-reply@invoices.example"]) == ["lea@acme.example", "franck@plomberie.example"])
}

private let threadJSON = #"""
{"thread":{"id":"t","number":"M-1","account_id":"a","subject":"Votre facture","snippet":"","participants":[],
"last_from":"Franck","last_date":0,"unread":false,"message_count":1,"has_attachments":false,"status":"todo","priority":0,
"projects":[],"due":null,"summary":null,"next_action":null,"suggestion":null},
"messages":[{"id":"m","thread_id":"t","from":{"name":"Franck ADAM (via Invoices)","email":"no-reply@invoices.example"},
"to":[],"cc":[],"bcc":[],"reply_to":[{"name":"Franck ADAM","email":"franck@plomberie.example"}],"date":0,"subject":"s",
"body_text":"","body_new":"","has_quoted":false,"body_clean":null,"has_html":false,"is_from_me":false,"attachments":[],"auth":null}],
"reply":{"to":[{"name":"","email":"no-reply@invoices.example"}],"cc":[],"no_reply":["no-reply@invoices.example"],
"suggestion":{"name":"Franck ADAM","email":"franck@plomberie.example"}}}
"""#

private func decoded() throws -> ThreadDetail {
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    return try d.decode(ThreadDetail.self, from: Data(threadJSON.utf8))
}

@Test func messageDecodesItsReplyTo() throws {
    #expect(try decoded().messages[0].replyTo?.first?.email == "franck@plomberie.example")
}

@Test func threadDecodesTheNoReplyFlag() throws {
    #expect(try decoded().reply?.noReply == ["no-reply@invoices.example"])
}

@Test func threadDecodesTheSuggestion() throws {
    #expect(try decoded().reply?.suggestion == franck)
}
