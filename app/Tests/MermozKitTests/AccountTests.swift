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

@Test func expiredSignInDecodes() throws {
    #expect(try account(#","needs_reconnect":true,"sign_in_expired":true"#).mustSignInAgain)
}

@Test func olderDaemonMeansSignedIn() throws {
    #expect(try account("").mustSignInAgain == false)
}

@Test func expiringChangesTheAccount() throws {
    #expect(try account(#","sign_in_expired":true"#) != account(""))
}

private func gmail(_ id: String, _ email: String, expired: Bool) -> Account {
    Account(id: id, email: email, provider: "gmail", sync: SyncInfo(state: "idle", synced: 0, total: 0),
            needsReconnect: expired, signInExpired: expired)
}

@Test func noNoticeWhileSignedIn() {
    #expect(SignInNotice(accounts: [gmail("a", "me@example.com", expired: false)]) == nil)
}

@Test func noticeNamesTheAccount() {
    let n = SignInNotice(accounts: [gmail("a", "me@example.com", expired: false), gmail("b", "you@gmail.com", expired: true)])
    #expect(n?.title == "Sign in again to keep getting mail for you@gmail.com")
}

@Test func noticeSignsInTheStoppedAccount() {
    let n = SignInNotice(accounts: [gmail("a", "me@example.com", expired: false), gmail("b", "you@gmail.com", expired: true)])
    #expect(n?.accountId == "b")
}

@Test func noticeCountsTheOthers() {
    let n = SignInNotice(accounts: [gmail("a", "me@example.com", expired: true), gmail("b", "you@gmail.com", expired: true)])
    #expect(n?.title == "Sign in again to keep getting mail for me@example.com and 1 more")
}

@Test func outlookNoticeNamesMicrosoft() throws {
    let s = #"{"id":"o1","email":"ana@outlook.example","provider":"outlook","sign_in_expired":true,"sync":{"state":"idle","synced":0,"total":0,"error":null}}"#
    let a = try RPCCoding.decoder().decode(Account.self, from: Data(s.utf8))
    #expect(SignInNotice(accounts: [a])?.detail == SignInNotice.outlookDetail)
}

@Test func noticeSpeaksPlainWords() {
    let words = SignInNotice.detail.lowercased()
    #expect(!["token", "oauth", "grant", "invalid"].contains { words.contains($0) })
}

@Test func hiddenAccountDecodes() throws {
    #expect(try account(#","hidden":true"#).isHidden == true)
}

@Test func accountWithoutHiddenIsShown() throws {
    #expect(try account("").isHidden == false)
}

@Test func demoAccountIsKnownByItsProvider() throws {
    let s = #"{"id":"acc2","email":"maya@fernhill.example","provider":"demo","sync":{"state":"idle","synced":0,"total":0,"error":null}}"#
    #expect(try RPCCoding.decoder().decode(Account.self, from: Data(s.utf8)).isDemo)
}

@Test func gmailAccountIsNoDemo() throws {
    #expect(try account("").isDemo == false)
}

@Test func hidingChangesTheAccount() throws {
    #expect(try account(#","hidden":true"#) != account(""))
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

private func provider(_ p: String) throws -> Account {
    let s = #"{"id":"acc3","email":"ana@outlook.example","provider":""# + p
        + #"","sync":{"state":"idle","synced":0,"total":0,"error":null}}"#
    return try RPCCoding.decoder().decode(Account.self, from: Data(s.utf8))
}

@Test func outlookAccountIsKnownByItsProvider() throws {
    #expect(try provider("outlook").isOutlook)
}

@Test func outlookAccountHasNoGoogleDrive() throws {
    #expect(try provider("outlook").hasGoogleDrive == false)
}

@Test func gmailAccountHasGoogleDrive() throws {
    #expect(try provider("gmail").hasGoogleDrive)
}

@Test(arguments: [("gmail", "Gmail"), ("outlook", "Outlook")])
func accountsNameTheirService(provider p: String, name: String) throws {
    #expect(try provider(p).serviceName == name)
}

@Test func demoAccountNamesNoService() throws {
    #expect(try provider("demo").serviceName == nil)
}

@Test func outlookSignInDecodesItsProvider() throws {
    #expect(try signIn(#"{"running":true,"error":null,"problem":null,"provider":"outlook"}"#).isOutlook)
}

@Test func outlookSignInOffersWhenTheDaemonCan() throws {
    #expect(try signIn(#"{"running":false,"error":null,"problem":null,"outlook_available":true}"#).canAddOutlook)
}

@Test func outlookSignInIsOffWhenTheDaemonCant() throws {
    #expect(try signIn(#"{"running":false,"error":null,"problem":null,"outlook_available":false}"#).canAddOutlook == false)
}

@Test func olderDaemonsOfferNoOutlook() throws {
    #expect(try signIn(#"{"running":false,"error":null,"problem":null}"#).canAddOutlook == false)
}
