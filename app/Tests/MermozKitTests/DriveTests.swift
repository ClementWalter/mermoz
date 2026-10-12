/// Google Drive payloads decode as mermozd sends them; link cards, Settings text, the
/// Docs tab entries and the share question follow what the daemon reported.
import Foundation
import Testing
@testable import MermozKit

private func decode<T: Decodable>(_ s: String) throws -> T {
    try RPCCoding.decoder().decode(T.self, from: Data(s.utf8))
}

private let plan = DriveFile(id: "1Plan", name: "Remediation plan", mime: "application/vnd.google-apps.document",
                             kind: "Google Doc", modified: 1_791_648_000_000,
                             owner: .init(name: "Marc", email: "marc@acme.example"), webLink: "https://docs.example/plan")
private let forecast = DriveFile(id: "1Fore", name: "Q4 forecast", kind: "Google Sheet", modified: 1_791_600_000_000)

private let filesJSON = #"""
{"account_id":"acc_1","connected":true,"can_read_all":true,
 "linked":[{"file_id":"1Plan","url":"https://docs.google.com/document/d/1Plan/edit","link_kind":"document","message_id":"msg_1","access":"ok",
   "file":{"id":"1Plan","name":"Remediation plan","mime":"application/vnd.google-apps.document","kind":"Google Doc","modified":1791648000000,
           "owner":{"name":"Marc","email":"marc@acme.example"},"web_link":"https://docs.example/plan","icon_link":null,"has_thumbnail":true}}],
 "shared":[],"saved":[{"attachment_id":"att_1","file_id":"f9","name":"report.pdf","web_link":null,"folder":"Mermoz/Clients/Acme","created":1}]}
"""#

@Test func threadFilesDecodeTheLinkedFile() throws {
    let f: DriveThreadFiles = try decode(filesJSON)
    #expect(f.linked[0].file?.name == "Remediation plan")
}

@Test func threadFilesDecodeTheThumbnailFlag() throws {
    let f: DriveThreadFiles = try decode(filesJSON)
    #expect(f.linked[0].file?.hasThumbnail == true)
}

@Test func threadFilesFindTheSavedAttachment() throws {
    let f: DriveThreadFiles = try decode(filesJSON)
    #expect(f.saved("att_1")?.folder == "Mermoz/Clients/Acme")
}

@Test func savedLineReadsAsFolders() {
    let s = SavedToDrive(attachmentId: "a", fileId: "f", name: "r.pdf", webLink: nil, folder: "Mermoz/Clients/Acme")
    #expect(s.savedLine == "Saved to Mermoz › Clients › Acme")
}

@Test func savedFileWithoutLinkStillOpensInDrive() {
    let s = SavedToDrive(attachmentId: "a", fileId: "f9", name: "r.pdf", webLink: nil, folder: "Mermoz")
    #expect(s.openURL?.absoluteString == "https://drive.google.com/open?id=f9")
}

@Test func linksAreKeptPerMessage() {
    let f = DriveThreadFiles(accountId: "a", connected: true, linked: [
        DriveLinkItem(fileId: "1", url: "u1", messageId: "m1", access: "ok"),
        DriveLinkItem(fileId: "2", url: "u2", messageId: "m2", access: "ok"),
    ])
    #expect(f.links(in: "m2").map(\.fileId) == ["2"])
}

@Test func driveChangedEventDecodes() {
    let line = Data(#"{"jsonrpc":"2.0","method":"event","params":{"kind":"drive_changed","account_id":"acc_1"}}"#.utf8)
    guard case let .event(e) = decodeIncoming(line) else { Issue.record("not an event"); return }
    #expect(e == .driveChanged(accountId: "acc_1"))
}

// MARK: Docs tab

@Test func docsTabListsLinkedThenSharedFiles() {
    let f = DriveThreadFiles(accountId: "a", connected: true, canReadAll: true,
                             linked: [DriveLinkItem(fileId: "1Plan", url: "u", messageId: "m", access: "ok", file: plan)],
                             shared: [forecast])
    #expect(f.docEntries.map(\.id) == ["link:1Plan", "link:1Fore"])
}

@Test func docsTabShowsAFileOnce() {
    let f = DriveThreadFiles(accountId: "a", connected: true,
                             linked: [DriveLinkItem(fileId: "1Plan", url: "u", messageId: "m", access: "ok", file: plan)],
                             shared: [plan])
    #expect(f.docEntries.count == 1)
}

@Test func docsTabSkipsLinksMermozCannotOpen() {
    let f = DriveThreadFiles(accountId: "a", connected: true,
                             linked: [DriveLinkItem(fileId: "1Plan", url: "u", messageId: "m", access: "no_access")])
    #expect(f.docEntries.isEmpty)
}

@Test func docsTabEntryNamesTheOwner() {
    let f = DriveThreadFiles(accountId: "a", connected: true, shared: [plan])
    #expect(f.docEntries == [.linked(id: "1Plan", title: "Remediation plan", url: "https://docs.example/plan",
                                     from: "Marc", date: 1_791_648_000_000)])
}

// MARK: link cards

@Test(arguments: [
    ("ok", true, DriveCardState.preview(plan)),
    ("not_connected", false, DriveCardState.connect),
    ("no_access", false, DriveCardState.allow),
    ("error", false, DriveCardState.unavailable),
])
func linkCardFollowsAccess(access: String, hasFile: Bool, want: DriveCardState) {
    let l = DriveLinkItem(fileId: "1Plan", url: "u", messageId: "m", access: access, file: hasFile ? plan : nil)
    #expect(DriveCardState(l) == want)
}

// MARK: errors

private func rpcError(_ code: Int, _ data: String) -> RPCError {
    RPCError(code: code, message: "x", data: Data(data.utf8))
}

@Test(arguments: [
    (-32030, #"{"account_id":"acc_1"}"#, DriveNeed.connect(accountId: "acc_1")),
    (-32031, #"{"account_id":"acc_1","reason":"read_all"}"#, DriveNeed.readAll),
    (-32031, #"{"account_id":"acc_1","reason":"save"}"#, DriveNeed.save),
    (-32031, #"{"account_id":"acc_1","reason":"pick"}"#, DriveNeed.pick),
])
func driveErrorsSayWhatIsNeeded(code: Int, data: String, want: DriveNeed) {
    #expect(DriveNeed(rpcError(code, data)) == want)
}

@Test func otherErrorsAreNotDriveNeeds() {
    #expect(DriveNeed(rpcError(-32602, "{}")) == nil)
}

// MARK: Settings

@Test(arguments: [
    (DriveStatus(accountId: "a", email: "e", connected: true, canReadAll: true, canSave: true), "Connected. Mermoz shows"),
    (DriveStatus(accountId: "a", email: "e", connected: true, canReadAll: false, canSave: true), "Connected to files you choose."),
    (DriveStatus(accountId: "a", email: "e", connected: false, attempt: .init(state: "declined")), "Not connected: you said no in Google."),
    (DriveStatus(accountId: "a", email: "e", connected: false, attempt: .init(state: "waiting")), "Finish in your browser"),
    (DriveStatus(accountId: "a", email: "e", connected: false), "See documents linked in your mail"),
])
func settingsSummaryIsPlainWords(status: DriveStatus, starts: String) {
    #expect(status.summary.hasPrefix(starts))
}

@Test func settingsSummaryNeverSaysScope() {
    let all = [
        DriveStatus(accountId: "a", email: "e", connected: true, canReadAll: true, canSave: false),
        DriveStatus(accountId: "a", email: "e", connected: true, canReadAll: false, canSave: true),
        DriveStatus(accountId: "a", email: "e", connected: false, attempt: .init(state: "declined")),
    ].map(\.summary).joined(separator: " ").lowercased()
    #expect(!all.contains("scope") && !all.contains("oauth") && !all.contains("api"))
}

@Test(arguments: [
    (DriveStatus(accountId: "a", email: "e", connected: false) as DriveStatus?, true),
    (DriveStatus(accountId: "a", email: "e", connected: true) as DriveStatus?, false),
    (nil as DriveStatus?, false),
])
func connectIsAskedOnlyWhenKnownDisconnected(status: DriveStatus?, want: Bool) {
    #expect(DriveStatus.needsConnect(status) == want)
}

@Test func statusDecodesTheAttempt() throws {
    let s: DriveStatus = try decode(#"{"account_id":"acc_1","email":"me@example.com","connected":false,"can_read_all":false,"can_save":false,"attempt":{"state":"declined","error":null}}"#)
    #expect(s.attempt?.state == "declined")
}

// MARK: file ids in links

@Test(arguments: [
    ("https://docs.google.com/document/d/1AbC/edit", "1AbC"),
    ("https://docs.google.com/spreadsheets/u/1/d/1AbC/edit#gid=0", "1AbC"),
    ("https://drive.google.com/file/d/1AbC/view", "1AbC"),
    ("https://drive.google.com/open?id=1AbC", "1AbC"),
    ("https://drive.google.com/drive/folders/1AbC", "1AbC"),
])
func fileIdsComeFromDriveLinks(url: String, want: String) {
    #expect(DriveLinks.fileId(URL(string: url)!) == want)
}

@Test(arguments: ["https://example.com/d/1AbC", "https://docs.google.com/document/d/e/2PACX/pub"])
func otherLinksHaveNoFileId(url: String) {
    #expect(DriveLinks.fileId(URL(string: url)!) == nil)
}

// MARK: sharing before sending

private let driveLink = ComposeAttachment(name: "Q4 forecast", size: 0,
                                          source: .link(URL(string: "https://docs.google.com/spreadsheets/d/1Fore/edit")!))
private let webLink = ComposeAttachment(name: "Blog", size: 0, source: .link(URL(string: "https://blog.example/post")!))

@Test func shareIsAskedForDriveFiles() {
    #expect(SharePlan(attachments: [driveLink], recipients: ["marc@acme.example"], me: "me@example.com").isNeeded)
}

@Test func shareIsNotAskedForOtherLinks() {
    #expect(!SharePlan(attachments: [webLink], recipients: ["marc@acme.example"], me: "me@example.com").isNeeded)
}

@Test func shareSkipsTheUser() {
    let p = SharePlan(attachments: [driveLink], recipients: ["Me@Example.com", "marc@acme.example"], me: "me@example.com")
    #expect(p.emails == ["marc@acme.example"])
}

@Test func shareIsNotAskedWhenOnlyTheUserReceives() {
    #expect(!SharePlan(attachments: [driveLink], recipients: ["me@example.com"], me: "me@example.com").isNeeded)
}

@Test func shareQuestionNamesOneFileAndOnePerson() {
    let p = SharePlan(attachments: [driveLink], recipients: ["marc@acme.example"], me: nil)
    #expect(p.question == "Let marc@acme.example open “Q4 forecast”?")
}

@Test func shareQuestionCountsSeveralRecipients() {
    let p = SharePlan(attachments: [driveLink], recipients: ["marc@acme.example", "lea@example.org"], me: nil)
    #expect(p.question == "Let the 2 recipients open “Q4 forecast”?")
}
