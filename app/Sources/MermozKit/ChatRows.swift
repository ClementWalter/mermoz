/// What the reading pane's chat shows, row by row: compact bubbles by default for every
/// sender (people, shops, automated letters alike), the whole email only for messages
/// the reader opened, and a note on a message that repeats the previous one but for a
/// few words.
import Foundation

/// Two near-identical messages in a row from one sender (two dunning letters sent the
/// same minute that differ only by a number): the words the second one changes.
public enum NearDuplicate {
    /// Words of `current` not in `previous`, when the two are otherwise the same text;
    /// nil when they differ more than a few words. Local and cheap: a word comparison.
    public static func differences(previous: String, current: String) -> [String]? {
        let a = words(previous), b = words(current)
        guard a.count >= 8, b.count >= 8, abs(a.count - b.count) <= 2 else { return nil }
        var changed: [String] = []
        if a.count == b.count {
            for i in b.indices where a[i] != b[i] {
                // A short label before a changed number ("N°", "No") reads with it.
                if i > 0, b[i - 1].count <= 3, b[i].contains(where: \.isNumber), !changed.contains(b[i - 1]) {
                    changed.append(b[i - 1] + " " + b[i])
                } else {
                    changed.append(b[i])
                }
            }
        } else {
            let seen = Set(a)
            changed = b.filter { !seen.contains($0) }
        }
        guard !changed.isEmpty, changed.count <= 3, changed.count * 10 <= b.count else { return nil }
        return changed
    }

    /// The note shown on the second message.
    public static func note(_ changed: [String]) -> String {
        let shown = changed.map { $0.trimmingCharacters(in: CharacterSet(charactersIn: ".,;:!?")) }
        return "Almost the same as the previous message · differs in: " + shown.joined(separator: ", ")
    }

    static func words(_ s: String) -> [String] {
        s.split(whereSeparator: { $0.isWhitespace }).map(String.init)
    }
}

/// One name and one avatar per person, whatever case or variant each message's From
/// line carries ("aude walter", "Aude Walter").
public enum SenderIdentity {
    /// Who a message is from, for avatar colors and runs: the lowercased address.
    public static func key(_ a: Address) -> String { a.email.lowercased() }

    /// The name to show for each sender key: the conversation's directory name, else the
    /// best-cased variant the messages carry (mixed case over all lower or all upper,
    /// then the most frequent, then the first seen).
    public static func names(_ messages: [Message], people: [ConversationPerson]) -> [String: String] {
        var seen: [String: [(name: String, count: Int)]] = [:]
        for m in messages where !m.from.name.isEmpty {
            let k = key(m.from)
            var variants = seen[k] ?? []
            if let i = variants.firstIndex(where: { $0.name == m.from.name }) {
                variants[i].count += 1
            } else {
                variants.append((m.from.name, 1))
            }
            seen[k] = variants
        }
        var out: [String: String] = [:]
        for (k, variants) in seen {
            // Only a strictly better variant wins: on a tie the first seen stays.
            var best = variants[0]
            for v in variants.dropFirst() where (casing(v.name), v.count) > (casing(best.name), best.count) { best = v }
            out[k] = best.name
        }
        for p in people where !p.name.isEmpty { out[p.email.lowercased()] = p.name }
        return out
    }

    /// 2: mixed case, 1: all upper, 0: all lower (or no letters).
    static func casing(_ name: String) -> Int {
        let upper = name.contains(where: \.isUppercase), lower = name.contains(where: \.isLowercase)
        return upper && lower ? 2 : upper ? 1 : 0
    }

    /// Up to two initials of a name, "?" when it has none.
    public static func initials(_ name: String) -> String {
        let s = name.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined().uppercased()
        return s.isEmpty ? "?" : s
    }

    /// An avatar color index for a sender key, the same across launches (a sum of code
    /// points: String.hashValue is seeded per launch).
    public static func colorIndex(_ key: String, count: Int) -> Int {
        key.unicodeScalars.reduce(0) { $0 &+ Int($1.value) } % max(count, 1)
    }
}

/// Photos sent along with a message (image files, not images pasted in the body): the
/// chat shows them as a grid; other files stay a count.
public enum ChatPhotos {
    static let types: Set<String> = ["image/jpeg", "image/jpg", "image/pjpeg", "image/png", "image/heic", "image/heif",
                                     "image/gif", "image/webp"]
    static let extensions: Set<String> = ["jpg", "jpeg", "png", "heic", "heif", "gif", "webp"]

    /// An image file a photo viewer opens; a generic type falls back on the file's extension.
    public static func isPhoto(_ a: Attachment) -> Bool {
        let mime = a.mime.lowercased()
        if types.contains(mime) { return true }
        guard mime.isEmpty || mime == "application/octet-stream" else { return false }
        return extensions.contains((a.filename as NSString).pathExtension.lowercased())
    }

    /// The message's attached photos, without the images pasted in its body.
    public static func attached(_ m: Message, pasted: [Attachment]) -> [Attachment] {
        let shown = Set(pasted.map(\.id))
        return m.attachments.filter { $0.inline != true && !shown.contains($0.id) && isPhoto($0) }
    }
}

/// A row the app draws in SwiftUI.
public enum HostedRow: Sendable {
    case summary
    case expanded(Message)
}

public enum ChatRows {
    /// The chat's rows and the SwiftUI views it needs, by key. `time` formats a date.
    /// `summary`: the AI's summary on top, when there is one.
    /// `people`: the conversation's directory, for the names senders show under.
    public static func build(items: [TimelineItem], hidden: Int, summary: String?, expanded: Set<String>,
                             shownInFull: Set<String>, focused: String?, people: [ConversationPerson] = [],
                             time: (Int64) -> String)
        -> (rows: [ChatRowSpec], hosted: [String: HostedRow])
    {
        let runs = ChatRuns.startsRun(items)
        let names = SenderIdentity.names(items.compactMap { if case let .message(m) = $0 { m } else { nil } }, people: people)
        var rows: [ChatRowSpec] = []
        var hosted: [String: HostedRow] = [:]
        if let summary, !summary.isEmpty {
            // Keyed by its text, so a new summary replaces the old one on screen.
            let key = "summary:\(summary.hashValue)"
            rows.append(.hosted(key: key))
            hosted[key] = .summary
        }
        if hidden > 0 { rows.append(.earlier(hidden)) }
        var previous: (message: Message, text: String)?
        for item in items {
            switch item {
            case let .marker(threadId, subject, date, joined):
                rows.append(.marker(MarkerSpec(threadId: threadId, line: "\(time(date)) · \(subject.isEmpty ? "(no subject)" : subject)",
                                               joined: joined)))
                previous = nil
            case let .message(m):
                let first = runs.contains(m.id)
                let parts = BubbleParts.compact(m)
                let content = BubbleContent.compact(m, showAll: shownInFull.contains(m.id))
                if expanded.contains(m.id) {
                    // Only a message the reader opened shows as the whole email.
                    let key = "expanded:\(m.id):\(m.bodyText.utf8.count):\(m.auth ?? ""):\(m.attachments.count)"
                    rows.append(.hosted(key: key))
                    hosted[key] = .expanded(m)
                } else {
                    var note: String?
                    if let p = previous, !first, p.message.from.email.lowercased() == m.from.email.lowercased(),
                       let changed = NearDuplicate.differences(previous: p.text, current: parts.text) {
                        note = NearDuplicate.note(changed)
                    }
                    let key = SenderIdentity.key(m.from)
                    let photos = ChatPhotos.attached(m, pasted: parts.thumbnails)
                    rows.append(.bubble(BubbleSpec(
                        id: m.id, content: content, mine: m.isFromMe, firstOfRun: first, sender: names[key] ?? m.from.display,
                        senderKey: key,
                        time: time(m.date), focused: focused == m.id, files: max(0, parts.fileCount - photos.count), note: note,
                        thumbnails: parts.thumbnails.map(\.id), photos: photos)))
                }
                previous = (m, parts.text)
            }
        }
        return (rows, hosted)
    }
}
