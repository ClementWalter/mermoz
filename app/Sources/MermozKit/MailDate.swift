/// Dates in the thread list: time today, "Yesterday", the weekday this week, else the
/// full date with its year, in the user's own order ("17 juil. 2026", "Jul 17, 2026").
import Foundation

public enum MailDate {
    public static func short(_ date: Date, now: Date = Date(), locale: Locale = .current, calendar: Calendar = .current) -> String {
        var cal = calendar
        cal.locale = locale
        let f = DateFormatter()
        f.locale = locale
        f.calendar = cal
        f.timeZone = cal.timeZone
        if cal.isDate(date, inSameDayAs: now) {
            f.setLocalizedDateFormatFromTemplate("jmm")
        } else if let y = cal.date(byAdding: .day, value: -1, to: now), cal.isDate(date, inSameDayAs: y) {
            return locale.language.languageCode?.identifier == "fr" ? "Hier" : "Yesterday"
        } else if let days = cal.dateComponents([.day], from: date, to: now).day, days < 7, days >= 0 {
            f.setLocalizedDateFormatFromTemplate("EEE")
        } else {
            f.setLocalizedDateFormatFromTemplate("dMMMy")
        }
        return f.string(from: date)
    }
}
