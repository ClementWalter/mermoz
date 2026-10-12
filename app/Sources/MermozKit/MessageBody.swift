/// What a message bubble shows, decided without the network or the AI: the email's own
/// text at once, its formatted version once loaded, the AI's clean version only when
/// the reader chose Optimized for that message.
import Foundation

public enum BodyShown: Equatable, Sendable {
    /// The email's text (quoted history and signature folded unless `showQuoted`).
    case text(String)
    /// The formatted email, already loaded.
    case html
    /// The AI's clean version.
    case clean(String)
}

public enum MessageBody {
    /// Text longer than this is clamped behind a visible "Show more".
    public static let clampLimit = 6000

    /// Optimized shows the AI's version only once it exists; until then, and in
    /// Original, the email itself shows: formatted when loaded, else its text.
    public static func shown(_ m: Message, mode: ReadingMode, htmlReady: Bool, showQuoted: Bool) -> BodyShown {
        if mode == .optimized, let clean = m.bodyClean { return .clean(clean) }
        if mode == .original, m.hasHtml, htmlReady { return .html }
        let text = showQuoted ? m.bodyText : m.displayBody
        return .text(text.isEmpty ? m.bodyText : text)
    }

    /// The text cut at a line or word boundary before `limit`, and whether it was cut.
    public static func clamp(_ text: String, expanded: Bool, limit: Int = clampLimit) -> (text: String, clamped: Bool) {
        guard !expanded, text.count > limit else { return (text, false) }
        let head = text.prefix(limit)
        let cut = head.lastIndex(of: "\n") ?? head.lastIndex(of: " ") ?? head.endIndex
        return (String(head[..<cut]), true)
    }

    /// Files sent along: images placed in the body are shown there, not as chips.
    public static func chips(_ m: Message) -> [Attachment] {
        m.attachments.filter { $0.inline != true }
    }

    /// Images placed in the body, shown under the text while the formatted version loads.
    public static func inlineImages(_ m: Message) -> [Attachment] {
        m.attachments.filter { $0.inline == true && $0.mime.lowercased().hasPrefix("image/") }
    }
}

/// How a message is laid out in the reading pane. Every message is a bubble, whatever
/// the conversation's kind or who is on it; feeds and lists only use a compact header.
public enum MessageLayout: Equatable, Sendable {
    case bubble(fromMe: Bool, compact: Bool)

    public static func of(_ m: Message, kind: String?) -> MessageLayout {
        .bubble(fromMe: m.isFromMe, compact: kind == "feed" || kind == "list")
    }
}

public enum Recipients {
    /// "to Nora, cc Sam": empty parts and undisclosed recipients are left out, so an
    /// email sent to Cc only reads "cc Sam", never "to , cc Sam".
    public static func line(to: [Address], cc: [Address], bcc: [Address] = []) -> String {
        func names(_ a: [Address]) -> String {
            a.filter { !$0.email.isEmpty && !$0.email.lowercased().hasPrefix("undisclosed-recipients") }
                .map(\.display).joined(separator: ", ")
        }
        let parts = [("to", names(to)), ("cc", names(cc)), ("bcc", names(bcc))]
            .filter { !$0.1.isEmpty }
            .map { "\($0.0) \($0.1)" }
        return parts.isEmpty ? "to undisclosed recipients" : parts.joined(separator: ", ")
    }
}
