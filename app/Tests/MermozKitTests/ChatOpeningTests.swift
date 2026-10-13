/// Opening a conversation lays out a screenful: the latest messages with their thread
/// markers, and only the opening of a long bubble.
import Foundation
import Testing
@testable import MermozKit

private func message(_ id: String, thread: String, date: Int64) -> Message {
    Message(id: id, threadId: thread, from: Address(name: "Nora Lind", email: "nora@agency.example"), to: [], cc: [],
            bcc: nil, date: date, subject: "Flat sale", bodyText: "Hello", bodyNew: nil, hasQuoted: nil, bodyClean: nil,
            hasHtml: false, isFromMe: false, attachments: [], auth: nil)
}

private func thread(_ id: String) -> ThreadSummary {
    ThreadSummary(id: id, number: id, accountId: "a", subject: "Flat sale", snippet: "", participants: [], lastFrom: "",
                  lastDate: 0, unread: false, messageCount: 1, hasAttachments: false, status: "todo", priority: 0)
}

/// Five messages in thread a, then three in thread b: two markers.
private let long = Timeline.items((1...5).map { message("a\($0)", thread: "a", date: Int64($0) * 10) }
    + (6...8).map { message("b\($0)", thread: "b", date: Int64($0) * 10) }, threads: [thread("a"), thread("b")])

@Test func aShortConversationShowsEverything() {
    #expect(Timeline.recent(long, keep: 20).items == long)
}

@Test func aShortConversationHidesNothing() {
    #expect(Timeline.recent(long, keep: 20).hidden == 0)
}

@Test func aLongConversationKeepsTheLatestMessages() {
    let ids = Timeline.recent(long, keep: 4).items.map(\.id)
    #expect(ids == ["a5", "marker:b", "b6", "b7", "b8"])
}

@Test func aLongConversationCountsWhatItHides() {
    #expect(Timeline.recent(long, keep: 4).hidden == 4)
}

@Test func theFirstKeptMessageKeepsItsThreadMarker() {
    #expect(Timeline.recent(long, keep: 3).items.first?.id == "marker:b")
}

@Test func theOpeningOfALongBubbleStopsAtTenLines() {
    let text = (1...30).map { "line \($0)" }.joined(separator: "\n")
    #expect(BubbleParts.opening(text).split(separator: "\n").count == 10)
}

@Test func theOpeningOfALongBubbleStopsAt900Characters() {
    #expect(BubbleParts.opening(String(repeating: "word ", count: 1000)).count == 900)
}

@Test func aShortBubbleOpensWhole() {
    #expect(BubbleParts.opening("See you at nine.") == "See you at nine.")
}
