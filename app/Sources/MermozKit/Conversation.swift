/// Conversations: Gmail threads with the same people about the same matter, read as one
/// chat. The daemon groups them; this file holds their protocol objects and the chat
/// timeline the reading pane draws.
import Foundation

public struct ConversationPerson: Codable, Sendable, Equatable, Hashable, Identifiable {
    public var name: String
    public var email: String
    public var title: String
    public var company: String
    /// Messages they wrote in the conversation.
    public var messages: Int
    public var id: String { email }

    public var display: String { name.isEmpty ? email : name }
    public var role: String { [title, company].filter { !$0.isEmpty }.joined(separator: " · ") }
}

public struct SharedLink: Codable, Sendable, Equatable, Hashable, Identifiable {
    public var url: String
    public var messageId: String
    public var from: String
    public var date: Int64
    public var id: String { url }

    /// "notary.example/rdv" for a link shown as text.
    public var short: String {
        var s = url
        for p in ["https://", "http://", "www."] where s.hasPrefix(p) { s.removeFirst(p.count) }
        return s.hasSuffix("/") ? String(s.dropLast()) : s
    }
}

public struct SharedFile: Codable, Sendable, Equatable, Hashable, Identifiable {
    public var id: String
    public var filename: String
    public var mime: String
    public var size: Int64
    public var messageId: String
    public var threadId: String
    public var from: String
    public var date: Int64

    public init(id: String, filename: String, mime: String, size: Int64, messageId: String, threadId: String,
                from: String, date: Int64) {
        self.id = id; self.filename = filename; self.mime = mime; self.size = size
        self.messageId = messageId; self.threadId = threadId; self.from = from; self.date = date
    }

    /// Pictures go to Photos; everything else is a doc.
    public var isImage: Bool { mime.lowercased().hasPrefix("image/") }

    public var attachment: Attachment { Attachment(id: id, filename: filename, mime: mime, size: size) }
}

public struct Conversation: Codable, Sendable, Equatable {
    public var id: String
    /// "person", "feed" or "list".
    public var kind: String
    /// Its Gmail threads, oldest first.
    public var threads: [ThreadSummary]
    public var people: [ConversationPerson]
    public var files: [SharedFile]
    public var links: [SharedLink]

    public init(id: String, kind: String = "person", threads: [ThreadSummary] = [], people: [ConversationPerson] = [],
                files: [SharedFile] = [], links: [SharedLink] = []) {
        self.id = id; self.kind = kind; self.threads = threads; self.people = people; self.files = files; self.links = links
    }

    public var docs: [SharedFile] { files.filter { !$0.isImage } }
    public var photos: [SharedFile] { files.filter(\.isImage) }
}

/// One entry of a conversation's Docs tab. Attachments come first; other sources
/// (files kept elsewhere and linked or shared with these people) add `.linked` entries.
public enum DocEntry: Equatable, Hashable, Identifiable, Sendable {
    case attachment(SharedFile)
    case linked(id: String, title: String, url: String, from: String, date: Int64)

    public var id: String {
        switch self {
        case let .attachment(f): "att:\(f.id)"
        case let .linked(id, _, _, _, _): "link:\(id)"
        }
    }

    public var date: Int64 {
        switch self {
        case let .attachment(f): f.date
        case let .linked(_, _, _, _, d): d
        }
    }

    /// Every source merged, newest first; a linked file already attached is shown once.
    public static func merged(attachments: [SharedFile], linked: [DocEntry]) -> [DocEntry] {
        let names = Set(attachments.map { $0.filename.lowercased() })
        let extra = linked.filter {
            if case let .linked(_, title, _, _, _) = $0 { !names.contains(title.lowercased()) } else { true }
        }
        return (attachments.map(DocEntry.attachment) + extra).sorted { $0.date > $1.date }
    }
}

/// One line of the chat: a marker where another Gmail thread starts, or a message.
public enum TimelineItem: Equatable, Identifiable, Sendable {
    /// `joined`: this thread was added to the conversation (it is not the first one),
    /// so the reader can take it out.
    case marker(threadId: String, subject: String, date: Int64, joined: Bool)
    case message(Message)

    public var id: String {
        switch self {
        case let .marker(t, _, _, _): "marker:\(t)"
        case let .message(m): m.id
        }
    }
}

public enum Timeline {
    /// Messages in date order, with a marker each time the Gmail thread changes. A lone
    /// thread gets no marker: the chat reads like a plain conversation.
    public static func items(_ messages: [Message], threads: [ThreadSummary]) -> [TimelineItem] {
        let sorted = messages.sorted { $0.date < $1.date }
        let distinct = Set(sorted.compactMap(\.threadId))
        guard distinct.count > 1 else { return sorted.map(TimelineItem.message) }
        let first = sorted.first?.threadId
        var out: [TimelineItem] = []
        var current: String?
        for m in sorted {
            if let t = m.threadId, t != current {
                let subject = threads.first { $0.id == t }?.subject ?? m.subject
                out.append(.marker(threadId: t, subject: subject, date: m.date, joined: t != first))
                current = t
            }
            out.append(.message(m))
        }
        return out
    }
}
