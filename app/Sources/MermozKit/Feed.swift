/// The Feed (third layout): cards as mermozd sends them, and the pure rules the view
/// follows — which action a key runs, where focus goes, what the header says, when
/// "Later" ends and how an event becomes a calendar file.
import Foundation

public struct Feed: Codable, Sendable, Equatable {
    public var sections: [FeedSection]
    public var open: Int
    public var judged: Int
    /// Jev still has open threads to judge.
    public var sorting: Bool
    /// Why the AI is on hold ("credits", "unavailable"); cards stay code-built meanwhile.
    public var paused: String?

    public init(sections: [FeedSection], open: Int = 0, judged: Int = 0, sorting: Bool = false) {
        self.sections = sections; self.open = open; self.judged = judged; self.sorting = sorting
    }

    /// Every card, top to bottom.
    public var cards: [FeedCard] { sections.flatMap(\.cards) }

    public func card(_ id: String?) -> FeedCard? { cards.first { $0.id == id } }
}

public struct FeedSection: Codable, Sendable, Equatable, Identifiable {
    public var key: String
    public var title: String
    public var cards: [FeedCard]
    /// Cards past the section's limit; the List shows them all.
    public var more: Int
    public var id: String { key }

    public init(key: String, title: String, cards: [FeedCard], more: Int = 0) {
        self.key = key; self.title = title; self.cards = cards; self.more = more
    }
}

/// One way to answer a decision, with the reply that picks it.
public struct FeedChoice: Codable, Sendable, Equatable, Hashable {
    public var label: String
    public var detail: String
    public var recommended: Bool
    public var reason: String
    public var reply: String

    public init(label: String, detail: String = "", recommended: Bool = false, reason: String = "", reply: String = "") {
        self.label = label; self.detail = detail; self.recommended = recommended; self.reason = reason; self.reply = reply
    }
}

/// The AI's words for a card.
public struct FeedTexts: Codable, Sendable, Equatable {
    public var title: String
    public var why: String
    public var draft: String
    public var options: [FeedChoice]
    public var points: [String]
    public var summary: String
    /// A batch's one-line summaries, one per member.
    public var lines: [String]?

    public init(title: String = "", why: String = "", draft: String = "", options: [FeedChoice] = [],
                points: [String] = [], summary: String = "", lines: [String]? = nil) {
        self.title = title; self.why = why; self.draft = draft; self.options = options
        self.points = points; self.summary = summary; self.lines = lines
    }
}

public struct FeedCard: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var kind: FeedKind
    public var section: String
    public var thread: ThreadSummary
    public var members: [ThreadSummary]
    public var title: String
    public var why: String?
    public var reasons: [String]
    public var due: String?
    public var dueTime: String?
    public var amount: String?
    public var fromBacklog: Bool
    public var waitingDays: Int?
    public var texts: FeedTexts?
    public var textsWanted: Bool

    public init(id: String, kind: FeedKind, section: String, thread: ThreadSummary, members: [ThreadSummary] = [],
                title: String, why: String? = nil, reasons: [String] = [], due: String? = nil, dueTime: String? = nil,
                amount: String? = nil, fromBacklog: Bool = false, waitingDays: Int? = nil, texts: FeedTexts? = nil,
                textsWanted: Bool = false) {
        self.id = id; self.kind = kind; self.section = section; self.thread = thread; self.members = members
        self.title = title; self.why = why; self.reasons = reasons; self.due = due; self.dueTime = dueTime
        self.amount = amount; self.fromBacklog = fromBacklog; self.waitingDays = waitingDays
        self.texts = texts; self.textsWanted = textsWanted
    }

    public var isBatch: Bool { !members.isEmpty }

    /// Who the card is about: the last sender, or the other person when the user wrote last.
    public var person: String {
        guard thread.lastFrom == "You" else { return thread.lastFrom }
        return thread.participants.first { $0 != "You" } ?? thread.lastFrom
    }

    /// Every thread the card stands for.
    public var threadIds: [String] { isBatch ? members.map(\.id) : [thread.id] }
}

/// What a card's keys and buttons do.
public enum FeedAction: Equatable, Sendable {
    /// Opens the composer on the card's thread, prefilled (never sent).
    case draft(String)
    /// Opens the mail in the reading pane.
    case open
    case addToCalendar
    /// Archives the listed threads, with undo.
    case archive([String])
    case done
    case later
}

public enum FeedActions {
    /// What ↩ runs on a card.
    public static func primary(_ card: FeedCard) -> FeedAction {
        switch card.kind {
        case .reply, .promise, .intro, .waiting:
            .draft(card.texts?.draft ?? "")
        case .decision:
            .draft(card.texts?.options.first { $0.recommended }?.reply ?? card.texts?.draft ?? "")
        case .event:
            card.due == nil ? .open : .addToCalendar
        case .cleanup, .reading:
            .archive(card.threadIds)
        case .sign, .payment, .other:
            .open
        }
    }

    /// The label of the primary button.
    public static func primaryLabel(_ card: FeedCard) -> String {
        switch card.kind {
        case .reply, .promise, .intro, .decision: "Reply"
        case .waiting: "Follow up"
        case .event: card.due == nil ? "Open" : "Add to Calendar"
        case .cleanup, .reading: card.members.count > 1 ? "Archive \(card.members.count)" : "Archive"
        case .sign: "Open to sign"
        case .payment, .other: "Open"
        }
    }

    /// What 1, 2 or 3 runs on a decision card: the reply choosing that option.
    public static func option(_ n: Int, of card: FeedCard) -> FeedAction? {
        guard card.kind == .decision, let options = card.texts?.options, n >= 1, n <= options.count else { return nil }
        return .draft(options[n - 1].reply)
    }
}

/// What a key does on the focused card while the Feed is on screen.
public enum FeedKeyAction: Equatable, Sendable {
    case next, previous, primary, option(Int), later, laterPickDay, open, done, archive, delete, ask, reply, accept, dismiss
}

public enum FeedKeyMap {
    /// The characters AppKit types for the ↓ and ↑ keys.
    public static let downArrow = "\u{f701}"
    public static let upArrow = "\u{f700}"

    /// Characters the Feed takes besides the app-wide shortcuts: ↩, ⌫, ⌦ and the arrows.
    public static let characters = "lo123\rdn\u{7f}\u{f728}" + downArrow + upArrow

    /// The action for a typed character (already lowercased; `shift` tells l from ⇧L).
    public static func action(_ c: String, shift: Bool = false) -> FeedKeyAction? {
        switch c {
        // The arrows read on the window, not on a focused view, so they work wherever the
        // focus is; with ⇧ they are left alone (the Feed has no multi-selection).
        case "j", downArrow where !shift: .next
        case "k", upArrow where !shift: .previous
        case "\r": .primary
        case "1", "2", "3": .option(Int(c) ?? 0)
        case "l": shift ? .laterPickDay : .later
        case "o": .open
        case "e": .done
        case "a": .archive
        // d, ⌫, ⌦ and # all send the mail to the Trash, as everywhere in the app.
        case "d", "\u{7f}", "\u{f728}", "#": .delete
        case "t": .ask
        case "r": .reply
        // The card's ✦ suggestion: y accepts it, n dismisses it.
        case "y": .accept
        case "n": .dismiss
        default: nil
        }
    }
}

public enum FeedSelection {
    /// The mail a card shows in the reading pane: its thread, or a batch's latest mail.
    public static func thread(of card: FeedCard) -> String {
        card.members.first?.id ?? card.thread.id
    }
}

public enum FeedFocus {
    /// The card after (or before) `current`; the first card when nothing is focused.
    public static func step(_ ids: [String], from current: String?, by delta: Int) -> String? {
        guard !ids.isEmpty else { return nil }
        guard let current, let i = ids.firstIndex(of: current) else { return delta >= 0 ? ids.first : ids.last }
        return ids[max(0, min(ids.count - 1, i + delta))]
    }

    /// Where focus lands once `gone` leaves: the next card, else the previous one.
    public static func after(removing gone: String, from ids: [String]) -> String? {
        guard let i = ids.firstIndex(of: gone) else { return ids.first }
        let rest = ids.filter { $0 != gone }
        return rest.isEmpty ? nil : rest[min(i, rest.count - 1)]
    }
}

public enum FeedHeader {
    /// "3 things need you now and 2 more today." — the line under the date.
    public static func line(_ feed: Feed) -> String {
        let count = { (key: String) in feed.sections.first { $0.key == key }?.cards.count ?? 0 }
        let now = count("now"), today = count("today")
        let things = { (n: Int) in n == 1 ? "1 thing needs" : "\(n) things need" }
        switch (now, today) {
        case (0, 0): return feed.cards.isEmpty ? "Nothing needs you. New mail lands here as it arrives." : "Nothing urgent today."
        case (0, _): return "\(things(today)) you today."
        case (_, 0): return "\(things(now)) you now."
        default: return "\(things(now)) you now and \(today) more today."
        }
    }
}

public enum FeedLater {
    /// When "Later" ends: 8:00 the next morning.
    public static func tomorrowMorning(after now: Date, calendar: Calendar = .current) -> Date {
        morning(of: calendar.date(byAdding: .day, value: 1, to: now) ?? now, calendar: calendar)
    }

    /// 8:00 on a chosen day (⇧L).
    public static func morning(of day: Date, calendar: Calendar = .current) -> Date {
        calendar.date(bySettingHour: 8, minute: 0, second: 0, of: day) ?? day
    }

    public static func millis(_ d: Date) -> Int64 { Int64((d.timeIntervalSince1970 * 1000).rounded()) }
}

public enum FeedWords {
    /// "Fri 16 Oct" for a "YYYY-MM-DD" date; "today" and "tomorrow" when close.
    public static func due(_ date: String, today: Date = Date(), calendar: Calendar = .current) -> String {
        guard let d = day(date, calendar: calendar) else { return date }
        let start = calendar.startOfDay(for: today)
        let days = calendar.dateComponents([.day], from: start, to: d).day ?? 0
        switch days {
        case 0: return "today"
        case 1: return "tomorrow"
        case -1: return "yesterday"
        default:
            // A formatter costs far more to make than to use: one per calendar, reused.
            let f = formatter(calendar)
            return f.string(from: d)
        }
    }

    nonisolated(unsafe) private static var formatters: [String: DateFormatter] = [:]
    private static let lock = NSLock()

    private static func formatter(_ calendar: Calendar) -> DateFormatter {
        lock.lock()
        defer { lock.unlock() }
        let key = calendar.timeZone.identifier
        if let f = formatters[key] { return f }
        let f = DateFormatter()
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.locale = Locale(identifier: "en_GB")
        f.dateFormat = "EEE d MMM"
        formatters[key] = f
        return f
    }

    /// Two lines that say almost the same thing (the AI's "why" and the mail's gist):
    /// one contains the other, or they share most of their words.
    public static func nearDuplicate(_ a: String?, _ b: String?) -> Bool {
        let words = { (s: String?) -> [String] in
            (s ?? "").lowercased().split { !$0.isLetter && !$0.isNumber }.map(String.init)
        }
        let x = words(a), y = words(b)
        guard !x.isEmpty, !y.isEmpty else { return false }
        let jx = x.joined(separator: " "), jy = y.joined(separator: " ")
        if jx.contains(jy) || jy.contains(jx) { return true }
        let sx = Set(x), sy = Set(y)
        return Double(sx.intersection(sy).count) / Double(sx.union(sy).count) >= 0.6
    }

    /// The start of a "YYYY-MM-DD" day.
    public static func day(_ s: String, calendar: Calendar = .current) -> Date? {
        let p = s.split(separator: "-").compactMap { Int($0) }
        guard p.count == 3 else { return nil }
        return calendar.date(from: DateComponents(year: p[0], month: p[1], day: p[2]))
    }
}

/// A calendar file for a Date card, opened with Calendar to add the event.
public enum CalendarFile {
    /// An all-day event when there is no time, else one hour from that time.
    public static func ics(title: String, date: String, time: String?, uid: String, now: Date = Date()) -> String? {
        let d = date.replacingOccurrences(of: "-", with: "")
        guard d.count == 8, d.allSatisfy(\.isNumber) else { return nil }
        let stamp = utc(now)
        var lines = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Mermoz//Feed//EN", "BEGIN:VEVENT",
                     "UID:\(uid)@mermoz", "DTSTAMP:\(stamp)", "SUMMARY:\(escape(title))"]
        if let time, let (h, m) = hourMinute(time) {
            // One hour, kept within the day.
            let (eh, em) = h >= 23 ? (23, 59) : (h + 1, m)
            let start = String(format: "%@T%02d%02d00", d, h, m)
            let end = String(format: "%@T%02d%02d00", d, eh, em)
            lines += ["DTSTART:\(start)", "DTEND:\(end)"]
        } else {
            lines += ["DTSTART;VALUE=DATE:\(d)"]
        }
        lines += ["END:VEVENT", "END:VCALENDAR"]
        return lines.joined(separator: "\r\n") + "\r\n"
    }

    static func hourMinute(_ s: String) -> (Int, Int)? {
        let p = s.split(separator: ":").compactMap { Int($0) }
        guard p.count == 2, (0..<24).contains(p[0]), (0..<60).contains(p[1]) else { return nil }
        return (p[0], p[1])
    }

    /// Commas, semicolons and backslashes are special in calendar text.
    static func escape(_ s: String) -> String {
        s.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: ";", with: "\\;")
            .replacingOccurrences(of: ",", with: "\\,")
            .replacingOccurrences(of: "\n", with: "\\n")
    }

    static func utc(_ d: Date) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyyMMdd'T'HHmmss'Z'"
        return f.string(from: d)
    }
}

public enum FeedTextsRequest {
    /// The cards on screen whose words the AI should write now: wanted, not written, not
    /// asked already. A batch asks for its members.
    public static func ids(visible: [FeedCard], asked: Set<String>) -> [String] {
        visible.filter(\.textsWanted).flatMap(\.threadIds).filter { !asked.contains($0) }
    }
}
