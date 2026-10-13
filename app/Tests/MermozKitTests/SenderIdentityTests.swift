/// One person, one name, one avatar: a sender whose From line varies in case or wording
/// ("aude walter", "Aude Walter") keeps one color, one name and one run in the chat.
import AppKit
import Testing
@testable import MermozKit

private func message(_ id: String, name: String, email: String = "aude@example.com", date: Int64) -> Message {
    Message(id: id, threadId: "t", from: Address(name: name, email: email), to: [], cc: [], bcc: nil, date: date, subject: "S",
            bodyText: "Message \(id) about the school trip on Friday.", bodyNew: nil, hasQuoted: nil, bodyClean: nil,
            hasHtml: false, isFromMe: false, attachments: [], auth: nil)
}

/// Three messages a minute apart from one address under two spellings of the name.
private let aude = [message("m1", name: "aude walter", date: 1_000),
                    message("m2", name: "aude walter", date: 61_000),
                    message("m3", name: "Aude Walter", email: "Aude@Example.com", date: 121_000)]

private func bubbles(_ messages: [Message], people: [ConversationPerson] = []) -> [BubbleSpec] {
    ChatRows.build(items: messages.map(TimelineItem.message), hidden: 0, summary: nil, expanded: [], shownInFull: [],
                   focused: nil, people: people, time: { _ in "09:41" })
        .rows.compactMap { if case let .bubble(b) = $0 { b } else { nil } }
}

private let palette: [NSColor] = [.systemBlue, .systemTeal, .systemPurple, .systemPink, .systemOrange, .systemGreen]

// MARK: color

@Test func theColorIgnoresTheCaseOfTheAddress() {
    #expect(SenderIdentity.colorIndex(SenderIdentity.key(Address(name: "aude walter", email: "aude@example.com")), count: 6)
        == SenderIdentity.colorIndex(SenderIdentity.key(Address(name: "Aude Walter", email: "Aude@Example.com")), count: 6))
}

@Test func theColorIgnoresTheNameVariant() {
    #expect(SenderIdentity.colorIndex(SenderIdentity.key(Address(name: "A. Walter", email: "aude@example.com")), count: 6)
        == SenderIdentity.colorIndex(SenderIdentity.key(Address(name: "Aude Walter", email: "aude@example.com")), count: 6))
}

@Test func everyBubbleOfOnePersonHasTheSameSenderKey() {
    #expect(Set(bubbles(aude).map(\.senderKey)) == ["aude@example.com"])
}

@MainActor @Test func aRunStartingUnderAnotherSpellingDrawsTheSameColor() {
    // Four hours apart: the third message starts a run of its own and draws its avatar.
    let later = [aude[0], aude[1], message("m3", name: "Aude Walter", email: "Aude@Example.com", date: 4 * 3_600_000)]
    let stream = ChatStreamView()
    stream.colors.avatars = palette
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 640, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView = stream
    stream.show(ChatRows.build(items: later.map(TimelineItem.message), hidden: 0, summary: nil, expanded: [], shownInFull: [],
                               focused: nil, time: { _ in "09:41" }).rows, conversation: "c", messages: 3)
    stream.documentView?.layoutSubtreeIfNeeded()
    #expect(stream.avatarColor("m3") == stream.avatarColor("m1"))
}

// MARK: name

@Test func theBestCasedVariantIsTheNameShown() {
    #expect(Set(bubbles(aude).map(\.sender)) == ["Aude Walter"])
}

@Test func theDirectoryNameWinsOverTheFromLine() {
    let person = ConversationPerson(name: "Aude Walter-Martin", email: "aude@example.com", title: "", company: "", messages: 3)
    #expect(bubbles(aude, people: [person])[0].sender == "Aude Walter-Martin")
}

@Test func aNameInCapitalsGivesWayToMixedCase() {
    let shouting = [message("m1", name: "AUDE WALTER", date: 1_000), message("m2", name: "Aude Walter", date: 2_000)]
    #expect(SenderIdentity.names(shouting, people: [])["aude@example.com"] == "Aude Walter")
}

@Test func theMoreFrequentOfTwoMixedCaseVariantsWins() {
    let variants = [message("m1", name: "Aude W.", date: 1_000), message("m2", name: "Aude Walter", date: 2_000),
                    message("m3", name: "Aude Walter", date: 3_000)]
    #expect(SenderIdentity.names(variants, people: [])["aude@example.com"] == "Aude Walter")
}

@Test func theInitialsComeFromTheCanonicalName() {
    #expect(SenderIdentity.initials("aude walter") == "AW")
}

// MARK: runs

@Test func messagesFromOnePersonUnderTwoSpellingsShareOneRun() {
    #expect(bubbles(aude).map(\.firstOfRun) == [true, false, false])
}

@Test func anotherPersonStartsANewRun() {
    let two = [aude[0], message("m2", name: "Aude Walter", email: "paul@example.com", date: 61_000)]
    #expect(bubbles(two).map(\.firstOfRun) == [true, true])
}
