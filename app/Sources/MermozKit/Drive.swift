/// Google Drive in the app: the daemon's `drive.*` payloads, what a linked file's card
/// shows, how a Drive connection reads in Settings, which files a reply must share,
/// and the Drive files of a conversation's Docs tab. UI-free so it is unit-testable.
import Foundation

/// A Drive file as the daemon describes it.
public struct DriveFile: Codable, Sendable, Equatable, Hashable, Identifiable {
    public struct Owner: Codable, Sendable, Equatable, Hashable {
        public var name: String
        public var email: String
    }

    public var id: String
    public var name: String
    public var mime: String
    /// "Google Doc", "PDF"…
    public var kind: String
    /// Last change, Unix ms (0 when Drive did not say).
    public var modified: Int64
    public var owner: Owner?
    public var webLink: String?
    public var hasThumbnail: Bool

    public init(id: String, name: String, mime: String = "", kind: String = "File", modified: Int64 = 0,
                owner: Owner? = nil, webLink: String? = nil, hasThumbnail: Bool = false) {
        self.id = id; self.name = name; self.mime = mime; self.kind = kind; self.modified = modified
        self.owner = owner; self.webLink = webLink; self.hasThumbnail = hasThumbnail
    }

    /// Where "Open in Drive" goes.
    public var openURL: URL? { webLink.flatMap(URL.init(string:)) ?? URL(string: "https://drive.google.com/open?id=\(id)") }
}

/// A Drive link found in a conversation's mail.
public struct DriveLinkItem: Codable, Sendable, Equatable, Identifiable {
    public var fileId: String
    public var url: String
    public var messageId: String
    /// "ok", "no_access", "not_connected" or "error".
    public var access: String
    public var file: DriveFile?

    public var id: String { fileId }

    public init(fileId: String, url: String, messageId: String, access: String, file: DriveFile? = nil) {
        self.fileId = fileId; self.url = url; self.messageId = messageId; self.access = access; self.file = file
    }
}

/// An attachment put in Drive by Mermoz.
public struct SavedToDrive: Codable, Sendable, Equatable {
    public var attachmentId: String
    public var fileId: String
    public var name: String
    public var webLink: String?
    /// "Mermoz/Clients/Acme".
    public var folder: String

    public init(attachmentId: String, fileId: String, name: String, webLink: String?, folder: String) {
        self.attachmentId = attachmentId; self.fileId = fileId; self.name = name; self.webLink = webLink; self.folder = folder
    }

    public var openURL: URL? { webLink.flatMap(URL.init(string:)) ?? URL(string: "https://drive.google.com/open?id=\(fileId)") }

    /// "Saved to Mermoz › Clients › Acme".
    public var savedLine: String { "Saved to " + folder.split(separator: "/").joined(separator: " › ") }
}

/// What `drive.files_for_thread` returns: the conversation's Drive files.
public struct DriveThreadFiles: Codable, Sendable, Equatable {
    public var accountId: String
    public var connected: Bool
    public var canReadAll: Bool
    public var linked: [DriveLinkItem]
    public var shared: [DriveFile]
    public var saved: [SavedToDrive]

    public init(accountId: String, connected: Bool, canReadAll: Bool = false, linked: [DriveLinkItem] = [],
                shared: [DriveFile] = [], saved: [SavedToDrive] = []) {
        self.accountId = accountId; self.connected = connected; self.canReadAll = canReadAll
        self.linked = linked; self.shared = shared; self.saved = saved
    }

    /// Entries for the Docs tab: linked files Mermoz could open, then files shared
    /// with these people; each file once.
    public var docEntries: [DocEntry] {
        var seen = Set<String>()
        var out: [DocEntry] = []
        let files = linked.compactMap(\.file) + shared
        for f in files where seen.insert(f.id).inserted {
            let url = f.openURL?.absoluteString ?? ""
            out.append(.linked(id: f.id, title: f.name, url: url, from: f.owner?.name ?? f.kind, date: f.modified))
        }
        return out
    }

    /// Links in one message, in the order they appear.
    public func links(in messageId: String) -> [DriveLinkItem] { linked.filter { $0.messageId == messageId } }

    public func saved(_ attachmentId: String) -> SavedToDrive? { saved.first { $0.attachmentId == attachmentId } }
}

/// One account's Drive connection.
public struct DriveStatus: Codable, Sendable, Equatable {
    public struct Attempt: Codable, Sendable, Equatable {
        /// "waiting", "connected", "declined" or "error".
        public var state: String
        public var error: String?
    }

    public var accountId: String
    public var email: String
    public var connected: Bool
    public var canReadAll: Bool
    public var canSave: Bool
    public var attempt: Attempt?

    public init(accountId: String, email: String, connected: Bool, canReadAll: Bool = false, canSave: Bool = false,
                attempt: Attempt? = nil) {
        self.accountId = accountId; self.email = email; self.connected = connected
        self.canReadAll = canReadAll; self.canSave = canSave; self.attempt = attempt
    }

    public var isWaiting: Bool { attempt?.state == "waiting" }

    /// Whether a Drive feature should first ask to connect Drive. Unknown status (not
    /// loaded yet) doesn't ask: the daemon's "not connected" answer will.
    public static func needsConnect(_ s: DriveStatus?) -> Bool { s.map { !$0.connected } ?? false }

    /// One plain sentence for Settings.
    public var summary: String {
        if isWaiting { return "Finish in your browser: Google is asking for your permission." }
        if connected {
            switch (canReadAll, canSave) {
            case (true, true): return "Connected. Mermoz shows documents linked in your mail and can save attachments to your Drive."
            case (true, false): return "Connected to read your files. Saving attachments needs permission to add files: connect again and keep that box ticked."
            case (false, true): return "Connected to files you choose. Linked documents need one click to open, and files shared with your contacts don't show."
            case (false, false): return "Connected."
            }
        }
        switch attempt?.state {
        case "declined": return "Not connected: you said no in Google. Your mail keeps working as before."
        case "error": return "Couldn't connect. \(attempt?.error ?? "Try again.")"
        default: return "See documents linked in your mail, save attachments to your Drive and attach Drive files to replies."
        }
    }
}

public struct DrivePick: Codable, Sendable, Equatable {
    public var accountId: String
    /// "waiting", "done", "declined" or "error".
    public var state: String
    public var files: [DriveFile]
    public var error: String?
}

public struct DrivePickStart: Codable, Sendable {
    public var pickId: String
    public var authUrl: String
}

public struct DriveFileList: Codable, Sendable {
    public var files: [DriveFile]
}

public struct DriveShareResult: Codable, Sendable, Equatable {
    public struct Failure: Codable, Sendable, Equatable {
        public var email: String
        public var reason: String
    }

    public var shared: [String]
    public var failed: [Failure]
}

public struct DriveThumbnail: Codable, Sendable {
    public var mime: String
    /// Base64.
    public var data: String
}

/// What a Drive error asks of the user.
public enum DriveNeed: Equatable, Sendable {
    /// Drive is not connected for this account.
    case connect(accountId: String)
    /// The "see all your files" permission was not granted.
    case readAll
    /// The "add files to your Drive" permission was not granted.
    case save
    /// The file must be chosen in Google's picker before Mermoz may use it.
    case pick

    public static let notConnectedCode = -32030
    public static let needsAccessCode = -32031

    /// The need behind a daemon error, if it is a Drive one.
    public init?(_ e: RPCError) {
        struct Data: Decodable { var accountId: String?; var reason: String? }
        let d = e.data.flatMap { try? RPCCoding.decoder().decode(Data.self, from: $0) }
        switch (e.code, d?.reason) {
        case (Self.notConnectedCode, _): self = .connect(accountId: d?.accountId ?? "")
        case (Self.needsAccessCode, "read_all"): self = .readAll
        case (Self.needsAccessCode, "save"): self = .save
        case (Self.needsAccessCode, _): self = .pick
        default: return nil
        }
    }
}

/// What the card under a message shows for one Drive link.
public enum DriveCardState: Equatable, Sendable {
    /// Title, type, owner, date (and a thumbnail when there is one).
    case preview(DriveFile)
    /// Drive isn't connected: offer to connect.
    case connect
    /// Mermoz may not open it yet: offer "Allow" (Google's picker, on this file).
    case allow
    /// Drive didn't answer; the link still opens in the browser.
    case unavailable

    public init(_ link: DriveLinkItem) {
        switch (link.access, link.file) {
        case let (_, file?): self = .preview(file)
        case ("not_connected", nil): self = .connect
        case ("no_access", nil): self = .allow
        default: self = .unavailable
        }
    }
}

/// Drive file ids in links, for files attached to a reply as links.
public enum DriveLinks {
    /// The file id of a docs.google.com or drive.google.com link, else nil.
    public static func fileId(_ url: URL) -> String? {
        guard let host = url.host?.lowercased(), host == "docs.google.com" || host == "drive.google.com" else { return nil }
        let parts = url.path.split(separator: "/").map(String.init)
        if let d = parts.firstIndex(of: "d"), d + 1 < parts.count, parts[d + 1] != "e" { return parts[d + 1] }
        if let f = parts.firstIndex(of: "folders"), f + 1 < parts.count { return parts[f + 1] }
        return URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "id" }?.value
    }
}

/// Before a reply goes out with Drive files: who would be given access, and the
/// question asked. Nothing is shared unless the user picks an answer.
public struct SharePlan: Equatable, Sendable {
    public struct File: Equatable, Sendable {
        public var id: String
        public var name: String
        public init(id: String, name: String) { self.id = id; self.name = name }
    }

    public var files: [File]
    public var emails: [String]

    /// Drive links among the attachments, and every recipient but the user.
    public init(attachments: [ComposeAttachment], recipients: [String], me: String?) {
        var files: [File] = []
        for a in attachments {
            if case let .link(u) = a.source, let id = DriveLinks.fileId(u), !files.contains(where: { $0.id == id }) {
                files.append(File(id: id, name: a.name))
            }
        }
        self.files = files
        var emails: [String] = []
        for r in recipients {
            let e = r.trimmingCharacters(in: .whitespaces).lowercased()
            if !e.isEmpty, e != me?.lowercased(), !emails.contains(e) { emails.append(e) }
        }
        self.emails = emails
    }

    /// Asks only when there is something to share and someone to share it with.
    public var isNeeded: Bool { !files.isEmpty && !emails.isEmpty }

    /// "Let the recipients open “Q4 forecast”?"
    public var question: String {
        let what = files.count == 1 ? "“\(files[0].name)”" : "these \(files.count) Google Drive files"
        let who = emails.count == 1 ? emails[0] : "the \(emails.count) recipients"
        return "Let \(who) open \(what)?"
    }

    public var detail: String {
        "They may not be able to open \(files.count == 1 ? "it" : "them") otherwise. Google Drive won't send them a separate email: your message carries the link."
    }
}

/// How a Drive file goes into an email: a link the recipients open in Drive, or the
/// file itself, downloaded (Google files exported, PDF by default).
public enum DriveAttachMode: String, Sendable, CaseIterable {
    case link
    case file

    /// Whether every recipient is outside the user's Google world: no Gmail address
    /// and none in the user's own Workspace domain (an administration, a supplier).
    /// Such people often can't or won't open a Drive link. False without recipients.
    public static func allOutside(recipients: [String], me: String?) -> Bool {
        let consumer: Set<String> = ["gmail.com", "googlemail.com"]
        let myDomain = me.flatMap(domain).flatMap { consumer.contains($0) ? nil : $0 }
        let domains = recipients.compactMap(domain)
        guard !domains.isEmpty, domains.count == recipients.count else { return false }
        return domains.allSatisfy { !consumer.contains($0) && $0 != myDomain }
    }

    /// The mode offered first: files for people outside the user's Google world, links
    /// otherwise, unless the user last chose differently for that kind of audience.
    public static func preferred(recipients: [String], me: String?, memory: DriveAttachMemory) -> DriveAttachMode {
        let outside = allOutside(recipients: recipients, me: me)
        return memory.last(outside: outside) ?? (outside ? .file : .link)
    }

    /// The formats a Google file can be attached as, PDF first; empty for stored files
    /// (sent as they are) and Google files with no file form.
    public static func formats(mime: String) -> [String] {
        switch mime {
        case "application/vnd.google-apps.document": ["pdf", "docx"]
        case "application/vnd.google-apps.spreadsheet": ["pdf", "xlsx"]
        case "application/vnd.google-apps.presentation": ["pdf", "pptx"]
        case "application/vnd.google-apps.drawing": ["pdf"]
        default: []
        }
    }

    /// The chip menu's name for a format.
    public static func formatLabel(_ format: String) -> String {
        switch format {
        case "pdf": "PDF"
        case "docx": "Word (.docx)"
        case "xlsx": "Excel (.xlsx)"
        case "pptx": "PowerPoint (.pptx)"
        default: "." + format
        }
    }

    private static func domain(_ address: String) -> String? {
        let a = address.trimmingCharacters(in: .whitespaces).lowercased()
        guard let at = a.lastIndex(of: "@") else { return nil }
        let d = a[a.index(after: at)...].trimmingCharacters(in: CharacterSet(charactersIn: ">"))
        return d.isEmpty ? nil : d
    }
}

/// The user's last link-or-file choice, one per kind of audience (everyone outside
/// their Google world, or not), kept across launches.
public struct DriveAttachMemory: @unchecked Sendable {
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) { self.defaults = defaults }

    private static func key(outside: Bool) -> String { outside ? "driveAttachMode.outside" : "driveAttachMode.inside" }

    public func last(outside: Bool) -> DriveAttachMode? {
        defaults.string(forKey: Self.key(outside: outside)).flatMap(DriveAttachMode.init(rawValue:))
    }

    public func remember(_ mode: DriveAttachMode, outside: Bool) {
        defaults.set(mode.rawValue, forKey: Self.key(outside: outside))
    }
}
