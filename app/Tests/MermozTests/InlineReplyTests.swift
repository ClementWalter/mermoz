/// r writes the reply in the chat's own composer, not the full window: the keyboard goes
/// there, single-key shortcuts stand aside while it has it, ⌘↩ sends, Esc leaves it with
/// the draft kept, and ⤢ (or the recipients) opens the full window with the draft.
import AppKit
@testable import MermozKit
import SwiftUI
import Testing
@testable import Mermoz

private func thread(_ i: Int) -> ThreadSummary {
    ThreadSummary(id: "t\(i)", number: "M-\(i)", accountId: "a", subject: "Keys \(i)", snippet: "", participants: [],
                  lastFrom: "Elliot", lastDate: Int64(1_000 - i), unread: false, messageCount: 1, hasAttachments: false,
                  status: "todo", priority: 0)
}

private func detail(_ i: Int) -> ThreadDetail {
    let m = Message(id: "m\(i)", threadId: "t\(i)", from: Address(name: "Elliot Moss", email: "elliot@example.com"),
                    to: [Address(name: "Me", email: "me@example.com")],
                    cc: [Address(name: "Nora", email: "nora@example.com"), Address(name: "Sam", email: "sam@example.com")],
                    bcc: nil, date: 1_000, subject: "Keys \(i)", bodyText: "Are the keys at the desk?", bodyNew: nil,
                    hasQuoted: nil, bodyClean: nil, hasHtml: false, isFromMe: false, attachments: [], auth: nil)
    return ThreadDetail(thread: thread(i), messages: [m])
}

/// The main window with the List, conversation 1 open in the pane.
@MainActor
private final class Window {
    let model = AppModel()
    let window: NSWindow
    let host: NSHostingView<AnyView>

    init() async {
        model.connection = .connected
        model.accounts = [Account(id: "a", email: "me@example.com", provider: "gmail",
                                  sync: SyncInfo(state: "idle", synced: 0, total: 0))]
        model.layout = .list
        model.threads = (0..<3).map(thread)
        model.totalThreads = 3
        host = NSHostingView(rootView: AnyView(RootView().environment(model)))
        window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 1_200, height: 800), styleMask: [.titled, .resizable],
                          backing: .buffered, defer: false)
        window.contentView = host
        model.open("t1")
        model.detail = detail(1)
        await settle()
    }

    func settle() async {
        host.layoutSubtreeIfNeeded()
        try? await Task.sleep(for: .milliseconds(100))
        host.layoutSubtreeIfNeeded()
    }

    func press(_ c: String) -> ThreadKeys.Routed {
        ThreadKeys.route(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                                          windowNumber: window.windowNumber, context: nil, characters: c,
                                          charactersIgnoringModifiers: c, isARepeat: false, keyCode: 0)!, model: model)
    }

    var typing: Bool { window.firstResponder?.isTyping ?? false }

    /// Esc, as the window hands it to whatever has the keyboard.
    func escape() {
        window.sendEvent(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                                          windowNumber: window.windowNumber, context: nil, characters: "\u{1b}",
                                          charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53)!)
    }

    /// r, then the time SwiftUI takes to move the keyboard (at most a second).
    func reply() async {
        _ = press("r")
        // Other test windows can hold SwiftUI's focus pass for a moment: ask again.
        for i in 1...30 where !typing {
            await settle()
            if i % 5 == 0 { _ = press("r") }
        }
    }
}

// MARK: r and Esc, one window at a time (SwiftUI moves the keyboard in one window at once)

@Suite(.serialized) @MainActor struct ReplyKeyboard {
@Test func rPutsTheKeyboardInTheReply() async {
    let w = await Window()
    await w.reply()
    await w.settle()
    #expect(w.typing)
}

@Test func rDoesNotOpenTheFullWindow() async {
    let w = await Window()
    await w.reply()
    await w.settle()
    #expect(w.model.compose == nil)
}

@Test func shortcutsStandAsideWhileTheReplyHasTheKeyboard() async {
    let w = await Window()
    await w.reply()
    await w.settle()
    #expect(w.press("e") == .skipped("typing"))
}



@Test func escLeavesTheReply() async {
    let w = await Window()
    await w.reply()
    await w.settle()
    w.escape()
    await w.settle()
    #expect(!w.typing)
}

@Test func escKeepsTheDraft() async {
    let w = await Window()
    await w.reply()
    await w.settle()
    var c = w.model.inlineReply(detail(1))
    c.body = "Yes, at the front desk."
    w.model.setInlineReply(c, for: detail(1))
    w.escape()
    await w.settle()
    #expect(w.model.inlineReply(detail(1)).body == "Yes, at the front desk.")
}

@Test func escInTheReplyKeepsTheConversationOpen() async {
    let w = await Window()
    await w.reply()
    await w.settle()
    w.escape()
    await w.settle()
    #expect(w.model.selectedThreadId == "t1")
}
}

// MARK: expand

@MainActor @Test func expandingOpensTheFullWindowWithTheDraft() {
    let model = AppModel()
    model.detail = detail(1)
    var c = model.inlineReply(detail(1))
    c.body = "Yes, at the front desk."
    model.setInlineReply(c, for: detail(1))
    model.expandInlineReply()
    #expect(model.compose?.body == "Yes, at the front desk.")
}

@MainActor @Test func theExpandedDraftKeepsItsRecipients() {
    let model = AppModel()
    model.accounts = [Account(id: "a", email: "me@example.com", provider: "gmail", sync: SyncInfo(state: "idle", synced: 0, total: 0))]
    model.detail = detail(1)
    model.expandInlineReply()
    #expect(model.compose?.cc == ["nora@example.com", "sam@example.com"])
}

@MainActor @Test func expandingEmptiesTheInlineReply() {
    let model = AppModel()
    model.detail = detail(1)
    var c = model.inlineReply(detail(1))
    c.body = "Yes."
    model.setInlineReply(c, for: detail(1))
    model.expandInlineReply()
    #expect(model.inlineReplies["t1"] == nil)
}

// MARK: send

@MainActor @Test func cmdReturnSendsTheReply() async {
    let model = AppModel()
    model.detail = detail(1)
    var sent: ComposeContext?
    model.sendOverride = { sent = $0; return true }
    var c = model.inlineReply(detail(1))
    c.body = "Yes, at the front desk."
    model.setInlineReply(c, for: detail(1))
    await model.sendInlineReply()
    #expect(sent?.body == "Yes, at the front desk.")
}

@MainActor @Test func theReplyGoesToTheConversationsThread() async {
    let model = AppModel()
    model.detail = detail(1)
    var sent: ComposeContext?
    model.sendOverride = { sent = $0; return true }
    var c = model.inlineReply(detail(1))
    c.body = "Yes."
    model.setInlineReply(c, for: detail(1))
    await model.sendInlineReply()
    #expect(sent?.threadId == "t1")
}

@MainActor @Test func aSentReplyLeavesAnEmptyComposer() async {
    let model = AppModel()
    model.detail = detail(1)
    model.sendOverride = { _ in true }
    var c = model.inlineReply(detail(1))
    c.body = "Yes."
    model.setInlineReply(c, for: detail(1))
    await model.sendInlineReply()
    #expect(model.inlineReply(detail(1)).body.isEmpty)
}

@MainActor @Test func aFailedSendKeepsTheDraft() async {
    let model = AppModel()
    model.detail = detail(1)
    model.sendOverride = { _ in false }
    var c = model.inlineReply(detail(1))
    c.body = "Yes."
    model.setInlineReply(c, for: detail(1))
    await model.sendInlineReply()
    #expect(model.inlineReply(detail(1)).body == "Yes.")
}

@MainActor @Test func anEmptyReplyIsNotSent() async {
    let model = AppModel()
    model.detail = detail(1)
    var sent = false
    model.sendOverride = { _ in sent = true; return true }
    await model.sendInlineReply()
    #expect(!sent)
}

// MARK: drafts and recipients

@MainActor @Test func eachConversationKeepsItsOwnDraft() {
    let model = AppModel()
    var c = model.inlineReply(detail(1))
    c.body = "For one."
    model.setInlineReply(c, for: detail(1))
    #expect(model.inlineReply(detail(2)).body.isEmpty)
}

@MainActor @Test func aDraftIsThereAfterVisitingAnotherConversation() {
    let model = AppModel()
    var c = model.inlineReply(detail(1))
    c.body = "For one."
    model.setInlineReply(c, for: detail(1))
    _ = model.inlineReply(detail(2))
    #expect(model.inlineReply(detail(1)).body == "For one.")
}

@MainActor @Test func theInlineReplyRepliesToAll() {
    let model = AppModel()
    model.accounts = [Account(id: "a", email: "me@example.com", provider: "gmail", sync: SyncInfo(state: "idle", synced: 0, total: 0))]
    let c = model.inlineReply(detail(1))
    #expect(c.to == ["elliot@example.com"] && c.cc == ["nora@example.com", "sam@example.com"])
}
