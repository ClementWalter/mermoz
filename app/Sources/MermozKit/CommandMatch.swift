/// Matching for the ⌘K palette: every typed word must start a word of the command's
/// title or one of its keywords, so "move proj" finds "Move to project…".
import Foundation

public enum CommandMatch {
    /// 0 = no match; higher is better (title words beat keywords, earlier beats later).
    public static func score(_ query: String, title: String, keywords: [String] = []) -> Int {
        let q = words(query)
        guard !q.isEmpty else { return 1 }
        let t = words(title)
        let k = keywords.flatMap(words)
        var total = 0
        for w in q {
            if let i = t.firstIndex(where: { $0.hasPrefix(w) }) {
                total += 100 - i
            } else if k.contains(where: { $0.hasPrefix(w) }) {
                total += 10
            } else {
                return 0
            }
        }
        return total
    }

    static func words(_ s: String) -> [String] {
        s.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
            .split(whereSeparator: { !$0.isLetter && !$0.isNumber })
            .map(String.init)
    }
}
