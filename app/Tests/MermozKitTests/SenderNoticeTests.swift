/// What the reader is told about new and lookalike senders: one wording per case.
import Foundation
import Testing
@testable import MermozKit

private func message(_ email: String, auth: String, usual: String? = nil, claim: SenderClaim? = nil) -> Message {
    let json = """
    {"id":"m1","thread_id":"t","from":{"name":"GitHub","email":"\(email)"},"to":[],"cc":[],"date":1,"subject":"S",
     "body_text":"x","has_html":false,"is_from_me":false,"attachments":[],"auth":"\(auth)"}
    """
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    var m = try! d.decode(Message.self, from: Data(json.utf8))
    m.usualDomain = usual
    m.senderClaim = claim
    return m
}

private let github = SenderClaim(kind: "organisation", organisation: "GitHub", domains: ["github.com"], matches: false)

@Test func aFirstEmailIsLabelled() {
    #expect(SenderNotice.label(message("hi@github.com", auth: "new")) == "First email")
}

@Test func aVerifiedSenderHasNoLabel() {
    #expect(SenderNotice.label(message("hi@github.com", auth: "pass")) == nil)
}

@Test func aFirstEmailWithoutResearchWarns() {
    #expect(SenderNotice.detail(message("hi@stripe.example", auth: "new"))
        == "First email from stripe.example: you have never received mail from it before. Be careful with links and attachments.")
}

@Test func aFirstEmailFromTheOfficialDomainReassures() {
    var official = github
    official.matches = true
    #expect(SenderNotice.detail(message("noreply@github.com", auth: "new", claim: official))
        == "First email from github.com: you have never received mail from it before. It is GitHub's official address.")
}

@Test func aLookalikeNamesTheRealDomain() {
    #expect(SenderNotice.detail(message("noreply@noreply-github.co", auth: "lookalike", usual: "github.com", claim: github))
        == "Likely not from “GitHub”: noreply-github.co is a lookalike address. GitHub's emails come from github.com. Don't click its links or open its attachments.")
}

@Test func aLookalikeFromHistoryNamesTheUsualDomain() {
    #expect(SenderNotice.detail(message("noreply@noreply-github.co", auth: "lookalike", usual: "github.com"))
        == "Likely not from “GitHub”: noreply-github.co is a lookalike address. “GitHub” usually writes from github.com. Don't click its links or open its attachments.")
}

@Test func theDaemonsClaimDecodes() throws {
    let json = """
    {"id":"m1","thread_id":"t","from":{"name":"GitHub","email":"a@b.co"},"to":[],"cc":[],"date":1,"subject":"S",
     "body_text":"x","has_html":false,"is_from_me":false,"attachments":[],"auth":"new",
     "sender_claim":{"kind":"organisation","organisation":"GitHub","domains":["github.com"],"sources":[],"matches":false}}
    """
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    #expect(try d.decode(Message.self, from: Data(json.utf8)).senderClaim == github)
}
