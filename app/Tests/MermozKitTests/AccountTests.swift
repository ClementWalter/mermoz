/// Accounts decode as mermozd sends them, including whether Google needs a new sign-in.
import Foundation
import Testing
@testable import MermozKit

private func account(_ extra: String) throws -> Account {
    let s = #"{"id":"acc1","email":"me@example.com","provider":"gmail","sync":{"state":"idle","synced":3,"total":3,"error":null}"#
        + extra + "}"
    return try RPCCoding.decoder().decode(Account.self, from: Data(s.utf8))
}

@Test func accountNeedingReconnectDecodes() throws {
    #expect(try account(#","needs_reconnect":true"#).needsReconnect == true)
}

@Test func accountWithoutTheFlagDecodes() throws {
    #expect(try account("").needsReconnect == nil)
}
