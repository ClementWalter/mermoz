/// Label nesting from Gmail's "Parent/Child" names.
import Testing
@testable import MermozKit

@Test(arguments: [("Receipts", 0), ("Clients/Acme", 1), ("Clients/Acme/2026", 2)])
func depthFollowsSlashes(name: String, depth: Int) {
    #expect(MailLabel(accountId: "a", id: "L", name: name, count: 0).depth == depth)
}

@Test func leafIsLastComponent() {
    #expect(MailLabel(accountId: "a", id: "L", name: "Clients/Acme", count: 0).leaf == "Acme")
}
