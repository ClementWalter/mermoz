/// List dates always carry the year once older than a week, in the reader's order.
import Foundation
import Testing
@testable import MermozKit

private var utc: Calendar {
    var c = Calendar(identifier: .gregorian)
    c.timeZone = TimeZone(identifier: "UTC")!
    return c
}

private func day(_ s: String) -> Date {
    let f = ISO8601DateFormatter()
    return f.date(from: s)!
}

private let now = day("2026-10-12T15:00:00Z")

@Test func frenchDateNamesTheYear() {
    #expect(MailDate.short(day("2026-07-17T09:00:00Z"), now: now, locale: Locale(identifier: "fr_FR"), calendar: utc) == "17 juil. 2026")
}

@Test func englishDateNamesTheYear() {
    #expect(MailDate.short(day("2026-07-17T09:00:00Z"), now: now, locale: Locale(identifier: "en_US"), calendar: utc) == "Jul 17, 2026")
}

@Test func todayShowsTheTime() {
    #expect(MailDate.short(day("2026-10-12T09:05:00Z"), now: now, locale: Locale(identifier: "fr_FR"), calendar: utc) == "09:05")
}
