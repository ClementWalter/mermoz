/// Single-key shortcuts run wherever the focus is, in the List, the Board and the Feed:
/// the window's real key path (ThreadKeys.route, as the app's key monitor calls it) with
/// the whole window on screen offscreen, a card or row selected and its reading pane open.
import AppKit
@testable import MermozKit
import SwiftUI
import Testing
@testable import Mermoz

private func thread(_ i: Int) -> ThreadSummary {
    ThreadSummary(id: "t\(i)", number: "M-\(i)", accountId: "a", subject: "Subject \(i)", snippet: "", participants: [],
                  lastFrom: "Sender", lastDate: Int64(1_000 - i), unread: false, messageCount: 1, hasAttachments: false,
                  status: "todo", priority: 0)
}

/// The main window in one layout, the second thread selected and shown in the pane.
@MainActor
private final class Window {
    let model = AppModel()
    let window: NSWindow
    let host: NSHostingView<AnyView>

    init(_ layout: LayoutMode) async {
        model.connection = .connected
        model.accounts = [Account(id: "a", email: "me@example.com", provider: "gmail",
                                  sync: SyncInfo(state: "idle", synced: 0, total: 0))]
        model.statuses = [Status(key: "todo", name: "To do", order: 0, color: "#000000"),
                          Status(key: "done", name: "Done", order: 1, color: "#000000")]
        model.layout = layout
        model.threads = (0..<5).map(thread)
        model.totalThreads = 5
        if layout == .feed {
            let card = FeedCard(id: "c1", kind: .reply, section: "now", thread: thread(1), title: "Subject 1")
            model.feed = Feed(sections: [FeedSection(key: "now", title: "Now", cards: [card])])
            model.feedFocus = "c1"
        }
        host = NSHostingView(rootView: AnyView(RootView().environment(model)))
        window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 1_200, height: 800), styleMask: [.titled, .resizable],
                          backing: .buffered, defer: false)
        window.contentView = host
        model.open("t1")
        model.detail = ThreadDetail.placeholder(thread(1))
        await settle()
    }

    func settle() async {
        host.layoutSubtreeIfNeeded()
        try? await Task.sleep(for: .milliseconds(80))
        host.layoutSubtreeIfNeeded()
    }

    func key(_ c: String, shift: Bool = false) -> NSEvent {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: shift ? .shift : [], timestamp: 0,
                         windowNumber: window.windowNumber, context: nil, characters: c, charactersIgnoringModifiers: c,
                         isARepeat: false, keyCode: 0)!
    }

    func press(_ c: String) -> ThreadKeys.Routed { ThreadKeys.route(key(c), model: model) }
}

private let keys = ["a", "d", "e", "l", "y", "t"]

// MARK: each key in each layout

@MainActor @Test(arguments: keys)
func aListShortcutRuns(key: String) async {
    #expect(await Window(.list).press(key) == .list(key))
}

@MainActor @Test(arguments: keys)
func aBoardShortcutRuns(key: String) async {
    #expect(await Window(.board).press(key) == .list(key))
}

@MainActor @Test(arguments: keys)
func aFeedShortcutRuns(key: String) async {
    #expect(await Window(.feed).press(key) == .feed(key))
}

@MainActor @Test(arguments: [LayoutMode.list, .board])
func lOpensLaterForTheSelection(layout: LayoutMode) async {
    let w = await Window(layout)
    _ = w.press("l")
    #expect(w.model.later?.ids == ["t1"])
}

@MainActor @Test func lOpensLaterOnTheFocusedFeedCard() async {
    let w = await Window(.feed)
    _ = w.press("l")
    #expect(w.model.later?.cardId == "c1")
}

@MainActor @Test(arguments: [LayoutMode.list, .board])
func tOpensTheChat(layout: LayoutMode) async {
    let w = await Window(layout)
    _ = w.press("t")
    #expect(w.model.chatOpen)
}

// MARK: wherever the focus is

@MainActor @Test(arguments: [LayoutMode.list, .board, .feed])
func aShortcutRunsAfterThePaletteCloses(layout: LayoutMode) async {
    let w = await Window(layout)
    w.model.paletteMode = .search
    await w.settle()
    w.model.paletteMode = nil
    await w.settle()
    #expect(w.press("e") != .skipped("typing"))
}

@MainActor @Test(arguments: [LayoutMode.list, .board])
func aShortcutRunsWithTheChatInFocus(layout: LayoutMode) async {
    let w = await Window(layout)
    // A click in the reading pane's chat gives it the keyboard.
    let chat = NSView()
    w.host.addSubview(chat)
    w.window.makeFirstResponder(chat)
    #expect(w.press("e") == .list("e"))
}

@MainActor @Test func aShortcutRunsWithAReadOnlyTextInFocus() async {
    let w = await Window(.board)
    let text = NSTextView()
    text.isEditable = false
    w.host.addSubview(text)
    w.window.makeFirstResponder(text)
    #expect(w.press("e") == .list("e"))
}

@MainActor @Test func aShortcutIsTypedInAFieldOnScreen() async {
    let w = await Window(.board)
    let field = NSTextView()
    w.host.addSubview(field)
    w.window.makeFirstResponder(field)
    #expect(w.press("e") == .skipped("typing"))
}

@MainActor @Test func aFieldThatClosedNoLongerTakesTheShortcuts() async {
    let w = await Window(.board)
    let field = NSTextView()
    w.host.addSubview(field)
    w.window.makeFirstResponder(field)
    field.isHidden = true
    #expect(w.press("e") == .list("e"))
}

@MainActor @Test func aFeedCardsLaterPickerLeftBehindDoesNotBlockTheBoard() async {
    let w = await Window(.board)
    w.model.later = LaterRequest(ids: ["t1"], cardId: "c1", typing: false)
    #expect(w.press("e") == .list("e"))
}

@MainActor @Test func theLaterPanelTakesTheKeysWhileOpen() async {
    let w = await Window(.board)
    w.model.later = LaterRequest(ids: ["t1"], cardId: nil, typing: false)
    #expect(w.press("e") == .skipped("later"))
}

@MainActor @Test func aDraftTakesTheKeys() async {
    let w = await Window(.board)
    w.model.startNew()
    #expect(w.press("e") == .skipped("compose"))
}
