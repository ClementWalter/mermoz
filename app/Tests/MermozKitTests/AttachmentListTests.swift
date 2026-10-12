/// The composer's attachment list: adding, removing, total size, duplicates, limit.
import Foundation
import Testing
@testable import MermozKit

private let plan = ComposeAttachment(name: "plan.pdf", size: 2048, source: .file(URL(fileURLWithPath: "/tmp/fictional/plan.pdf")))
private let photo = ComposeAttachment(name: "photo.jpg", size: 3 * 1024 * 1024, source: .file(URL(fileURLWithPath: "/tmp/fictional/photo.jpg")))
private let drive = ComposeAttachment(name: "Budget", size: 0, source: .link(URL(string: "https://drive.example/f/1")!))
private let saved = DraftAttachment(id: "datt_1", filename: "notes.txt", mime: "text/plain", size: 512)

@Test func addKeepsTheItem() {
    var l = AttachmentList()
    l.add(plan)
    #expect(l.items.map(\.name) == ["plan.pdf"])
}

@Test func addKeepsInsertionOrder() {
    var l = AttachmentList()
    l.add(photo)
    l.add(plan)
    #expect(l.items.map(\.name) == ["photo.jpg", "plan.pdf"])
}

@Test func sameFileTwiceIsAddedOnce() {
    var l = AttachmentList()
    l.add(plan)
    l.add(ComposeAttachment(name: "plan.pdf", size: 2048, source: .file(URL(fileURLWithPath: "/tmp/fictional/./plan.pdf"))))
    #expect(l.items.count == 1)
}

@Test func duplicateAddReportsFalse() {
    var l = AttachmentList()
    l.add(plan)
    let added = l.add(plan)
    #expect(!added)
}

@Test func sameNameInAnotherFolderIsAdded() {
    var l = AttachmentList()
    l.add(plan)
    l.add(ComposeAttachment(name: "plan.pdf", size: 10, source: .file(URL(fileURLWithPath: "/tmp/other/plan.pdf"))))
    #expect(l.items.count == 2)
}

@Test func sameMessageAttachmentIsAddedOnce() {
    var l = AttachmentList()
    l.add(ComposeAttachment(name: "a.pdf", size: 1, source: .message(attachmentId: "att_1")))
    l.add(ComposeAttachment(name: "a.pdf", size: 1, source: .message(attachmentId: "att_1")))
    #expect(l.items.count == 1)
}

@Test func removeDropsTheItem() {
    var l = AttachmentList()
    l.add(plan)
    l.add(photo)
    l.remove(id: plan.id)
    #expect(l.items.map(\.name) == ["photo.jpg"])
}

@Test func removingASavedItemRemembersToDetachIt() {
    var l = AttachmentList(stored: [saved])
    l.remove(id: l.items[0].id)
    #expect(l.removedStored == ["datt_1"])
}

@Test func removingANewItemDetachesNothing() {
    var l = AttachmentList()
    l.add(plan)
    l.remove(id: plan.id)
    #expect(l.removedStored.isEmpty)
}

@Test func totalSizeAddsFiles() {
    var l = AttachmentList()
    l.add(plan)
    l.add(photo)
    #expect(l.totalSize == 2048 + 3 * 1024 * 1024)
}

@Test func linksDoNotCountTowardsTheSize() {
    var l = AttachmentList()
    l.add(plan)
    l.add(drive)
    #expect(l.totalSize == 2048)
}

@Test func atTheLimitIsFine() {
    var l = AttachmentList()
    l.add(ComposeAttachment(name: "big.mov", size: AttachmentList.maxBytes, source: .file(URL(fileURLWithPath: "/tmp/big.mov"))))
    #expect(!l.isOverLimit)
}

@Test func overTheLimitIsFlagged() {
    var l = AttachmentList()
    l.add(ComposeAttachment(name: "big.mov", size: AttachmentList.maxBytes + 1, source: .file(URL(fileURLWithPath: "/tmp/big.mov"))))
    #expect(l.isOverLimit)
}

@Test func overTheLimitExplainsInPlainWords() {
    var l = AttachmentList()
    l.add(ComposeAttachment(name: "big.mov", size: 30 * 1024 * 1024, source: .file(URL(fileURLWithPath: "/tmp/big.mov"))))
    #expect(l.limitMessage == "These files add up to 30.0 MB. Gmail can send at most 25.0 MB per email: remove some files, or share the large ones with a link.")
}

@Test func underTheLimitHasNoMessage() {
    var l = AttachmentList()
    l.add(plan)
    #expect(l.limitMessage == nil)
}

@Test func savedItemsAreNotSentAgain() {
    let l = AttachmentList(stored: [saved])
    #expect(l.newSpecs.isEmpty)
}

@Test func newFileBecomesAPathSpec() {
    var l = AttachmentList()
    l.add(plan)
    #expect(l.newSpecs == [AttachSpec(path: "/tmp/fictional/plan.pdf", filename: "plan.pdf")])
}

@Test func messageAttachmentSpecUsesTheProtocolKey() throws {
    let spec = AttachSpec(fromAttachment: "att_1")
    let json = String(data: try JSONEncoder().encode(spec), encoding: .utf8)!
    #expect(json.contains("\"from_attachment\":\"att_1\""))
}

@Test(arguments: [(Int64(1536), "2 KB"), (Int64(3 * 1024 * 1024 + 400 * 1024), "3.4 MB"), (Int64(10), "1 KB")])
func sizeFormatting(bytes: Int64, want: String) {
    #expect(AttachmentList.format(bytes) == want)
}

@Test func savedDraftItemsKeepTheirNames() {
    let l = AttachmentList(stored: [saved])
    #expect(l.items.map(\.name) == ["notes.txt"])
}
