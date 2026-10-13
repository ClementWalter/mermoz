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

private func signIn(_ s: String) throws -> GmailSignIn {
    try RPCCoding.decoder().decode(GmailSignIn.self, from: Data(s.utf8))
}

private let refusal = #"{"running":false,"error":"Your organization only allows apps it trusts.","problem":{"kind":"admin_policy","message":"Your organization only allows apps it trusts.","client_id":"app-cid","custom":false},"app_client_id":"app-cid"}"#

@Test func refusedSignInDecodesItsKind() throws {
    #expect(try signIn(refusal).problem?.kind == "admin_policy")
}

@Test func refusedSignInDecodesTheClientToTrust() throws {
    #expect(try signIn(refusal).problem?.clientId == "app-cid")
}

@Test func signInWithoutProblemDecodes() throws {
    #expect(try signIn(#"{"running":true,"error":null,"problem":null,"app_client_id":"app-cid"}"#).problem == nil)
}

private func params(_ c: OwnGoogleClient?) throws -> [String: String] {
    let data = DaemonClient.addGmailParams(c).data()
    return try JSONSerialization.jsonObject(with: data) as? [String: String] ?? [:]
}

@Test func ownClientFileIsSentAsIs() throws {
    #expect(try params(.file(#"{"installed":{}}"#)) == ["client_json": #"{"installed":{}}"#])
}

@Test func pastedOwnClientIsSentAsIds() throws {
    #expect(try params(.ids(clientId: "company-cid", secret: "s")) == ["client_id": "company-cid", "client_secret": "s"])
}

@Test func appClientSendsNoClient() throws {
    #expect(try params(nil).isEmpty)
}
