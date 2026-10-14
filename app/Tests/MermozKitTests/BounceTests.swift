/// A message that didn't reach someone says so under its bubble, in red, with "Fix
/// address"; the bounce that reported it is no bubble. A bounce for a message out of
/// view is one quiet line.
import AppKit
import Foundation
import Testing
@testable import MermozKit

private func message(_ id: String, _ body: String, mine: Bool = false, date: Int64) -> Message {
    Message(id: id, threadId: "t", from: Address(name: mine ? "Me" : "Mail Delivery Subsystem",
                                                 email: mine ? "me@x.com" : "mailer-daemon@googlemail.com"),
            to: [Address(name: "Nora", email: "nora@exampel.com")], cc: [], bcc: nil, date: date, subject: "Keys",
            bodyText: body, bodyNew: body, hasQuoted: nil, bodyClean: nil, hasHtml: false, isFromMe: mine, attachments: [],
            auth: nil)
}

private let failed: Message = {
    var m = message("m1", "The keys are at the desk.", mine: true, date: 1)
    m.deliveryFailure = DeliveryFailure(recipient: "nora@exampel.com", status: "5.1.1", reason: "address doesn't exist",
                                        bounceMessageId: "b1")
    return m
}()

private let bounce: Message = {
    var m = message("b1", "Address not found\nYour message wasn't delivered to nora@exampel.com.", date: 2)
    m.isBounce = true
    return m
}()

private func rows(_ messages: [Message]) -> [ChatRowSpec] {
    ChatRows.build(items: messages.map(TimelineItem.message), hidden: 0, summary: nil, expanded: [], shownInFull: [],
                   focused: nil, time: { _ in "09:41" }).rows
}

private func bubbles(_ r: [ChatRowSpec]) -> [BubbleSpec] { r.compactMap { if case let .bubble(b) = $0 { b } else { nil } } }
private func markers(_ r: [ChatRowSpec]) -> [MarkerSpec] { r.compactMap { if case let .marker(m) = $0 { m } else { nil } } }

// MARK: rows

@Test func theBounceThatReportedAFailureIsNoBubble() {
    #expect(bubbles(rows([failed, bounce])).map(\.id) == ["m1"])
}

@Test func theFailedMessageSaysWhoItDidNotReach() {
    #expect(bubbles(rows([failed, bounce]))[0].failure?.line == "Not delivered to nora@exampel.com: address doesn't exist")
}

@Test func aBounceForAMessageOutOfViewIsOneLine() {
    #expect(markers(rows([bounce])).map(\.line) == ["A message wasn't delivered · Address not found"])
}

@Test func aBounceForAMessageOutOfViewIsNoBubble() {
    #expect(bubbles(rows([bounce])).isEmpty)
}

@Test func aDeliveredMessageHasNoFailure() {
    #expect(bubbles(rows([message("m2", "Hello", mine: true, date: 1)]))[0].failure == nil)
}

// MARK: decoding

@Test func theDaemonsFailureDecodes() throws {
    let json = """
    {"id":"m1","thread_id":"t","from":{"name":"Me","email":"me@x.com"},"to":[],"cc":[],"date":1,"subject":"S",
     "body_text":"x","has_html":false,"is_from_me":true,"attachments":[],
     "delivery_failure":{"recipient":"nora@exampel.com","status":"5.1.1","reason":"address doesn't exist","bounce_message_id":"b1"}}
    """
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    #expect(try d.decode(Message.self, from: Data(json.utf8)).deliveryFailure?.bounceMessageId == "b1")
}

@Test func aBounceDecodes() throws {
    let json = """
    {"id":"b1","thread_id":"t","from":{"name":"","email":"mailer-daemon@x.com"},"to":[],"cc":[],"date":1,"subject":"S",
     "body_text":"x","has_html":false,"is_from_me":false,"attachments":[],"is_bounce":true,"is_auto_reply":false}
    """
    let d = JSONDecoder()
    d.keyDecodingStrategy = .convertFromSnakeCase
    #expect(try d.decode(Message.self, from: Data(json.utf8)).isBounce == true)
}

// MARK: drawn

@MainActor
private final class Shown {
    let stream = ChatStreamView()
    let window: NSWindow
    var fixed: [String] = []

    init(_ messages: [Message], width: CGFloat = 640) {
        window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: width, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
        window.contentView = stream
        stream.actions.fixAddress = { [weak self] in self?.fixed.append($0) }
        stream.show(rows(messages), conversation: "c", messages: messages.count)
        stream.layoutSubtreeIfNeeded()
        stream.documentView?.layoutSubtreeIfNeeded()
    }
}

@MainActor @Test func theFailureLineSitsUnderTheBubble() {
    let row = Shown([failed, bounce]).stream.bubbleRow("m1")!
    #expect(row.failureRect.minY >= row.bubbleRect.maxY)
}

@MainActor @Test(arguments: [360.0, 640.0])
func theFailureLineStaysInItsRow(width: Double) {
    let row = Shown([failed, bounce], width: width).stream.bubbleRow("m1")!
    #expect(row.bounds.contains(row.failureRect) && row.bounds.contains(row.fixButton!.frame))
}

@MainActor @Test func aFailedRowDoesNotOverlapTheNext() {
    let s = Shown([failed, bounce, message("m3", "Did you get it?", mine: true, date: 3)])
    let f = s.stream.rowViews.map(\.frame)
    #expect(zip(f, f.dropFirst()).allSatisfy { $0.maxY <= $1.minY })
}

@MainActor @Test func fixAddressAsksToFixThatMessage() {
    let s = Shown([failed, bounce])
    s.stream.bubbleRow("m1")!.fixButton?.performClick(nil)
    #expect(s.fixed == ["m1"])
}

@MainActor @Test func aDeliveredRowShowsNoFixButton() {
    let row = Shown([message("m2", "Hello", mine: true, date: 1)]).stream.bubbleRow("m2")!
    #expect(row.fixButton?.isHidden ?? true)
}

@MainActor @Test func theFailedBubbleTellsVoiceOver() {
    let row = Shown([failed, bounce]).stream.bubbleRow("m1")!
    #expect(row.accessibilityLabel()?.hasSuffix("Not delivered to nora@exampel.com: address doesn't exist") == true)
}
