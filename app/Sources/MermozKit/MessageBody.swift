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
        // The whole email keeps its signature; quoted history stays behind its fold.
        let text = showQuoted ? m.bodyText : withoutQuotes(m.bodyText)
        return .text(text.isEmpty ? m.bodyText : text)
    }

    /// The text above the quoted history: the first "> " line or "On … wrote:" opener.
    public static func withoutQuotes(_ text: String) -> String {
        let lines = text.components(separatedBy: "\n")
        let cut = lines.firstIndex { l in
            let t = l.trimmingCharacters(in: .whitespaces).lowercased()
            return t.hasPrefix(">") || ((t.hasPrefix("on ") || t.hasPrefix("le ")) && (t.hasSuffix("wrote:") || t.hasSuffix("écrit :") || t.hasSuffix("écrit:")))
        }
        guard let cut else { return text }
        return lines[..<cut].joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
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

/// What a message shows at each of the two levels of the reading pane. Compact: a light
/// chat bubble with only what the message adds. Expanded: the whole email.
public struct BubbleParts: Equatable, Sendable {
    /// "to Nora, cc Sam" (expanded only).
    public var recipients: String?
    public var text: String
    /// The Original / Optimized choice and the "···" menu (expanded only).
    public var controls: Bool
    /// Files sent along, as chips (expanded only).
    public var chips: [Attachment]
    /// Pasted images, as small thumbnails (compact only; expanded renders them in place).
    public var thumbnails: [Attachment]
    /// Files sent along, counted on a compact bubble.
    public var fileCount: Int

    /// The message's new text only: quoted history and signature already folded locally.
    public static func compact(_ m: Message) -> BubbleParts {
        let new = m.displayBody.trimmingCharacters(in: .whitespacesAndNewlines)
        let text = new.isEmpty ? m.bodyText.trimmingCharacters(in: .whitespacesAndNewlines) : new
        return BubbleParts(recipients: nil, text: text, controls: false, chips: [],
                           thumbnails: MessageBody.inlineImages(m), fileCount: MessageBody.chips(m).count)
    }

    /// The whole email: recipients, controls, its full text (signature included) and chips.
    public static func expanded(_ m: Message) -> BubbleParts {
        BubbleParts(recipients: Recipients.line(to: m.to, cc: m.cc, bcc: m.bcc ?? []), text: m.bodyText,
                    controls: true, chips: MessageBody.chips(m), thumbnails: [], fileCount: 0)
    }

    /// Compact text longer than about eight lines starts clamped, behind "Show more".
    public static func clampsCompact(_ text: String) -> Bool {
        text.count > 700 || text.split(separator: "\n", omittingEmptySubsequences: false).count > 8
    }
}

/// Consecutive messages from the same person read as one run, like a messaging app:
/// only the first shows the name, avatar and time.
public enum ChatRuns {
    /// A new run starts after a thread marker, with another sender, or after 3 hours.
    public static func startsRun(_ items: [TimelineItem]) -> Set<String> {
        var out = Set<String>()
        var previous: Message?
        for item in items {
            switch item {
            case .marker:
                previous = nil
            case let .message(m):
                let same = previous.map {
                    $0.isFromMe == m.isFromMe && $0.from.email.lowercased() == m.from.email.lowercased()
                        && m.date - $0.date < 3 * 3_600_000
                } ?? false
                if !same { out.insert(m.id) }
                previous = m
            }
        }
        return out
    }
}
