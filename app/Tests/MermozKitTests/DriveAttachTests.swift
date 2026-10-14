/// Drive files in the composer as links or regular files: the chip's switch, the
/// download states, the size limit, the share prompt, and the link-or-file default.
import Foundation
import Testing
@testable import MermozKit

private let notice = DriveOrigin(fileId: "1NoticeScanPdf", name: "Avis 2026.pdf", mime: "application/pdf",
                                 link: URL(string: "https://drive.google.com/file/d/1NoticeScanPdf/view")!)
private let certificate = DriveOrigin(fileId: "1CertificateDoc", name: "Attestation",
                                      mime: "application/vnd.google-apps.document",
                                      link: URL(string: "https://docs.google.com/document/d/1CertificateDoc/edit")!)
private let noticeFile = DriveDownload(fileId: "1NoticeScanPdf", path: "/tmp/fictional/dl_1/Avis 2026.pdf",
                                       filename: "Avis 2026.pdf", mime: "application/pdf", size: 180_000)
private let certificatePDF = DriveDownload(fileId: "1CertificateDoc", path: "/tmp/fictional/dl_2/Attestation.pdf",
                                           filename: "Attestation.pdf", mime: "application/pdf", size: 64_000)
private let storedLink = DraftAttachment(id: "datt_7", kind: "link", filename: "Attestation", mime: "text/uri-list", size: 0,
                                         url: "https://docs.google.com/document/d/1CertificateDoc/edit")

/// A list holding one Drive file, as a link.
private func listWith(_ origin: DriveOrigin) -> (AttachmentList, UUID) {
    var l = AttachmentList()
    let item = ComposeAttachment(drive: origin)
    l.add(item)
    return (l, item.id)
}

// MARK: link or file

@Test func aDriveFileStartsAsALink() {
    let (l, _) = listWith(notice)
    #expect(l.items[0].isLink)
}

@Test func downloadingShowsOnTheItem() {
    var (l, id) = listWith(notice)
    l.startDownload(id: id)
    #expect(l.items[0].fetch == .downloading)
}

@Test func downloadingBlocksSend() {
    var (l, id) = listWith(notice)
    l.startDownload(id: id)
    #expect(l.blocksSend)
}

@Test func aDownloadedFileIsAFile() {
    var (l, id) = listWith(notice)
    l.downloaded(id: id, noticeFile)
    #expect(l.items[0].source == .file(URL(fileURLWithPath: "/tmp/fictional/dl_1/Avis 2026.pdf")))
}

@Test func aDownloadedFileShowsItsSize() {
    var (l, id) = listWith(notice)
    l.downloaded(id: id, noticeFile)
    #expect(l.items[0].size == 180_000)
}

@Test func anExportTakesTheExportedName() {
    var (l, id) = listWith(certificate)
    l.downloaded(id: id, certificatePDF)
    #expect(l.items[0].name == "Attestation.pdf")
}

@Test func aDownloadedFileCountsTowardTheLimit() {
    var (l, id) = listWith(notice)
    l.downloaded(id: id, noticeFile)
    #expect(l.totalSize == 180_000)
}

@Test func aDownloadedFileIsSentAsAPath() {
    var (l, id) = listWith(notice)
    l.downloaded(id: id, noticeFile)
    #expect(l.newSpecs == [AttachSpec(path: "/tmp/fictional/dl_1/Avis 2026.pdf", filename: "Avis 2026.pdf")])
}

@Test func aDownloadedFileDoesNotBlockSend() {
    var (l, id) = listWith(notice)
    l.startDownload(id: id)
    l.downloaded(id: id, noticeFile)
    #expect(!l.blocksSend)
}

@Test func tooBigADownloadIsOverTheLimit() {
    var (l, id) = listWith(notice)
    l.downloaded(id: id, DriveDownload(fileId: "1NoticeScanPdf", path: "/tmp/fictional/big.pdf", filename: "big.pdf",
                                       mime: "application/pdf", size: AttachmentList.maxBytes + 1))
    #expect(l.isOverLimit)
}

@Test func aFailedDownloadStaysALink() {
    var (l, id) = listWith(notice)
    l.downloadFailed(id: id, "Couldn't download “Avis 2026.pdf” from Google Drive. Try again, or keep it as a link.")
    #expect(l.items[0].isLink)
}

@Test func aFailedDownloadSaysWhy() {
    var (l, id) = listWith(notice)
    l.downloadFailed(id: id, "“Visit.mov” is 40.0 MB, more than the 25.0 MB Gmail can send in one email. Keep it as a link.")
    #expect(l.items[0].fetch == .failed("“Visit.mov” is 40.0 MB, more than the 25.0 MB Gmail can send in one email. Keep it as a link."))
}

@Test func aFailedDownloadBlocksSendUntilTheUserChooses() {
    var (l, id) = listWith(notice)
    l.downloadFailed(id: id, "No.")
    #expect(l.blocksSend)
}

@Test func keepAsLinkAfterAFailureAllowsSending() {
    var (l, id) = listWith(notice)
    l.downloadFailed(id: id, "No.")
    l.keepAsLink(id: id)
    #expect(!l.blocksSend)
}

@Test func keepAsLinkTurnsAFileBackIntoALink() {
    var (l, id) = listWith(certificate)
    l.downloaded(id: id, certificatePDF)
    l.keepAsLink(id: id)
    #expect(l.items[0].source == .link(certificate.link))
}

@Test func keepAsLinkRestoresTheDriveName() {
    var (l, id) = listWith(certificate)
    l.downloaded(id: id, certificatePDF)
    l.keepAsLink(id: id)
    #expect(l.items[0].name == "Attestation")
}

@Test func keepAsLinkFreesTheSize() {
    var (l, id) = listWith(certificate)
    l.downloaded(id: id, certificatePDF)
    l.keepAsLink(id: id)
    #expect(l.totalSize == 0)
}

@Test func theSameDriveFileIsAddedOnce() {
    var (l, _) = listWith(notice)
    l.add(ComposeAttachment(drive: notice))
    #expect(l.items.count == 1)
}

// MARK: a draft's saved links

@Test func aSavedLinkIsNotSentAgain() {
    let l = AttachmentList(stored: [storedLink])
    #expect(l.newSpecs.isEmpty)
}

@Test func aSavedLinkKnowsItsDriveFile() {
    let l = AttachmentList(stored: [storedLink])
    #expect(l.items[0].drive?.fileId == "1CertificateDoc")
}

@Test func aSavedLinkTurnedIntoAFileIsDetached() {
    var l = AttachmentList(stored: [storedLink])
    l.downloaded(id: l.items[0].id, certificatePDF)
    #expect(l.removedStored == ["datt_7"])
}

@Test func aSavedLinkTurnedIntoAFileIsAttached() {
    var l = AttachmentList(stored: [storedLink])
    l.downloaded(id: l.items[0].id, certificatePDF)
    #expect(l.newSpecs == [AttachSpec(path: "/tmp/fictional/dl_2/Attestation.pdf", filename: "Attestation.pdf")])
}

@Test func removingASavedLinkDetachesIt() {
    var l = AttachmentList(stored: [storedLink])
    l.remove(id: l.items[0].id)
    #expect(l.removedStored == ["datt_7"])
}

// MARK: the share prompt

@Test func noShareQuestionWhenEveryDriveFileIsAFile() {
    var l = AttachmentList()
    let a = ComposeAttachment(drive: notice), b = ComposeAttachment(drive: certificate)
    l.add(a)
    l.add(b)
    l.downloaded(id: a.id, noticeFile)
    l.downloaded(id: b.id, certificatePDF)
    #expect(!SharePlan(attachments: l.items, recipients: ["accueil@urssaf.example"], me: "me@example.com").isNeeded)
}

@Test func theShareQuestionCoversOnlyTheLinks() {
    var l = AttachmentList()
    let a = ComposeAttachment(drive: notice), b = ComposeAttachment(drive: certificate)
    l.add(a)
    l.add(b)
    l.downloaded(id: a.id, noticeFile)
    let plan = SharePlan(attachments: l.items, recipients: ["accueil@urssaf.example"], me: "me@example.com")
    #expect(plan.files == [SharePlan.File(id: "1CertificateDoc", name: "Attestation")])
}

@Test func aDriveLinkStillAsksToShare() {
    let (l, _) = listWith(certificate)
    #expect(SharePlan(attachments: l.items, recipients: ["accueil@urssaf.example"], me: "me@example.com").isNeeded)
}

// MARK: link or file by default

@Test(arguments: [
    (["accueil@urssaf.example"], "me@acme.example", true),
    (["accueil@urssaf.example", "impots@dgfip.example"], "me@acme.example", true),
    (["accueil@urssaf.example", "lea@gmail.com"], "me@acme.example", false),
    (["Lea@GoogleMail.com"], "me@acme.example", false),
    (["marc@acme.example"], "me@acme.example", false),
    (["accueil@urssaf.example", "marc@acme.example"], "me@acme.example", false),
    (["accueil@urssaf.example"], "me@gmail.com", true),
    ([], "me@acme.example", false),
    (["not an address"], "me@acme.example", false),
])
func everyoneOutsideTheUsersGoogleWorld(recipients: [String], me: String, want: Bool) {
    #expect(DriveAttachMode.allOutside(recipients: recipients, me: me) == want)
}

/// Memory backed by a throwaway defaults suite, so tests never touch the user's settings.
private func freshMemory() -> DriveAttachMemory {
    let name = "DriveAttachTests.\(UUID().uuidString)"
    return DriveAttachMemory(defaults: UserDefaults(suiteName: name)!)
}

@Test func anAdministrationGetsFilesByDefault() {
    #expect(DriveAttachMode.preferred(recipients: ["accueil@urssaf.example"], me: "me@acme.example", memory: freshMemory()) == .file)
}

@Test func colleaguesGetLinksByDefault() {
    #expect(DriveAttachMode.preferred(recipients: ["marc@acme.example"], me: "me@acme.example", memory: freshMemory()) == .link)
}

@Test func noRecipientsYetMeansLinks() {
    #expect(DriveAttachMode.preferred(recipients: [], me: "me@acme.example", memory: freshMemory()) == .link)
}

@Test func theLastChoiceForOutsidersWins() {
    let memory = freshMemory()
    memory.remember(.link, outside: true)
    #expect(DriveAttachMode.preferred(recipients: ["accueil@urssaf.example"], me: "me@acme.example", memory: memory) == .link)
}

@Test func theLastChoiceForColleaguesWins() {
    let memory = freshMemory()
    memory.remember(.file, outside: false)
    #expect(DriveAttachMode.preferred(recipients: ["marc@acme.example"], me: "me@acme.example", memory: memory) == .file)
}

@Test func aChoiceForOutsidersLeavesColleaguesAlone() {
    let memory = freshMemory()
    memory.remember(.file, outside: true)
    #expect(DriveAttachMode.preferred(recipients: ["marc@acme.example"], me: "me@acme.example", memory: memory) == .link)
}

// MARK: formats

@Test(arguments: [
    ("application/vnd.google-apps.document", ["pdf", "docx"]),
    ("application/vnd.google-apps.spreadsheet", ["pdf", "xlsx"]),
    ("application/pdf", [String]()),
])
func formatsOfferedOnTheChip(mime: String, want: [String]) {
    #expect(DriveAttachMode.formats(mime: mime) == want)
}

@Test func wordIsNamedPlainly() {
    #expect(DriveAttachMode.formatLabel("docx") == "Word (.docx)")
}
