/// How an unsent draft reads in its conversation: a caption saying it is a draft (and
/// where it was written) and its own words, without the quoted message it answers.
import Foundation

public enum DraftPreview {
    /// "Draft", or "Draft from Gmail" for one written in Gmail on the web or a phone.
    public static func caption(_ d: Draft) -> String {
        d.origin == "gmail" ? "Draft from Gmail" : "Draft"
    }

    /// The draft's own lines: what follows a quote header ("On … wrote:", "Le … a écrit :")
    /// or a quoted line ("> …") is the message it answers.
    public static func text(_ body: String) -> String {
        var kept: [Substring] = []
        for line in body.split(separator: "\n", omittingEmptySubsequences: false) {
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix(">") || isQuoteHeader(t) { break }
            kept.append(line)
        }
        return kept.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    static func isQuoteHeader(_ line: String) -> Bool {
        let l = line.lowercased()
        return (l.hasPrefix("on ") && l.hasSuffix("wrote:")) || (l.hasPrefix("le ") && l.hasSuffix("a écrit :"))
            || (l.hasPrefix("le ") && l.hasSuffix("a écrit:"))
    }
}
