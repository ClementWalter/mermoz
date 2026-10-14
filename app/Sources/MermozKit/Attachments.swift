/// Attachments in the composer: what the user added, its total size against
/// Gmail's limit, and how each item is handed to the daemon when the draft is saved.
/// A Drive file goes out as a link or, downloaded by the daemon, as a regular file.
import Foundation

/// A file or link saved with a draft by the daemon.
public struct DraftAttachment: Codable, Sendable, Identifiable, Equatable, Hashable {
    public var id: String
    /// "file" or "link".
    public var kind: String
    public var filename: String
    public var mime: String
    public var size: Int64
    public var path: String?
    public var url: String?
    public var inline: Bool

    public init(id: String, kind: String = "file", filename: String, mime: String, size: Int64,
                path: String? = nil, url: String? = nil, inline: Bool = false) {
        self.id = id; self.kind = kind; self.filename = filename; self.mime = mime
        self.size = size; self.path = path; self.url = url; self.inline = inline
    }
}

/// Where a composer attachment comes from.
public enum AttachmentSource: Equatable, Hashable, Sendable {
    /// A file on this Mac; the daemon copies it when the draft is saved.
    case file(URL)
    /// An attachment of a message already in Mermoz.
    case message(attachmentId: String)
    /// Already saved with the draft being edited.
    case stored(id: String)
    /// Shared by link rather than by bytes (e.g. a Drive file).
    case link(URL)
}

/// The Drive file behind a composer item, so it can switch between link and file.
public struct DriveOrigin: Equatable, Hashable, Sendable {
    public var fileId: String
    public var name: String
    /// Drive's type ("application/pdf", "application/vnd.google-apps.document"…); empty when unknown.
    public var mime: String
    public var link: URL

    public init(fileId: String, name: String, mime: String, link: URL) {
        self.fileId = fileId; self.name = name; self.mime = mime; self.link = link
    }
}

/// Where a Drive item stands on its way to becoming a regular file.
public enum DriveFetch: Equatable, Sendable {
    case idle
    case downloading
    /// Plain-words reason, shown on the chip with "Keep as link".
    case failed(String)
}

/// What the daemon's `drive.download` saved on this Mac.
public struct DriveDownload: Codable, Sendable, Equatable {
    public var fileId: String
    public var path: String
    public var filename: String
    public var mime: String
    public var size: Int64

    public init(fileId: String, path: String, filename: String, mime: String, size: Int64) {
        self.fileId = fileId; self.path = path; self.filename = filename; self.mime = mime; self.size = size
    }
}

public struct ComposeAttachment: Identifiable, Equatable, Sendable {
    public let id: UUID
    public var name: String
    public var size: Int64
    public var source: AttachmentSource
    /// Shown in the message by mail apps (pasted images) rather than offered as a download.
    public var inline: Bool
    /// Set for Drive files: they can go out as a link or as a file.
    public var drive: DriveOrigin?
    public var fetch: DriveFetch = .idle
    /// The saved draft attachment this item is (a link kept by the draft being edited).
    public var storedId: String?

    public init(name: String, size: Int64, source: AttachmentSource, inline: Bool = false, drive: DriveOrigin? = nil) {
        id = UUID(); self.name = name; self.size = size; self.source = source; self.inline = inline; self.drive = drive
    }

    /// A Drive file, as a link until it is downloaded.
    public init(drive: DriveOrigin) {
        self.init(name: drive.name, size: 0, source: .link(drive.link), drive: drive)
    }

    public var isLink: Bool {
        if case .link = source { return true }
        return false
    }

    /// Links cost nothing against the size limit.
    var countedSize: Int64 { isLink ? 0 : size }

    /// Spec for `drafts.create` / `drafts.attach`; nil for items the draft already has.
    public var spec: AttachSpec? {
        if storedId != nil { return nil }
        return switch source {
        case let .file(url): AttachSpec(path: url.path, filename: name, inline: inline)
        case let .message(id): AttachSpec(fromAttachment: id, filename: name, inline: inline)
        case let .link(url): AttachSpec(url: url.absoluteString, filename: name)
        case .stored: nil
        }
    }
}

/// One attachment as the daemon's protocol spells it.
public struct AttachSpec: Encodable, Sendable, Equatable {
    public var path: String?
    public var fromAttachment: String?
    public var url: String?
    public var filename: String?
    public var inline: Bool = false

    public init(path: String? = nil, fromAttachment: String? = nil, url: String? = nil,
                filename: String? = nil, inline: Bool = false) {
        self.path = path; self.fromAttachment = fromAttachment; self.url = url
        self.filename = filename; self.inline = inline
    }

    enum CodingKeys: String, CodingKey {
        case path, url, filename, inline
        case fromAttachment = "from_attachment"
    }
}

/// The composer's attachments, in the order they were added.
public struct AttachmentList: Equatable, Sendable {
    /// Gmail sends at most 25 MB of files in one email.
    public static let maxBytes: Int64 = 25 * 1024 * 1024

    public private(set) var items: [ComposeAttachment] = []
    /// Ids of saved attachments the user removed; detached from the draft on send.
    public private(set) var removedStored: [String] = []

    public init() {}

    /// Starts from what an existing draft already carries.
    public init(stored: [DraftAttachment]) {
        items = stored.map {
            guard $0.kind == "link", let u = $0.url.flatMap(URL.init(string:)) else {
                return ComposeAttachment(name: $0.filename, size: $0.size, source: .stored(id: $0.id), inline: $0.inline)
            }
            let name = $0.filename
            let origin = DriveLinks.fileId(u).map { DriveOrigin(fileId: $0, name: name, mime: "", link: u) }
            var a = ComposeAttachment(name: name, size: $0.size, source: .link(u), drive: origin)
            a.storedId = $0.id
            return a
        }
    }

    /// Adds the item unless the same file, message attachment, Drive file or link is
    /// already there; returns whether it was added.
    @discardableResult
    public mutating func add(_ a: ComposeAttachment) -> Bool {
        let key = Self.dedupKey(a)
        if items.contains(where: { Self.dedupKey($0) == key }) { return false }
        items.append(a)
        return true
    }

    public mutating func remove(id: UUID) {
        guard let i = items.firstIndex(where: { $0.id == id }) else { return }
        if case let .stored(sid) = items[i].source { removedStored.append(sid) }
        if let sid = items[i].storedId { removedStored.append(sid) }
        items.remove(at: i)
    }

    // MARK: Drive files as links or files

    /// The daemon is fetching this Drive item's bytes.
    public mutating func startDownload(id: UUID) {
        guard let i = index(id), items[i].drive != nil else { return }
        items[i].fetch = .downloading
    }

    /// The Drive item becomes a regular file: its bytes, name and size.
    public mutating func downloaded(id: UUID, _ d: DriveDownload) {
        guard let i = index(id) else { return }
        if let sid = items[i].storedId { removedStored.append(sid) }
        items[i].storedId = nil
        items[i].source = .file(URL(fileURLWithPath: d.path))
        items[i].name = d.filename
        items[i].size = d.size
        items[i].fetch = .idle
    }

    /// The download failed: the item stays a link and says why.
    public mutating func downloadFailed(id: UUID, _ reason: String) {
        guard let i = index(id) else { return }
        items[i].fetch = .failed(reason)
    }

    /// Back to a link (from a file, or after a failed download).
    public mutating func keepAsLink(id: UUID) {
        guard let i = index(id), let origin = items[i].drive else { return }
        items[i].fetch = .idle
        guard !items[i].isLink else { return }
        items[i].source = .link(origin.link)
        items[i].name = origin.name
        items[i].size = 0
    }

    public var isEmpty: Bool { items.isEmpty }

    public var totalSize: Int64 { items.reduce(0) { $0 + $1.countedSize } }

    public var isOverLimit: Bool { totalSize > Self.maxBytes }

    /// A Drive file is still downloading.
    public var isDownloading: Bool { items.contains { $0.fetch == .downloading } }

    /// A Drive file the user wanted as a file couldn't be fetched, and they haven't chosen yet.
    public var hasFailedDownload: Bool {
        items.contains { if case .failed = $0.fetch { true } else { false } }
    }

    /// Sending waits until every file is there and fits.
    public var blocksSend: Bool { isOverLimit || isDownloading || hasFailedDownload }

    /// Plain-words explanation when the files are too big to send, else nil.
    public var limitMessage: String? {
        guard isOverLimit else { return nil }
        return "These files add up to \(Self.format(totalSize)). Gmail can send at most \(Self.format(Self.maxBytes)) per email: remove some files, or share the large ones with a link."
    }

    /// Items the draft does not have yet, as daemon specs.
    public var newSpecs: [AttachSpec] { items.compactMap(\.spec) }

    /// "820 KB", "3.4 MB": the size shown on chips.
    public static func format(_ bytes: Int64) -> String {
        let mb = 1024.0 * 1024.0
        if Double(bytes) >= mb { return String(format: "%.1f MB", Double(bytes) / mb) }
        return "\(max(1, (bytes + 1023) / 1024)) KB"
    }

    private func index(_ id: UUID) -> Int? { items.firstIndex { $0.id == id } }

    private static func dedupKey(_ a: ComposeAttachment) -> String {
        if let d = a.drive { return "drive:" + d.fileId }
        return switch a.source {
        case let .file(u): "file:" + u.standardizedFileURL.resolvingSymlinksInPath().path
        case let .message(id): "msg:" + id
        case let .stored(id): "stored:" + id
        case let .link(u): "link:" + u.absoluteString
        }
    }
}
