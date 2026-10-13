/// Dates in the thread list: time today, "Yesterday", the weekday this week, else the
/// full date with its year, in the user's own order ("17 juil. 2026", "Jul 17, 2026").
import Foundation

public enum MailDate {
    public static func short(_ date: Date, now: Date = Date(), locale: Locale = .current, calendar: Calendar = .current) -> String {
        var cal = calendar
        cal.locale = locale
        let template: String
        if cal.isDate(date, inSameDayAs: now) {
            template = "jmm"
        } else if let y = cal.date(byAdding: .day, value: -1, to: now), cal.isDate(date, inSameDayAs: y) {
            return locale.language.languageCode?.identifier == "fr" ? "Hier" : "Yesterday"
        } else if let days = cal.dateComponents([.day], from: date, to: now).day, days < 7, days >= 0 {
            template = "EEE"
        } else {
            template = "dMMMy"
        }
        return Formatters.shared.string(date, template: template, calendar: cal)
    }
}

/// Date formatters per template, locale, calendar and time zone. Building one costs about
/// as much as laying out a list row, and every row asks for one on each redraw.
final class Formatters: @unchecked Sendable {
    static let shared = Formatters()
    private let lock = NSLock()
    private var cache: [String: DateFormatter] = [:]

    func string(_ date: Date, template: String, calendar: Calendar) -> String {
        let locale = calendar.locale ?? .current
        let key = "\(template)|\(locale.identifier)|\(calendar.identifier)|\(calendar.timeZone.identifier)"
        lock.lock()
        defer { lock.unlock() }
        if let f = cache[key] { return f.string(from: date) }
        let f = DateFormatter()
        f.locale = locale
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.setLocalizedDateFormatFromTemplate(template)
        cache[key] = f
        return f.string(from: date)
    }
}
