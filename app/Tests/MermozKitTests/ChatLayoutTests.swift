/// Every row of the chat has its own place: thread markers, my bubbles, theirs and a
/// near-duplicate never overlap, at any pane width.
import AppKit
import Testing
@testable import MermozKit

private let letter = """
Madame, Monsieur,

Sauf erreur ou omission de notre part, votre compte présente un solde débiteur.

Veuillez trouver ci-joint un avis de relance N° %@.

Cordialement, le service des ventes.
"""

private func message(_ id: String, _ body: String, from: String, mine: Bool = false, date: Int64) -> Message {
    Message(id: id, threadId: "t", from: Address(name: "", email: from), to: [], cc: [], bcc: nil, date: date, subject: "S",
            bodyText: body, bodyNew: nil, hasQuoted: nil, bodyClean: nil, hasHtml: false, isFromMe: mine, attachments: [],
            auth: nil)
}

/// Two Gmail threads: a separator, theirs, mine, then a joined thread with two
/// near-identical letters.
private let items: [TimelineItem] = [
    .marker(threadId: "t1", subject: "Action required – your account is past due", date: 1_000, joined: false),
    .message(message("a", "Your invoice is ready.", from: "billing@cloud.example", date: 1_000)),
    .message(message("b", "Paid it this morning.", from: "me@example.com", mine: true, date: 2_000)),
    .marker(threadId: "t2", subject: "Avis de relance", date: 3_000, joined: true),
    .message(message("c", String(format: letter, "104325"), from: "recouvrement@lpcr.example", date: 3_000)),
    .message(message("d", String(format: letter, "104326"), from: "recouvrement@lpcr.example", date: 3_001)),
]

@MainActor
private func frames(width: CGFloat) -> [CGRect] {
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: width, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView = stream
    let rows = ChatRows.build(items: items, hidden: 2, summary: nil, expanded: [], shownInFull: [], focused: nil,
                              time: { _ in "3 oct." }).rows
    stream.show(rows, conversation: "c", messages: 4)
    stream.layoutSubtreeIfNeeded()
    stream.documentView?.layoutSubtreeIfNeeded()
    return stream.rowViews.map(\.frame)
}

@MainActor @Test(arguments: [360.0, 640.0, 1_100.0])
func noTwoRowsOverlap(width: Double) {
    let f = frames(width: width)
    let overlapping = f.indices.flatMap { i in f.indices.filter { $0 > i && f[i].intersects(f[$0]) }.map { (i, $0) } }
    #expect(overlapping.isEmpty)
}

@MainActor @Test(arguments: [360.0, 640.0, 1_100.0])
func everyRowHasHeight(width: Double) {
    #expect(frames(width: width).allSatisfy { $0.height > 0 })
}

@MainActor @Test(arguments: [360.0, 640.0, 1_100.0])
func rowsRunTopToBottomInOrder(width: Double) {
    let f = frames(width: width)
    #expect(zip(f, f.dropFirst()).allSatisfy { $0.maxY <= $1.minY })
}
