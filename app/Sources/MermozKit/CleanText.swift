/// The Optimized reading view: the AI's light Markdown (paragraphs, "- " lists, bold,
/// links) turned into styled text, line breaks kept.
import Foundation

public enum CleanText {
    public static func attributed(_ markdown: String) -> AttributedString {
        // "- " list lines read better as bullets; inline Markdown keeps every line break.
        let bulleted = markdown
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> String in
                let trimmed = line.drop { $0 == " " }
                guard trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") else { return String(line) }
                return String(line.prefix(line.count - trimmed.count)) + "• " + trimmed.dropFirst(2)
            }
            .joined(separator: "\n")
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: bulleted, options: options)) ?? AttributedString(markdown)
    }
}
