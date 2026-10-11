/// Recipient heuristics: one verdict per case.
import Testing
@testable import MermozKit

private let checker = RecipientChecker(addresses: [
    Address(name: "Marc Petit", email: "marc.petit@acme.example"),
    Address(name: "Jonas Weber", email: "jonas@acme.example"),
])

@Test func knownAddressIsKnown() {
    #expect(checker.check("marc.petit@acme.example") == .known)
}

@Test func knownAddressMatchesCaseInsensitively() {
    #expect(checker.check("Marc.Petit@ACME.example") == .known)
}

@Test func cleanUnseenAddressIsUnknown() {
    #expect(checker.check("someone@company.org") == .unknown)
}

@Test(arguments: ["no-at-sign", "a@b", "@acme.example", "a@acme..example", "a b@acme.example", "a@.acme.example"])
func malformedAddressIsSuspicious(_ raw: String) {
    #expect(checker.check(raw) == .suspicious(reason: "This isn't a valid email address."))
}

@Test(arguments: ["x@w0rkmail.com", "x@paypa1ly.com"])
func digitSubstitutionIsFlagged(_ raw: String) {
    #expect(checker.check(raw) == .suspicious(reason: "This domain swaps letters for look-alike numbers."))
}

@Test func disposableDomainIsFlagged() {
    #expect(checker.check("x@mailinator.com") == .suspicious(reason: "This is a throwaway email service."))
}

@Test func riskyTldWithHyphenIsFlagged() {
    #expect(checker.check("x@secure-login.xyz") == .suspicious(reason: "This domain looks newly made up."))
}

@Test func randomLookingLabelIsFlagged() {
    #expect(checker.check("x@qzxkvbnrtplm.com") == .suspicious(reason: "This domain looks randomly generated."))
}

@Test func knownNameOnUnknownDomainIsFlagged() {
    #expect(checker.check("marc.petit@other.example") ==
        .suspicious(reason: "This doesn't look like Marc Petit's usual address. You've never emailed this domain."))
}

@Test func lookalikeOfKnownDomainIsFlagged() {
    #expect(checker.check("bob@acrne.example") == .suspicious(reason: "This domain looks like acme.example but isn't the same."))
}

@Test(arguments: [("kitten", "sitting", 3), ("acme", "acne", 1), ("", "abc", 3), ("same", "same", 0)])
func editDistanceMatches(_ a: String, _ b: String, _ expected: Int) {
    #expect(RecipientChecker.editDistance(a, b) == expected)
}
