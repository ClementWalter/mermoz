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
        // Folded, the core's reading (quoted history already out); unfolded, every word.
        return .text(showQuoted ? m.bodyText : m.displayBody)
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

    /// Photos pasted in the body (signature logos and icons left out), shown under the
    /// text while the formatted version loads.
    public static func inlineImages(_ m: Message) -> [Attachment] {
        m.attachments.filter { $0.inline == true && $0.decorative != true && $0.mime.lowercased().hasPrefix("image/") }
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

    /// The message's new text, as the core read it (`bodyNew`: quoted history, signature
    /// and boilerplate already out, LF line ends). Kept per message: the chat asks again on
    /// every redraw.
    public static func compact(_ m: Message) -> BubbleParts {
        // Keyed by the texts themselves: an edited message never reuses its old parts.
        let key = "\(m.id)|\(m.bodyText.hashValue)|\(m.bodyNew?.hashValue ?? 0)|\(m.attachments.count)"
        return CompactParts.shared.parts(key) { build(m) }
    }

    static func build(_ m: Message) -> BubbleParts {
        // Display only, no cleaning: the core never sends a blank first or last line, but
        // an older daemon's raw text may end on a line break. An empty text then reads
        // "(empty message)".
        let text = m.displayBody.trimmingCharacters(in: .whitespacesAndNewlines)
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
        // Blank lines between paragraphs do not count: a short letter is not clamped.
        text.utf8.count > 700 && text.count > 700
            || text.split(separator: "\n").filter { !$0.allSatisfy(\.isWhitespace) }.count > 8
    }

    /// What a clamped bubble shows: enough of the start to fill its eight lines (at most
    /// ten lines and 900 characters), so the rest of a long email is never laid out.
    public static func opening(_ text: String) -> String {
        var out = Substring(text.prefix(900))
        if let tenth = out.indices.lazy.filter({ out[$0] == "\n" }).dropFirst(9).first {
            out = out[..<tenth]
        }
        return String(out)
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
            case .marker, .draft:
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

/// Compact bubble parts by message, safe from any thread; starts over past a few
/// thousand messages.
final class CompactParts: @unchecked Sendable {
    static let shared = CompactParts()
    private let lock = NSLock()
    private var parts: [String: BubbleParts] = [:]

    func parts(_ key: String, _ make: () -> BubbleParts) -> BubbleParts {
        lock.lock()
        if let p = parts[key] { lock.unlock(); return p }
        lock.unlock()
        let p = make()
        lock.lock()
        if parts.count >= 4000 { parts.removeAll(keepingCapacity: true) }
        parts[key] = p
        lock.unlock()
        return p
    }
}
