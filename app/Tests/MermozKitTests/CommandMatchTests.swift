/// The palette finds commands by the start of their words, accents and case ignored.
import Testing
@testable import MermozKit

@Test(arguments: [
    ("move proj", "Move to project…", [String](), true),
    ("project", "Move to project…", [], true),
    ("label", "Move to project…", ["label", "folder"], true),
    ("arch", "Archive", [], true),
    ("trash", "Delete", ["trash", "bin"], true),
    ("émoji", "Emoji picker", [], true),
    ("move xyz", "Move to project…", [], false),
    ("roject", "Move to project…", [], false),
])
func commandMatching(_ query: String, _ title: String, _ keywords: [String], _ matches: Bool) {
    #expect((CommandMatch.score(query, title: title, keywords: keywords) > 0) == matches)
}

@Test func titleWordsRankAboveKeywords() {
    #expect(CommandMatch.score("arch", title: "Archive") > CommandMatch.score("arch", title: "Delete", keywords: ["archive"]))
}
