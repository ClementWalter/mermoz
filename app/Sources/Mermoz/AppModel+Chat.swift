/// "Chat about this": opening the panel, sending questions, streaming answers and applying
/// the AI's proposals through the same undoable actions as the rest of the app.
import Foundation
import MermozKit

extension AppModel {
    /// Opens or closes the chat beside the open thread (t, ⌘K, or the toolbar button).
    func toggleChat() {
        if chatOpen { chatOpen = false } else { openChat() }
    }

    func openChat() {
        guard let id = selectedThreadId else { toast = "Open a conversation first"; return }
        // ⌘K can run on a selected thread that isn't the one on screen yet.
        if detail?.thread.id != id { open(id, record: false, keepSelection: true) }
        chatOpen = true
        loadChat(id)
    }

    /// Loads a thread's earlier conversation once; later updates come from sending.
    func loadChat(_ id: String) {
        guard chats[id] == nil, let client else { return }
        Task {
            do {
                let h = try await client.chat(thread: id)
                chats[id] = h.messages
                chatResearchAvailable[id] = h.researchAvailable
            } catch { chatErrors[id] = message(of: error) }
        }
    }

    func sendChat(_ text: String, thread id: String) {
        let q = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty, chatStreaming[id] == nil, let client else { return }
        // The question shows at once; the stored copy replaces it when the answer lands.
        chats[id, default: []].append(ChatMessage(id: "local-\(UUID().uuidString)", threadId: id, role: "user", text: q))
        chatErrors[id] = nil
        chatStreaming[id] = ""
        let research = chatResearch
        Task {
            defer { chatStreaming[id] = nil }
            do {
                let answer = try await client.sendChat(thread: id, text: q, research: research)
                if let h = try? await client.chat(thread: id) {
                    chats[id] = h.messages
                } else {
                    chats[id, default: []].append(answer)
                }
            } catch { chatErrors[id] = message(of: error) }
        }
    }

    /// "Start over": forgets this thread's conversation.
    func clearChat(_ id: String) {
        guard let client else { return }
        Task {
            do {
                try await client.clearChat(thread: id)
                chats[id] = []
                chatErrors[id] = nil
            } catch { report(error) }
        }
    }

    /// Runs one proposal on the thread. A reply opens in the composer and is never sent.
    func apply(_ action: ChatAction, key: String, thread id: String) {
        switch action.plan {
        case let .draftReply(body):
            guard detail?.thread.id == id else { return }
            startReply()
            compose?.body = body
        case let .status(s):
            update([id], status: s)
        case let .addProject(p):
            update([id], addProjects: [p])
        case let .followUp(d):
            update([id], due: .set(d))
        case .none:
            return
        }
        chatApplied.insert(key)
        if action.kind != "draft_reply" { toast = "\(action.label) · ⌘Z to undo" }
    }

    private func message(of error: Error) -> String {
        (error as? RPCError)?.message ?? error.localizedDescription
    }
}
