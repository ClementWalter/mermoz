/// A whole email opened in the chat grows once its HTML has loaded: the stream measures
/// it again, and the reader can scroll to its last pixel.
import AppKit
import Observation
import SwiftUI
import Testing
@testable import MermozKit

/// The height of an opened email's body, which grows after it is first shown.
@MainActor @Observable private final class EmailBody { var height: CGFloat = 200 }

private struct GrowingEmail: View {
    let email: EmailBody
    var body: some View { Color.gray.frame(height: email.height) }
}

private let short = [
    BubbleSpec(id: "a", content: BubbleContent(text: "Your invoice is ready.", clamped: false), mine: false, firstOfRun: true,
               sender: "AWS", time: "09:41", focused: false),
]

/// A stream in an offscreen window showing two bubbles' worth of chat and an email that
/// grows from 200 to 1,800 points once shown.
@MainActor
private func grown() async -> ChatStreamView {
    let email = EmailBody()
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 700, height: 600), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView = stream
    var a = ChatActions()
    a.hosted = { _ in AnyView(GrowingEmail(email: email)) }
    stream.actions = a
    stream.show(short.map(ChatRowSpec.bubble) + [.hosted(key: "expanded:x")], conversation: "c1", messages: 2)
    stream.layoutSubtreeIfNeeded()
    stream.documentView?.layoutSubtreeIfNeeded()
    email.height = 1_800
    // SwiftUI, then the stream, catch up over the next turns of the run loop.
    for _ in 0..<10 {
        try? await Task.sleep(for: .milliseconds(30))
        stream.documentView?.layoutSubtreeIfNeeded()
    }
    _ = window
    return stream
}

@MainActor @Test func anOpenedEmailThatGrowsIsMeasuredWhole() async {
    #expect(await grown().hostedFrame("expanded:x")!.height >= 1_800)
}

@MainActor @Test func theEndOfAGrownEmailCanBeScrolledIntoView() async {
    let stream = await grown()
    stream.scrollToEnd()
    #expect(stream.hostedFrame("expanded:x")!.maxY <= stream.visibleArea.maxY)
}
