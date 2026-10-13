/// Later picker: choices and typed times decode as mermozd sends them, keys pick choices,
/// and the strip and the hint read naturally.
import Foundation
import Testing
@testable import MermozKit

private let optionsJSON = #"[{"key":"e","label":"This evening","until":1791820800000,"when":"today, 18:00"},{"key":"f","label":"Friday evening","until":1792166400000,"when":"Fri 18:00"}]"#

private func choices() throws -> [LaterChoice] {
    try RPCCoding.decoder().decode([LaterChoice].self, from: Data(optionsJSON.utf8))
}

@Test func choicesDecode() throws {
    #expect(try choices().count == 2)
}

@Test func aTypedTimeDecodes() throws {
    let t = try RPCCoding.decoder().decode(LaterTime?.self, from: Data(#"{"until":1792166400000,"when":"Fri 18:00"}"#.utf8))
    #expect(t?.when == "Fri 18:00")
}

@Test func unclearWordsDecodeAsNothing() throws {
    #expect(try RPCCoding.decoder().decode(LaterTime?.self, from: Data("null".utf8)) == nil)
}

@Test(arguments: [("f", "Friday evening"), ("e", "This evening"), ("F", "Friday evening")])
func aKeyPicksItsChoice(key: String, label: String) throws {
    #expect(LaterKeys.choice(for: key, in: try choices())?.label == label)
}

@Test func anUnknownKeyPicksNothing() throws {
    #expect(LaterKeys.choice(for: "z", in: try choices()) == nil)
}

@Test func theStripSaysWhen() {
    #expect(LaterKeys.strip("Fri 18:00") == "Later · Fri 18:00")
}

@Test(arguments: [("Friday evening", "Friday evening"), ("Tomorrow morning", "tomorrow morning"),
                  ("Next week", "next week"), ("Fri 18:00", "Fri 18:00")])
func theHintReadsNaturally(label: String, want: String) {
    #expect(LaterKeys.hint(label) == want)
}
