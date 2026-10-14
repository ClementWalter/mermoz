/// Search by meaning: its settings line and when Gmail's own search is still offered.
import Foundation
import Testing
@testable import MermozKit

private func decode<T: Decodable>(_ s: String) throws -> T {
    try RPCCoding.decoder().decode(T.self, from: Data(s.utf8))
}

@Test(arguments: [
    (SmartSearchStatus(enabled: true, phase: "indexing", progress: 42), "Improving search… 42%"),
    (SmartSearchStatus(enabled: true, phase: "downloading", progress: 7), "Improving search… 7%"),
    (SmartSearchStatus(enabled: true, phase: "error", progress: 30), "Improving search is paused. Mermoz will try again later."),
    (SmartSearchStatus(enabled: true, phase: "ready", progress: 100), nil),
    (SmartSearchStatus(enabled: false, phase: "off", progress: 0), nil),
] as [(SmartSearchStatus, String?)])
func smartSearchLine(status: SmartSearchStatus, line: String?) {
    #expect(status.line == line)
}

@Test func smartSearchStatusDecodes() throws {
    let s: SmartSearchStatus = try decode(#"{"enabled":true,"phase":"indexing","progress":42,"indexed":10,"total":20}"#)
    #expect(s == SmartSearchStatus(enabled: true, phase: "indexing", progress: 42))
}

@Test func searchResultDecodesWordMatches() throws {
    let r: SearchResult = try decode(#"{"threads":[],"word_matches":0}"#)
    #expect(r.wordMatches == 0)
}

@Test(arguments: [
    (SearchResult(threads: [], wordMatches: 0), true),
    (SearchResult(threads: [], wordMatches: nil), true),
    (SearchResult(threads: [], wordMatches: 2), false),
])
func gmailIsOfferedWithoutWordMatch(result: SearchResult, offered: Bool) {
    #expect(result.noWordMatch == offered)
}
