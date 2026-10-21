/// Unsent drafts sit in the conversation at the time they were saved, as their own rows.
import Foundation
import Testing
@testable import MermozKit

private func message(_ id: String, date: Int64) -> Message {
    Message(id: id, threadId: "t", from: Address(name: "Ana", email: "ana@example.com"), to: [], cc: [],
            bcc: nil, date: date, subject: "Flat sale", bodyText: "Hello", bodyNew: nil, hasQuoted: nil, bodyClean: nil,
            hasHtml: false, isFromMe: false, attachments: [], auth: nil)
}

private func draft(_ id: String, updated: Int64) -> Draft {
    Draft(id: id, accountId: "a", threadId: "t", to: [], cc: [], subject: "", body: "Draft text", state: "draft",
          origin: "gmail", created: updated, updated: updated, attachments: [])
}

private func ids(_ items: [TimelineItem]) -> [String] { items.map(\.id) }

@Test func aDraftSitsBetweenTheMessagesAroundItsSaveTime() {
    let items = Timeline.items([message("m1", date: 10), message("m2", date: 30)], threads: [], drafts: [draft("d", updated: 20)])
    #expect(ids(items) == ["m1", "draft:d", "m2"])
}

@Test func aDraftSavedLastEndsTheConversation() {
    let items = Timeline.items([message("m1", date: 10)], threads: [], drafts: [draft("d", updated: 20)])
    #expect(ids(items) == ["m1", "draft:d"])
}

@Test func draftsKeepTheirOwnOrder() {
    let items = Timeline.items([message("m1", date: 10)], threads: [], drafts: [draft("late", updated: 40), draft("early", updated: 20)])
    #expect(ids(items) == ["m1", "draft:early", "draft:late"])
}

@Test func aDraftIsAHostedRowInTheChat() {
    let items = Timeline.items([message("m1", date: 10)], threads: [], drafts: [draft("d", updated: 20)])
    let built = ChatRows.build(items: items, hidden: 0, summary: nil, expanded: [], shownInFull: [], focused: nil) { _ in "" }
    #expect(built.hosted.keys.contains("draft:d:20"))
}

@Test func aMessageAfterADraftStartsANewRun() {
    let items = Timeline.items([message("m1", date: 10), message("m2", date: 30)], threads: [], drafts: [draft("d", updated: 20)])
    #expect(ChatRuns.startsRun(items).contains("m2"))
}
