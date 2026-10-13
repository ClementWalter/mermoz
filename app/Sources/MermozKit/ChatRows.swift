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

/// A row the app draws in SwiftUI.
public enum HostedRow: Sendable {
    case summary
    case marker(threadId: String, subject: String, date: Int64, joined: Bool)
    case expanded(Message)
    /// A bubble with pasted images, which load as thumbnails.
    case images(Message, firstOfRun: Bool)
}

public enum ChatRows {
    /// The chat's rows and the SwiftUI views it needs, by key. `time` formats a date.
    /// `summary`: the AI's summary on top, when there is one.
    public static func build(items: [TimelineItem], hidden: Int, summary: String?, expanded: Set<String>,
                             shownInFull: Set<String>, focused: String?, time: (Int64) -> String)
        -> (rows: [ChatRowSpec], hosted: [String: HostedRow])
    {
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
            case let .marker(threadId, subject, date, joined):
                let key = "marker:\(threadId):\(joined)"
                rows.append(.hosted(key: key))
                hosted[key] = .marker(threadId: threadId, subject: subject, date: date, joined: joined)
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
                } else if !parts.thumbnails.isEmpty {
                    let key = "images:\(m.id):\(first)"
                    rows.append(.hosted(key: key))
                    hosted[key] = .images(m, firstOfRun: first)
                } else {
                    var note: String?
                    if let p = previous, !first, p.message.from.email.lowercased() == m.from.email.lowercased(),
                       let changed = NearDuplicate.differences(previous: p.text, current: parts.text) {
                        note = NearDuplicate.note(changed)
                    }
                    rows.append(.bubble(BubbleSpec(
                        id: m.id, content: content, mine: m.isFromMe, firstOfRun: first, sender: m.from.display,
                        time: time(m.date), focused: focused == m.id, files: parts.fileCount, note: note)))
                }
                previous = (m, parts.text)
            }
        }
        return (rows, hosted)
    }
}
