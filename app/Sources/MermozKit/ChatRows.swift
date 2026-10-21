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
        return m.attachments.filter {
            $0.inline != true && $0.decorative != true && !shown.contains($0.id) && isPhoto($0)
        }
    }
}

/// The small line a bubble shows over its text.
public enum BubbleCaption {
    /// "Forwarded from Nora" for a forward (just "Forwarded" when the original sender is
    /// unknown); nil otherwise.
    public static func of(_ m: Message) -> String? {
        guard m.forwarded == true else { return nil }
        guard let from = m.forwardedFrom?.trimmingCharacters(in: .whitespaces), !from.isEmpty else { return "Forwarded" }
        return "Forwarded from \(from)"
    }
}

/// A sent message that didn't reach someone: said under its bubble, in red.
public struct DeliveryNote: Equatable, Sendable {
    public var recipient: String
    public var reason: String

    public init(recipient: String, reason: String) { self.recipient = recipient; self.reason = reason }

    /// "Not delivered to nora@x.com: address doesn't exist".
    public var line: String { reason.isEmpty ? "Not delivered to \(recipient)" : "Not delivered to \(recipient): \(reason)" }

    /// A bounce whose failed message isn't in the conversation, in one line.
    public static func systemLine(_ bounce: Message) -> String {
        let text = BubbleParts.compact(bounce).text.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
        let short = text.count > 70 ? String(text.prefix(70)) + "…" : text
        return short.isEmpty ? "A message wasn't delivered" : "A message wasn't delivered · \(short)"
    }
}

/// An email delivered several times (a list and a direct copy, a resend): the chat shows
/// it once with a small note.
public enum Received {
    /// "Received twice", "Received 3 times"; nil for an email received once.
    public static func note(copies: Int?) -> String? {
        guard let n = copies, n >= 2 else { return nil }
        return n == 2 ? "Received twice" : "Received \(n) times"
    }
}

/// A row the app draws in SwiftUI.
public enum HostedRow: Sendable {
    case summary
    case expanded(Message)
    /// An unsent reply, shown where it falls in the conversation.
    case draft(Draft)
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
        let all = items.compactMap { if case let .message(m) = $0 { m } else { nil } }
        let names = SenderIdentity.names(all, people: people)
        // A reaction to a bubble on screen is a badge on that bubble, not a message of
        // its own: it neither shows nor breaks the run it falls in. One to a message out
        // of view (or opened as the whole email) stays, as a small line.
        let bubbles = Set(all.filter { $0.reaction == nil && !expanded.contains($0.id) }.map(\.id))
        var badges: [String: [ReactionBadge]] = [:]
        var badged = Set<String>()
        for m in all {
            guard let r = m.reaction, let target = r.targetMessageId, bubbles.contains(target) else { continue }
            badges[target, default: []].append(ReactionBadge(emoji: r.emoji, by: Self.reactor(m, names: names)))
            badged.insert(m.id)
        }
        // A bounce whose failed message is in the conversation is said on that message
        // ("Not delivered to …"), not as a bubble of its own.
        let reported = Set(all.compactMap { $0.deliveryFailure?.bounceMessageId })
        let items = items.filter {
            if case let .message(m) = $0 { !badged.contains(m.id) && !reported.contains(m.id) } else { true }
        }
        let runs = ChatRuns.startsRun(items)
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
            case let .draft(d):
                // Keyed by its save time, so an edited draft redraws.
                let key = "draft:\(d.id):\(d.updated)"
                rows.append(.hosted(key: key))
                hosted[key] = .draft(d)
                previous = nil
            case let .marker(threadId, subject, date, joined):
                rows.append(.marker(MarkerSpec(threadId: threadId, line: "\(time(date)) · \(subject.isEmpty ? "(no subject)" : subject)",
                                               joined: joined)))
                previous = nil
            case let .message(m) where m.isBounce == true:
                // A bounce for a message not in view: one quiet line, not a bubble.
                rows.append(.marker(MarkerSpec(threadId: "", line: DeliveryNote.systemLine(m), joined: false)))
                previous = nil
            case let .message(m) where m.reaction != nil:
                // "❤️ Nora reacted": the message reacted to is not a bubble here.
                rows.append(.marker(MarkerSpec(threadId: "", line: "\(m.reaction!.emoji) \(Self.reactor(m, names: names)) reacted",
                                               joined: false)))
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
                    // The same email delivered more than once shows once, saying so.
                    if let received = Received.note(copies: m.copies) {
                        note = note.map { "\(received) · \($0)" } ?? received
                    }
                    let key = SenderIdentity.key(m.from)
                    let photos = ChatPhotos.attached(m, pasted: parts.thumbnails)
                    rows.append(.bubble(BubbleSpec(
                        id: m.id, content: content, mine: m.isFromMe, firstOfRun: first, sender: names[key] ?? m.from.display,
                        senderKey: key,
                        // Signature logos and icons sent as files are no files to the reader.
                        time: time(m.date), focused: focused == m.id,
                        files: max(0, MessageBody.chips(m).filter { $0.decorative != true }.count - photos.count), note: note,
                        thumbnails: parts.thumbnails.map(\.id), photos: photos, caption: BubbleCaption.of(m),
                        quoted: m.quotedOnly == true, reactions: badges[m.id] ?? [],
                        failure: m.deliveryFailure.map { DeliveryNote(recipient: $0.recipient, reason: $0.reason) })))
                }
                previous = (m, parts.text)
            }
        }
        return (rows, hosted)
    }

    /// Who reacted, as the chat names them: "You" for the user.
    static func reactor(_ m: Message, names: [String: String]) -> String {
        m.isFromMe ? "You" : names[SenderIdentity.key(m.from)] ?? m.from.display
    }
}
