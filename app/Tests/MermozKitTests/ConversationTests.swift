/// Conversations read as one chat: markers where another Gmail thread starts, files split
/// into docs and photos, and the protocol fields decode.
import Foundation
import Testing
@testable import MermozKit

private func message(_ id: String, thread: String, date: Int64, subject: String = "Flat sale") -> Message {
    Message(id: id, threadId: thread, from: Address(name: "Nora Lind", email: "nora@agency.example"), to: [], cc: [],
            bcc: nil, date: date, subject: subject, bodyText: "Hello", bodyNew: nil, hasQuoted: nil, bodyClean: nil,
            hasHtml: false, isFromMe: false, attachments: [], auth: nil)
}

private func thread(_ id: String, subject: String) -> ThreadSummary {
    ThreadSummary(id: id, number: id, accountId: "a", subject: subject, snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: false, messageCount: 1, hasAttachments: false, status: "todo", priority: 0)
}

private func file(_ name: String, mime: String) -> SharedFile {
    SharedFile(id: name, filename: name, mime: mime, size: 1, messageId: "m", threadId: "t", from: "You", date: 0)
}

private let twoThreads = [message("m3", thread: "b", date: 3), message("m1", thread: "a", date: 1), message("m2", thread: "a", date: 2)]
private let subjects = [thread("a", subject: "Flat sale, next steps"), thread("b", subject: "Signing postponed")]

private func markers(_ items: [TimelineItem]) -> [TimelineItem] {
    items.filter { if case .marker = $0 { true } else { false } }
}

@Test func aLoneThreadReadsWithoutMarkers() {
    let items = Timeline.items([message("m1", thread: "a", date: 1), message("m2", thread: "a", date: 2)], threads: [])
    #expect(markers(items).isEmpty)
}

@Test func eachGmailThreadGetsAMarker() {
    #expect(markers(Timeline.items(twoThreads, threads: subjects)).count == 2)
}

@Test func messagesReadOldestFirst() {
    let ids = Timeline.items(twoThreads, threads: subjects).compactMap { item -> String? in
        if case let .message(m) = item { m.id } else { nil }
    }
    #expect(ids == ["m1", "m2", "m3"])
}

@Test func theFirstThreadIsNotMarkedAsJoined() {
    #expect(Timeline.items(twoThreads, threads: subjects).first == .marker(threadId: "a", subject: "Flat sale, next steps", date: 1, joined: false))
}

@Test func aLaterThreadIsMarkedAsJoined() {
    #expect(markers(Timeline.items(twoThreads, threads: subjects)).last == .marker(threadId: "b", subject: "Signing postponed", date: 3, joined: true))
}

@Test func picturesAreImages() {
    #expect(file("cellar.jpg", mime: "image/jpeg").isImage)
}

@Test func docsLeavePhotosOut() {
    let c = Conversation(id: "a", files: [file("plan.pdf", mime: "application/pdf"), file("door.png", mime: "image/png")])
    #expect(c.docs.map(\.filename) == ["plan.pdf"])
}

@Test func photosAreTheImages() {
    let c = Conversation(id: "a", files: [file("plan.pdf", mime: "application/pdf"), file("door.png", mime: "image/png")])
    #expect(c.photos.map(\.filename) == ["door.png"])
}

@Test func linksReadWithoutTheirScheme() {
    let l = SharedLink(url: "https://www.notary.example/rdv/", messageId: "m", from: "Nora", date: 0)
    #expect(l.short == "notary.example/rdv")
}

@Test func rowsKnowTheyGroupSeveralThreads() throws {
    let json = #"{"id":"t","number":"M-1","account_id":"a","subject":"s","snippet":"","participants":[],"last_from":"","last_date":0,"unread":false,"message_count":1,"has_attachments":false,"status":"todo","priority":0,"projects":[],"conversation_id":"t","conversation_kind":"person","conversation_threads":3}"#
    let t = try RPCCoding.decoder().decode(ThreadSummary.self, from: Data(json.utf8))
    #expect(t.groupsThreads)
}

@Test func olderDaemonsStillDecode() throws {
    let json = #"{"id":"t","number":"M-1","account_id":"a","subject":"s","snippet":"","participants":[],"last_from":"","last_date":0,"unread":false,"message_count":1,"has_attachments":false,"status":"todo","priority":0,"projects":[]}"#
    let t = try RPCCoding.decoder().decode(ThreadSummary.self, from: Data(json.utf8))
    #expect(!t.groupsThreads)
}

@Test func linkedFilesAlreadyAttachedShowOnce() {
    let linked = DocEntry.linked(id: "d1", title: "plan.pdf", url: "https://docs.example/plan", from: "Nora", date: 5)
    #expect(DocEntry.merged(attachments: [file("plan.pdf", mime: "application/pdf")], linked: [linked]).count == 1)
}

@Test func docsReadNewestFirst() {
    let linked = DocEntry.linked(id: "d1", title: "Budget sheet", url: "https://docs.example/b", from: "Nora", date: 5)
    #expect(DocEntry.merged(attachments: [file("plan.pdf", mime: "application/pdf")], linked: [linked]).first == linked)
}
