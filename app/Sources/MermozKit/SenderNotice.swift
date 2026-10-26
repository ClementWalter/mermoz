/// What the reader is told about a sender the account has no reason to trust yet: a
/// first email from its domain, or a lookalike of someone else. Plain words only.
import Foundation

/// Who a sender's name claims to be, and the domains that organisation really sends
/// email from, as a web search found them.
public struct SenderClaim: Codable, Sendable, Equatable {
    /// "organisation", "person" or "unknown".
    public var kind: String
    public var organisation: String
    public var domains: [String]
    public var sources: [String]
    /// Whether the sender's own domain is one of `domains`.
    public var matches: Bool

    public init(kind: String, organisation: String, domains: [String], sources: [String] = [], matches: Bool) {
        self.kind = kind
        self.organisation = organisation
        self.domains = domains
        self.sources = sources
        self.matches = matches
    }
}

public enum SenderNotice {
    /// The short label shown next to the sender's name, if any.
    public static func label(_ m: Message) -> String? {
        switch m.auth {
        case "new": return "First email"
        case "lookalike": return "Lookalike"
        default: return nil
        }
    }

    /// The full explanation shown on hover.
    public static func detail(_ m: Message) -> String? {
        let name = m.from.display, domain = m.from.domain
        let org = m.senderClaim.flatMap { $0.kind == "organisation" && !$0.organisation.isEmpty ? $0 : nil }
        switch m.auth {
        case "new":
            let first = "First email from \(domain): you have never received mail from it before."
            guard let org else { return first + " Be careful with links and attachments." }
            if org.matches { return first + " It is \(org.organisation)'s official address." }
            if let official = org.domains.first {
                return first + " It claims to be \(org.organisation), whose emails come from \(official). Be careful with links and attachments."
            }
            return first + " Be careful with links and attachments."
        case "lookalike":
            let usual = m.usualDomain.map { official in
                org.map { " \($0.organisation)'s emails come from \(official)." } ?? " “\(name)” usually writes from \(official)."
            } ?? ""
            return "Likely not from “\(name)”: \(domain) is a lookalike address.\(usual) Don't click its links or open its attachments."
        default:
            return nil
        }
    }
}
