/// The one-line summary of who a reply goes to, as the inline composer shows it:
/// "to Elliot, +2 cc".
import Foundation

public enum RecipientLine {
    /// The first recipient by first name (their address when no name is known), how many
    /// more are in To, and how many in Cc.
    public static func compact(to: [String], cc: [String], names: [String: String]) -> String {
        guard let first = to.first ?? cc.first else { return "No recipients" }
        let name = names[first.lowercased()].map { $0.split(separator: " ").first.map(String.init) ?? $0 } ?? first
        var line = "to \(name)"
        let moreTo = to.isEmpty ? 0 : to.count - 1
        let moreCc = to.isEmpty ? cc.count - 1 : cc.count
        if moreTo > 0 { line += " +\(moreTo)" }
        if moreCc > 0 { line += ", +\(moreCc) cc" }
        return line
    }
}
