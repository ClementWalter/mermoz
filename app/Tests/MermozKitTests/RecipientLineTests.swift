/// "to Elliot, +2 cc": who an inline reply goes to, in one line.
import Testing
@testable import MermozKit

private let names = ["elliot@example.com": "Elliot Moss"]

@Test func theFirstRecipientIsNamedByFirstName() {
    #expect(RecipientLine.compact(to: ["elliot@example.com"], cc: [], names: names) == "to Elliot")
}

@Test func ccRecipientsAreCounted() {
    #expect(RecipientLine.compact(to: ["elliot@example.com"], cc: ["a@x.com", "b@x.com"], names: names) == "to Elliot, +2 cc")
}

@Test func moreToRecipientsAreCounted() {
    #expect(RecipientLine.compact(to: ["elliot@example.com", "a@x.com"], cc: [], names: names) == "to Elliot +1")
}

@Test func anUnnamedRecipientShowsTheAddress() {
    #expect(RecipientLine.compact(to: ["zoe@example.com"], cc: [], names: names) == "to zoe@example.com")
}

@Test func aReplyWithNobodySaysSo() {
    #expect(RecipientLine.compact(to: [], cc: [], names: names) == "No recipients")
}
