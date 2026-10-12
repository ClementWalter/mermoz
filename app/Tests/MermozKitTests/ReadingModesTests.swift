/// Original / Optimized is chosen per message, not for the whole thread.
import Foundation
import Testing
@testable import MermozKit

@Test func untouchedMessageShowsOriginal() {
    #expect(ReadingModes()[message: "m1"] == .original)
}

@Test func settingOneMessageChangesIt() {
    var r = ReadingModes()
    r[message: "m1"] = .optimized
    #expect(r[message: "m1"] == .optimized)
}

@Test func settingOneMessageLeavesTheOthers() {
    var r = ReadingModes()
    r[message: "m1"] = .optimized
    #expect(r[message: "m2"] == .original)
}

@Test func switchingBackToOriginalStoresNothing() {
    var r = ReadingModes()
    r[message: "m1"] = .optimized
    r[message: "m1"] = .original
    #expect(r == ReadingModes())
}

@Test func oldestChoiceIsForgottenPastTheLimit() {
    var r = ReadingModes(limit: 2)
    r[message: "m1"] = .optimized
    r[message: "m2"] = .optimized
    r[message: "m3"] = .optimized
    #expect(r[message: "m1"] == .original)
}

@Test func reSettingAChoiceKeepsItFresh() {
    var r = ReadingModes(limit: 2)
    r[message: "m1"] = .optimized
    r[message: "m2"] = .optimized
    r[message: "m1"] = .optimized
    r[message: "m3"] = .optimized
    #expect(r[message: "m1"] == .optimized)
}

@Test func choicesSurviveSaving() throws {
    var r = ReadingModes()
    r[message: "m1"] = .optimized
    let back = try JSONDecoder().decode(ReadingModes.self, from: JSONEncoder().encode(r))
    #expect(back[message: "m1"] == .optimized)
}
