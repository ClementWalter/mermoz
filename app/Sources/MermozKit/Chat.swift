/// "Chat about this": a thread's conversation with the user's AI, and the proposals the
/// AI attaches to an answer, decoded as mermozd sends them (docs/protocol.md).
import Foundation

public struct ChatAction: Codable, Sendable, Equatable, Hashable {
    public var kind: String
    public var label: String
    public var status: String?
    public var project: String?
    public var due: String?
    public var body: String?

    public init(kind: String, label: String, status: String? = nil, project: String? = nil, due: String? = nil, body: String? = nil) {
        self.kind = kind; self.label = label; self.status = status; self.project = project; self.due = due; self.body = body
    }

    /// What clicking the chip does; every case goes through an existing, undoable action.
    public enum Plan: Equatable, Sendable {
        /// Opens the reply composer with this text (never sends).
        case draftReply(String)
        case status(String)
        case addProject(String)
        case followUp(String)
        case none
    }

    public var plan: Plan {
        switch kind {
        case "draft_reply": body.map(Plan.draftReply) ?? .none
        case "status": status.map(Plan.status) ?? .none
        case "project": project.map(Plan.addProject) ?? .none
        case "follow_up": due.map(Plan.followUp) ?? .none
        default: .none
        }
    }

    public var symbol: String {
        switch kind {
        case "draft_reply": "arrowshape.turn.up.left"
        case "status": "circle.dashed"
        case "project": "folder"
        case "follow_up": "clock"
        default: "sparkle"
        }
    }
}

public struct ChatMessage: Codable, Sendable, Identifiable, Equatable {
    public var id: String
    public var threadId: String
    public var role: String
    public var text: String
    public var actions: [ChatAction]
    public var searched: Bool
    public var researchUnavailable: Bool
    public var created: Int64

    public var isAssistant: Bool { role == "assistant" }

    public init(id: String, threadId: String, role: String, text: String, actions: [ChatAction] = [],
                searched: Bool = false, researchUnavailable: Bool = false, created: Int64 = 0) {
        self.id = id; self.threadId = threadId; self.role = role; self.text = text; self.actions = actions
        self.searched = searched; self.researchUnavailable = researchUnavailable; self.created = created
    }
}

/// `chat.get`: the conversation so far, and whether this account's AI can search the web.
public struct ChatHistory: Codable, Sendable, Equatable {
    public var messages: [ChatMessage]
    public var researchAvailable: Bool
}
