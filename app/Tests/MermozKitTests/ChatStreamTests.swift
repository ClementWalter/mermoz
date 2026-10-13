/// The AppKit chat stream driven in-process: rows built from a conversation, then shown
/// in an offscreen window and clicked or typed into, as a reader would.
import AppKit
import Testing
@testable import MermozKit

private let dunning = """
Madame, Monsieur,

Sauf erreur ou omission de notre part, votre compte présente un solde débiteur.

Veuillez trouver ci-joint un avis de relance N° %@.

Veuillez agréer Madame, Monsieur, nos salutations distinguées.

Cordialement.

Service Administration des Ventes
"""

private func message(_ id: String, body: String, from: String = "recouvrement@lpcr.example", mine: Bool = false,
                     date: Int64 = 1_000, files: Int = 0) -> Message {
    Message(id: id, threadId: "t1", from: Address(name: "", email: from), to: [], cc: [], bcc: nil, date: date,
            subject: "Avis de relance", bodyText: body, bodyNew: nil, hasQuoted: nil, bodyClean: nil, hasHtml: false,
            isFromMe: mine,
            attachments: (0..<files).map { Attachment(id: "\(id)-a\($0)", filename: "avis.pdf", mime: "application/pdf", size: 107_000) },
            auth: nil)
}

private let letters = [message("m1", body: String(format: dunning, "104325"), files: 1),
                       message("m2", body: String(format: dunning, "104326"), date: 2_000, files: 1)]

private func rows(_ messages: [Message], expanded: Set<String> = [], focused: String? = nil, hidden: Int = 0) -> [ChatRowSpec] {
    ChatRows.build(items: messages.map(TimelineItem.message), hidden: hidden, summary: nil, expanded: expanded,
                   shownInFull: [], focused: focused, time: { _ in "09:41" }).rows
}

private func bubble(_ row: ChatRowSpec) -> BubbleSpec? {
    if case let .bubble(b) = row { b } else { nil }
}

// MARK: near-identical messages

@Test func aDunningLetterRepeatedButForItsNumberIsANearDuplicate() {
    let d = NearDuplicate.differences(previous: String(format: dunning, "104325"), current: String(format: dunning, "104326"))
    #expect(d == ["N° 104326."])
}

@Test func theNoteNamesWhatDiffers() {
    #expect(NearDuplicate.note(["N° 104326."]) == "Almost the same as the previous message · differs in: N° 104326")
}

@Test func differentLettersAreNotNearDuplicates() {
    #expect(NearDuplicate.differences(previous: String(format: dunning, "104325"),
                                      current: "Thanks for the photos from Saturday, see you next week at the station with the kids.")
        == nil)
}

@Test func shortRepliesAreNeverMarked() {
    #expect(NearDuplicate.differences(previous: "Thanks!", current: "Thanks a lot!") == nil)
}

@Test func theSecondNearDuplicateCarriesTheNote() {
    #expect(bubble(rows(letters)[1])?.note == "Almost the same as the previous message · differs in: N° 104326")
}

@Test func theFirstOfTwoNearDuplicatesHasNoNote() {
    #expect(bubble(rows(letters)[0])?.note == nil)
}

@Test func aNearDuplicateFromSomeoneElseHasNoNote() {
    let other = message("m2", body: String(format: dunning, "104326"), from: "billing@other.example", date: 2_000)
    #expect(bubble(rows([letters[0], other])[1])?.note == nil)
}

// MARK: compact by default

@Test func anAutomatedLetterWithAFileIsACompactBubble() {
    #expect(bubble(rows(letters)[0]) != nil)
}

@Test func theCompactBubbleCountsItsFile() {
    #expect(bubble(rows(letters)[0])?.files == 1)
}

@Test func aShortLetterWithBlankLinesIsNotClamped() {
    #expect(bubble(rows(letters)[0])?.content.clamped == false)
}

@Test func onlyAnOpenedMessageShowsAsTheWholeEmail() {
    #expect(bubble(rows(letters, expanded: ["m2"])[1]) == nil)
}

@Test func theOtherMessageStaysCompactWhenOneIsOpened() {
    #expect(bubble(rows(letters, expanded: ["m2"])[0]) != nil)
}

// MARK: the stream, driven

/// A stream in an offscreen window, laid out, with what it asked the app recorded.
@MainActor
private final class Harness {
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 700, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
    var toggled: [String] = []
    var shownMore: [String] = []
    var earlier = 0
    var moves: [Int] = []
    var activated = 0

    init(_ rows: [ChatRowSpec], conversation: String = "c1") {
        var a = ChatActions()
        a.toggle = { [unowned self] in toggled.append($0) }
        a.showMore = { [unowned self] in shownMore.append($0) }
        a.showEarlier = { [unowned self] in earlier += 1 }
        a.moveFocus = { [unowned self] in moves.append($0) }
        a.activate = { [unowned self] in activated += 1 }
        stream.actions = a
        window.contentView = stream
        stream.frame = window.contentView!.bounds
        show(rows, conversation: conversation)
    }

    func show(_ rows: [ChatRowSpec], conversation: String = "c1") {
        stream.show(rows, conversation: conversation, messages: rows.count)
        stream.layoutSubtreeIfNeeded()
        stream.documentView?.layoutSubtreeIfNeeded()
    }

    /// Clicks a point of the document view, in its own coordinates.
    func click(_ p: CGPoint, in view: NSView) {
        let inWindow = view.convert(p, to: nil)
        guard let e = NSEvent.mouseEvent(with: .leftMouseDown, location: inWindow, modifierFlags: [], timestamp: 0,
                                         windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1,
                                         pressure: 1) else { return }
        view.hitTest(view.superview?.convert(p, from: view) ?? p)?.mouseDown(with: e)
    }
}

private let chat = [message("a", body: "Is Saturday still on?", from: "nora@friends.example", date: 1_000),
                    message("b", body: "Yes, see you at nine.", from: "me@example.com", mine: true, date: 2_000)]

@MainActor @Test func clickingABubbleOutsideItsTextOpensTheWholeEmail() {
    let h = Harness(rows(chat))
    let f = h.stream.bubbleFrame("a")!
    h.click(CGPoint(x: f.minX + 3, y: f.minY + 3), in: h.stream.documentView!)
    #expect(h.toggled == ["a"])
}

@MainActor @Test func downArrowMovesTheFocusDown() {
    let h = Harness(rows(chat))
    h.stream.press(keyCode: 125)
    #expect(h.moves == [1])
}

@MainActor @Test func upArrowMovesTheFocusUp() {
    let h = Harness(rows(chat))
    h.stream.press(keyCode: 126)
    #expect(h.moves == [-1])
}

@MainActor @Test func enterOpensTheMessageInFocus() {
    let h = Harness(rows(chat, focused: "a"))
    h.stream.press(keyCode: 36)
    #expect(h.activated == 1)
}

@MainActor @Test func theEarlierRowCountsWhatIsLeftOut() {
    let h = Harness(rows(chat, hidden: 3))
    #expect(h.stream.earlierTitle == "Show 3 earlier messages")
}

@MainActor @Test func pressingTheEarlierRowAsksForThem() {
    let h = Harness(rows(chat, hidden: 3))
    h.stream.pressEarlier()
    #expect(h.earlier == 1)
}

@MainActor @Test func showMoreAsksForTheWholeText() {
    let long = message("l", body: (1...30).map { "Line \($0) of the minutes." }.joined(separator: "\n"))
    let h = Harness(rows([long]))
    h.stream.pressShowMore("l")
    #expect(h.shownMore == ["l"])
}

@MainActor @Test func theirBubbleSitsOnTheLeft() {
    let h = Harness(rows(chat))
    #expect(h.stream.bubbleFrame("a")!.minX < 100)
}

@MainActor @Test func myBubbleSitsOnTheRight() {
    let h = Harness(rows(chat))
    #expect(h.stream.bubbleFrame("b")!.maxX > 600)
}

@MainActor @Test func aShortConversationStartsAtTheTop() {
    let h = Harness(rows(chat))
    #expect(h.stream.bubbleFrame("a")!.minY < 60)
}

@MainActor @Test func aBubbleTextViewDrawsItsContainer() {
    let h = Harness(rows(chat))
    let t = h.stream.bubbleText("a")!
    #expect(t.textContainer?.textView === t)
}

@MainActor @Test func aBubbleShowsItsWholeText() {
    let h = Harness(rows(chat))
    #expect(h.stream.bubbleText("b")?.textStorage?.string == "Yes, see you at nine.")
}

@MainActor @Test func theNextConversationReusesTheBubbleViews() {
    let h = Harness(rows(chat))
    let before = h.stream.rowViews.map(ObjectIdentifier.init)
    h.show(rows([message("c", body: "Lunch?", from: "sam@work.example"), message("d", body: "Sure.", from: "me@example.com",
                                                                             mine: true, date: 2_000)]), conversation: "c2")
    #expect(h.stream.rowViews.map(ObjectIdentifier.init) == before)
}

@MainActor @Test func aBubbleSaysWhoItIsFrom() {
    let h = Harness(rows(chat))
    let row = h.stream.rowViews.first { ($0 as? NSView)?.accessibilityLabel()?.hasPrefix("Message from") == true }
    #expect(row?.accessibilityLabel() == "Message from nora@friends.example")
}
