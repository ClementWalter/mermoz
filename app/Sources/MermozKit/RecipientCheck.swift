/// Local recipient-address heuristics behind the compose green/red marks.
import Foundation

public enum RecipientVerdict: Sendable, Equatable {
    /// Seen before in local mail.
    case known
    /// Well-formed, no red flags, but never seen.
    case unknown
    case suspicious(reason: String)
}

/// Fast, offline checks; a daemon-side check can later refine `.unknown`.
public struct RecipientChecker: Sendable {
    /// Lower-cased addresses that appear in local mail.
    public var knownAddresses: Set<String>
    /// Lower-cased display name -> lower-cased domains it was seen with.
    public var knownNames: [String: Set<String>]

    public init(knownAddresses: Set<String> = [], knownNames: [String: Set<String>] = [:]) {
        self.knownAddresses = knownAddresses
        self.knownNames = knownNames
    }

    /// Builds the checker from addresses seen in loaded messages.
    public init(addresses: [Address]) {
        var known = Set<String>()
        var names: [String: Set<String>] = [:]
        for a in addresses {
            let email = a.email.lowercased()
            known.insert(email)
            let name = a.name.trimmingCharacters(in: .whitespaces).lowercased()
            if !name.isEmpty, let d = RecipientChecker.domain(of: email) { names[name, default: []].insert(d) }
        }
        self.init(knownAddresses: known, knownNames: names)
    }

    public func check(_ raw: String, displayName: String? = nil) -> RecipientVerdict {
        let email = raw.trimmingCharacters(in: .whitespaces).lowercased()
        guard RecipientChecker.isWellFormed(email), let domain = RecipientChecker.domain(of: email) else {
            return .suspicious(reason: "This isn't a valid email address.")
        }
        if knownAddresses.contains(email) { return .known }
        if let reason = RecipientChecker.domainRedFlag(domain) { return .suspicious(reason: reason) }
        if let name = displayName ?? nameGuess(from: email), let domains = knownNames[name.lowercased()], !domains.contains(domain) {
            return .suspicious(reason: "This doesn't look like \(name.capitalized)'s usual address. You've never emailed this domain.")
        }
        if let lookalike = lookalikeOfKnownDomain(domain) {
            return .suspicious(reason: "This domain looks like \(lookalike) but isn't the same.")
        }
        return .unknown
    }

    /// "marc.petit@x" -> "marc petit", matched against known display names.
    func nameGuess(from email: String) -> String? {
        guard let local = email.split(separator: "@").first else { return nil }
        let parts = local.split(whereSeparator: { $0 == "." || $0 == "_" || $0 == "-" })
        guard parts.count >= 2 else { return nil }
        let guess = parts.joined(separator: " ")
        return knownNames[guess] != nil ? guess : nil
    }

    func lookalikeOfKnownDomain(_ domain: String) -> String? {
        let knownDomains = Set(knownAddresses.compactMap(RecipientChecker.domain(of:)))
        let normalized = RecipientChecker.deconfuse(domain)
        for k in knownDomains where k != domain {
            if RecipientChecker.deconfuse(k) == normalized || RecipientChecker.editDistance(k, domain) == 1 {
                return k
            }
        }
        return nil
    }

    public static func isWellFormed(_ email: String) -> Bool {
        let parts = email.split(separator: "@", omittingEmptySubsequences: false)
        guard parts.count == 2, !parts[0].isEmpty, parts[1].count >= 3 else { return false }
        let domain = parts[1]
        guard domain.contains("."), !domain.hasPrefix("."), !domain.hasSuffix("."), !domain.contains("..") else { return false }
        let allowedLocal = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: ".!#$%&'*+/=?^_`{|}~-"))
        let allowedDomain = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: ".-"))
        return parts[0].unicodeScalars.allSatisfy(allowedLocal.contains) && domain.unicodeScalars.allSatisfy(allowedDomain.contains)
    }

    public static func domain(of email: String) -> String? {
        guard let at = email.lastIndex(of: "@") else { return nil }
        let d = email[email.index(after: at)...]
        return d.isEmpty ? nil : String(d)
    }

    /// Red flags visible from the domain alone.
    public static func domainRedFlag(_ domain: String) -> String? {
        let labels = domain.split(separator: ".")
        guard let tld = labels.last else { return "This domain looks malformed." }
        let name = labels.dropLast().joined(separator: ".")
        if disposableDomains.contains(domain) { return "This is a throwaway email service." }
        // Letters swapped for look-alike digits (w0rk, paypa1) are a classic phishing tell.
        if name.range(of: "[a-z][01][a-z]", options: .regularExpression) != nil {
            return "This domain swaps letters for look-alike numbers."
        }
        if riskyTLDs.contains(String(tld)) && name.contains("-") {
            return "This domain looks newly made up."
        }
        for label in labels.dropLast() where label.count >= 12 && looksRandom(String(label)) {
            return "This domain looks randomly generated."
        }
        return nil
    }

    /// Few vowels or long consonant runs in a long label suggest a generated name.
    static func looksRandom(_ s: String) -> Bool {
        let letters = s.filter(\.isLetter)
        guard letters.count >= 10 else { return false }
        let vowels = letters.filter { "aeiouy".contains($0) }.count
        let digits = s.filter(\.isNumber).count
        var run = 0, maxRun = 0
        for c in letters {
            run = "aeiouy".contains(c) ? 0 : run + 1
            maxRun = max(maxRun, run)
        }
        return Double(vowels) / Double(letters.count) < 0.2 || maxRun >= 5 || digits >= 4
    }

    static func deconfuse(_ s: String) -> String {
        s.replacingOccurrences(of: "0", with: "o").replacingOccurrences(of: "1", with: "l")
            .replacingOccurrences(of: "rn", with: "m").replacingOccurrences(of: "vv", with: "w")
    }

    static func editDistance(_ a: String, _ b: String) -> Int {
        let a = Array(a), b = Array(b)
        if a.isEmpty { return b.count }
        if b.isEmpty { return a.count }
        var prev = Array(0...b.count)
        for i in 1...a.count {
            var cur = [i] + Array(repeating: 0, count: b.count)
            for j in 1...b.count {
                cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (a[i - 1] == b[j - 1] ? 0 : 1))
            }
            prev = cur
        }
        return prev[b.count]
    }

    static let disposableDomains: Set<String> = [
        "mailinator.com", "guerrillamail.com", "10minutemail.com", "tempmail.com", "temp-mail.org",
        "yopmail.com", "trashmail.com", "sharklasers.com", "getnada.com", "dispostable.com",
    ]

    static let riskyTLDs: Set<String> = ["xyz", "top", "click", "zip", "mov", "icu", "cfd", "sbs", "rest", "quest"]
}
