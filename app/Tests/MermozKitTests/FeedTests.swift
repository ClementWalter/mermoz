/// The Feed: cards decode as mermozd sends them, and the view's rules (primary action,
/// focus, header, Later, calendar file, which words to ask for) hold.
import Foundation
import Testing
@testable import MermozKit

private let feedJSON = #"""
{"sections":[{"key":"now","title":"Now","cards":[{"id":"thr_1","kind":"decision","section":"now",
"thread":{"id":"thr_1","number":"M-1","account_id":"acc","subject":"Carrelage","snippet":"","participants":[],
"last_from":"Marie Lefèvre","last_date":1791700000000,"unread":true,"message_count":1,"has_attachments":false,
"status":"unread","priority":0,"projects":["Kitchen"],"due":null,"summary":null,"next_action":null,"suggestion":null},
"members":[],"title":"Choose the kitchen tiles","why":null,"reasons":["The mail asks for it by Fri 16 Oct."],
"due":"2026-10-16","due_time":"18:00","amount":null,"from_backlog":false,"waiting_days":null,
"texts":{"title":"Choose the kitchen tiles","why":"Marie needs it Friday.","draft":"","summary":"s","points":[],
"options":[{"label":"Gris","detail":"42 €","recommended":true,"reason":"sooner","reply":"On part sur le gris."},
{"label":"Terre cuite","detail":"58 €","recommended":false,"reason":"","reply":"On part sur la terre cuite."}]},
"texts_wanted":false}],"more":2}],"open":12,"judged":10,"sorting":true}
"""#

private func decodeFeed() throws -> Feed {
    try RPCCoding.decoder().decode(Feed.self, from: Data(feedJSON.utf8))
}

private func thread(_ id: String) -> ThreadSummary {
    ThreadSummary(id: id, number: "M-1", accountId: "acc", subject: "S", snippet: "", participants: [],
                  lastFrom: "Marie", lastDate: 0, unread: false, messageCount: 1, hasAttachments: false,
                  status: "todo", priority: 0)
}

private func card(_ id: String, _ kind: FeedKind, section: String = "now", members: [ThreadSummary] = [],
                  texts: FeedTexts? = nil, due: String? = nil, wanted: Bool = false) -> FeedCard {
    FeedCard(id: id, kind: kind, section: section, thread: thread(id), members: members, title: "T",
             due: due, texts: texts, textsWanted: wanted)
}

private var utcCalendar: Calendar {
    var c = Calendar(identifier: .gregorian)
    c.timeZone = TimeZone(identifier: "UTC")!
    return c
}

// MARK: decoding

@Test func feedDecodesItsCards() throws {
    #expect(try decodeFeed().cards.count == 1)
}

@Test func cardDecodesItsKind() throws {
    #expect(try decodeFeed().cards[0].kind == .decision)
}

@Test func cardDecodesSnakeCaseDueTime() throws {
    #expect(try decodeFeed().cards[0].dueTime == "18:00")
}

@Test func cardDecodesTheAIOptions() throws {
    #expect(try decodeFeed().cards[0].texts?.options.count == 2)
}

@Test func sectionDecodesWhatItLeftOut() throws {
    #expect(try decodeFeed().sections[0].more == 2)
}

@Test func feedDecodesSorting() throws {
    #expect(try decodeFeed().sorting)
}

@Test func feedChangedEventDecodes() {
    #expect(decodeEvent(["kind": "feed_changed"]) == .feedChanged)
}

// MARK: primary action

@Test func replyOpensTheDraft() {
    let c = card("a", .reply, texts: FeedTexts(draft: "Hi Thomas"))
    #expect(FeedActions.primary(c) == .draft("Hi Thomas"))
}

@Test func replyWithoutWordsOpensAnEmptyDraft() {
    #expect(FeedActions.primary(card("a", .reply)) == .draft(""))
}

@Test func decisionOpensTheRecommendedReply() {
    let texts = FeedTexts(options: [FeedChoice(label: "A", reply: "a"), FeedChoice(label: "B", recommended: true, reply: "b")])
    #expect(FeedActions.primary(card("a", .decision, texts: texts)) == .draft("b"))
}

@Test func eventWithADateGoesToTheCalendar() {
    #expect(FeedActions.primary(card("a", .event, due: "2026-10-16")) == .addToCalendar)
}

@Test func eventWithoutADateOpensTheMail() {
    #expect(FeedActions.primary(card("a", .event)) == .open)
}

@Test func cleanUpArchivesEveryMember() {
    let c = card("batch:cleanup", .cleanup, members: [thread("x"), thread("y")])
    #expect(FeedActions.primary(c) == .archive(["x", "y"]))
}

@Test func waitingCardSaysFollowUp() {
    #expect(FeedActions.primaryLabel(card("w", .waiting)) == "Follow up")
}

@Test(arguments: FeedKind.allCases)
func cardLabelsUsePlainWords(kind: FeedKind) {
    let jargon = ["nudge", "clean-up", "digest", "fyi", "waiting on", "triage"]
    let words = (kind.label + " " + FeedActions.primaryLabel(card("x", kind))).lowercased()
    #expect(!jargon.contains { words.contains($0) })
}

@Test func batchButtonCountsItsMails() {
    let c = card("batch:cleanup", .cleanup, members: [thread("x"), thread("y")])
    #expect(FeedActions.primaryLabel(c) == "Archive 2")
}

@Test func aSingleCleanUpCardSaysArchive() {
    #expect(FeedActions.primaryLabel(card("a", .cleanup)) == "Archive")
}

@Test func aSingleCleanUpCardArchivesItsThread() {
    #expect(FeedActions.primary(card("a", .cleanup)) == .archive(["a"]))
}

@Test func paymentOpensTheMail() {
    #expect(FeedActions.primary(card("a", .payment)) == .open)
}

@Test(arguments: [(1, "a"), (2, "b")])
func numberKeysPickAnOption(n: Int, reply: String) {
    let texts = FeedTexts(options: [FeedChoice(label: "A", reply: "a"), FeedChoice(label: "B", reply: "b")])
    #expect(FeedActions.option(n, of: card("d", .decision, texts: texts)) == .draft(reply))
}

@Test func numberKeysPastTheOptionsDoNothing() {
    let texts = FeedTexts(options: [FeedChoice(label: "A", reply: "a")])
    #expect(FeedActions.option(3, of: card("d", .decision, texts: texts)) == nil)
}

@Test func numberKeysDoNothingOnOtherCards() {
    #expect(FeedActions.option(1, of: card("r", .reply)) == nil)
}

// MARK: focus

@Test(arguments: [
    (nil, 1, "a"), (nil, -1, "c"), ("a", 1, "b"), ("c", 1, "c"), ("b", -1, "a"),
] as [(String?, Int, String)])
func focusSteps(current: String?, delta: Int, want: String) {
    #expect(FeedFocus.step(["a", "b", "c"], from: current, by: delta) == want)
}

@Test(arguments: [("a", "b"), ("b", "c"), ("c", "b")])
func focusMovesOnWhenACardLeaves(gone: String, want: String) {
    #expect(FeedFocus.after(removing: gone, from: ["a", "b", "c"]) == want)
}

@Test func focusIsNilWhenTheLastCardLeaves() {
    #expect(FeedFocus.after(removing: "a", from: ["a"]) == nil)
}

// MARK: header

private func feed(now: Int, today: Int) -> Feed {
    var sections: [FeedSection] = []
    if now > 0 { sections.append(FeedSection(key: "now", title: "Now", cards: (0..<now).map { card("n\($0)", .reply) })) }
    if today > 0 { sections.append(FeedSection(key: "today", title: "Today", cards: (0..<today).map { card("t\($0)", .reply) })) }
    return Feed(sections: sections)
}

@Test(arguments: [
    (3, 2, "3 things need you now and 2 more today."),
    (1, 0, "1 thing needs you now."),
    (0, 4, "4 things need you today."),
    (0, 0, "Nothing needs you. New mail lands here as it arrives."),
])
func headerSaysWhatIsLeft(now: Int, today: Int, want: String) {
    #expect(FeedHeader.line(feed(now: now, today: today)) == want)
}

@Test func headerIsCalmWhenOnlyLaterWorkIsLeft() {
    let f = Feed(sections: [FeedSection(key: "week", title: "This week", cards: [card("w", .intro)])])
    #expect(FeedHeader.line(f) == "Nothing urgent today.")
}

// MARK: Later

@Test func laterEndsTomorrowAtEight() {
    let now = Date(timeIntervalSince1970: 1_791_787_200) // 2026-10-12 06:40 UTC
    let back = FeedLater.tomorrowMorning(after: now, calendar: utcCalendar)
    #expect(back == Date(timeIntervalSince1970: 1_791_878_400)) // 2026-10-13 08:00 UTC
}

@Test func laterSendsMilliseconds() {
    #expect(FeedLater.millis(Date(timeIntervalSince1970: 1.5)) == 1500)
}

// MARK: words

@Test(arguments: [
    ("2026-10-12", "today"), ("2026-10-13", "tomorrow"), ("2026-10-11", "yesterday"), ("2026-10-16", "Fri 16 Oct"),
])
func dueInWords(date: String, want: String) {
    let monday = Date(timeIntervalSince1970: 1_791_787_200) // 2026-10-12 06:40 UTC
    #expect(FeedWords.due(date, today: monday, calendar: utcCalendar) == want)
}

// MARK: calendar file

@Test func calendarEventStartsAtTheWrittenTime() {
    let ics = CalendarFile.ics(title: "Réunion", date: "2026-10-14", time: "18:00", uid: "thr_1")!
    #expect(ics.contains("DTSTART:20261014T180000"))
}

@Test func calendarEventLastsAnHour() {
    let ics = CalendarFile.ics(title: "Réunion", date: "2026-10-14", time: "18:30", uid: "thr_1")!
    #expect(ics.contains("DTEND:20261014T193000"))
}

@Test func calendarEventWithoutTimeIsAllDay() {
    let ics = CalendarFile.ics(title: "Brocante", date: "2026-10-18", time: nil, uid: "thr_2")!
    #expect(ics.contains("DTSTART;VALUE=DATE:20261018"))
}

@Test func calendarTitleEscapesCommas() {
    let ics = CalendarFile.ics(title: "Réunion, salle 12", date: "2026-10-14", time: nil, uid: "u")!
    #expect(ics.contains("SUMMARY:Réunion\\, salle 12"))
}

@Test func calendarRefusesABadDate() {
    #expect(CalendarFile.ics(title: "x", date: "soon", time: nil, uid: "u") == nil)
}

// MARK: which words to ask for

@Test func onlyWantedCardsAskForWords() {
    let ids = FeedTextsRequest.ids(visible: [card("a", .reply, wanted: true), card("b", .reply)], asked: [])
    #expect(ids == ["a"])
}

@Test func cardsAreAskedOnce() {
    #expect(FeedTextsRequest.ids(visible: [card("a", .reply, wanted: true)], asked: ["a"]).isEmpty)
}

@Test func batchesAskForTheirMembers() {
    let c = card("batch:reading", .reading, members: [thread("x"), thread("y")], wanted: true)
    #expect(FeedTextsRequest.ids(visible: [c], asked: []) == ["x", "y"])
}

// MARK: who the card is about

@Test func cardIsAboutTheLastSender() {
    #expect(card("a", .reply).person == "Marie")
}

@Test func cardIWroteLastIsAboutTheOtherPerson() {
    var c = card("a", .promise)
    c.thread.lastFrom = "You"
    c.thread.participants = ["Inès Garnier", "You"]
    #expect(c.person == "Inès Garnier")
}

// MARK: reading pane

@Test func aCardShowsItsOwnThread() {
    #expect(FeedSelection.thread(of: card("thr_9", .reply)) == "thr_9")
}

@Test func aBatchShowsItsLatestMail() {
    let c = card("batch:cleanup", .cleanup, members: [thread("latest"), thread("older")])
    #expect(FeedSelection.thread(of: c) == "latest")
}

@Test func jMovesTheReadingPaneToTheNextCard() {
    let f = Feed(sections: [FeedSection(key: "now", title: "Now", cards: [card("a", .reply), card("b", .decision)])])
    let next = FeedFocus.step(f.cards.map(\.id), from: "a", by: 1)
    #expect(f.card(next).map(FeedSelection.thread) == "b")
}

@Test func feedDecodesWhyTheAIIsOnHold() throws {
    let f = try RPCCoding.decoder().decode(Feed.self, from: Data(#"{"sections":[],"open":0,"judged":0,"sorting":false,"paused":"credits"}"#.utf8))
    #expect(f.paused == "credits")
}

// MARK: keys

@Test(arguments: ["d", "\u{7f}", "\u{f728}", "#"])
func deleteKeysDelete(key: String) {
    #expect(FeedKeyMap.action(key) == .delete)
}

@Test func theFeedListensForD() {
    #expect(FeedKeyMap.characters.contains("d"))
}

@Test func archiveStaysOnA() {
    #expect(FeedKeyMap.action("a") == .archive)
}

@Test func shiftLPicksADay() {
    #expect(FeedKeyMap.action("l", shift: true) == .laterPickDay)
}

// MARK: duplicate lines

@Test func aWhyRepeatingTheGistIsADuplicate() {
    #expect(FeedWords.nearDuplicate("Spring sale: 30% off all shoes until Sunday.", "Spring sale, 30% off all shoes until Sunday!"))
}

@Test func aWhyContainedInTheGistIsADuplicate() {
    #expect(FeedWords.nearDuplicate("30% off all shoes", "Spring sale: 30% off all shoes until Sunday."))
}

@Test func differentLinesAreKept() {
    #expect(!FeedWords.nearDuplicate("Marie needs your pick by Friday.", "On part sur le grès cérame gris ou la terre cuite ?"))
}

@Test func aMissingWhyIsNoDuplicate() {
    #expect(!FeedWords.nearDuplicate(nil, "Anything"))
}
