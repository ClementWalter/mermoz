/// Attachments in the composer: what the user added, its total size against
/// Gmail's limit, and how each item is handed to the daemon when the draft is saved.
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

public struct ComposeAttachment: Identifiable, Equatable, Sendable {
    public let id: UUID
    public var name: String
    public var size: Int64
    public var source: AttachmentSource
    /// Shown in the message by mail apps (pasted images) rather than offered as a download.
    public var inline: Bool

    public init(name: String, size: Int64, source: AttachmentSource, inline: Bool = false) {
        id = UUID(); self.name = name; self.size = size; self.source = source; self.inline = inline
    }

    /// Links cost nothing against the size limit.
    var countedSize: Int64 {
        if case .link = source { return 0 }
        return size
    }

    /// Spec for `drafts.create` / `drafts.attach`; nil for items the draft already has.
    public var spec: AttachSpec? {
        switch source {
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
            let source: AttachmentSource = if $0.kind == "link", let u = $0.url.flatMap(URL.init(string:)) {
                .link(u)
            } else {
                .stored(id: $0.id)
            }
            return ComposeAttachment(name: $0.filename, size: $0.size, source: source, inline: $0.inline)
        }
    }

    /// Adds the item unless the same file, message attachment or link is already
    /// there; returns whether it was added.
    @discardableResult
    public mutating func add(_ a: ComposeAttachment) -> Bool {
        let key = Self.dedupKey(a.source)
        if items.contains(where: { Self.dedupKey($0.source) == key }) { return false }
        items.append(a)
        return true
    }

    public mutating func remove(id: UUID) {
        guard let i = items.firstIndex(where: { $0.id == id }) else { return }
        if case let .stored(sid) = items[i].source { removedStored.append(sid) }
        items.remove(at: i)
    }

    public var isEmpty: Bool { items.isEmpty }

    public var totalSize: Int64 { items.reduce(0) { $0 + $1.countedSize } }

    public var isOverLimit: Bool { totalSize > Self.maxBytes }

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

    private static func dedupKey(_ s: AttachmentSource) -> String {
        switch s {
        case let .file(u): "file:" + u.standardizedFileURL.resolvingSymlinksInPath().path
        case let .message(id): "msg:" + id
        case let .stored(id): "stored:" + id
        case let .link(u): "link:" + u.absoluteString
        }
    }
}
