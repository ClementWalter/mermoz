/// Conversations in the app: reloading the open one, taking a thread out, joining
/// selected rows, the reply target, and the sidebar's Docs list.
import AppKit
import Foundation
import MermozKit

extension AppModel {
    /// Open feeds and mailing lists, for the sidebar.
    var feedCount: Int? {
        let n = (counts.kind ?? [:]).values.reduce(0, +)
        return n > 0 ? n : nil
    }

    /// Fetches the open conversation again (after an AI rewrite, a split, new mail).
    func reloadDetail() async {
        guard let id = selectedThreadId, let client else { return }
        if let d = try? await client.thread(id), selectedThreadId == id, d != detail { detail = d }
    }

    /// The Gmail thread a reply goes to: the one holding the latest message.
    func replyTarget(_ d: ThreadDetail) -> (id: String, subject: String) {
        guard let t = d.messages.last?.threadId, t != d.thread.id,
              let thread = d.conversation?.threads.first(where: { $0.id == t })
        else { return (d.thread.id, d.thread.subject) }
        return (thread.id, thread.subject)
    }

    /// Takes one Gmail thread out of the open conversation ("Not the same conversation").
    func splitThread(_ threadId: String) {
        guard let client else { return }
        Task {
            do {
                try await client.splitConversation(threadId)
                toast = "Moved to its own conversation"
                await reloadDetail()
                await loadList()
                await refreshCounts()
            } catch { report(error) }
        }
    }

    /// Joins the selected rows into one conversation (⇧M, or the row menu).
    func mergeSelected() {
        let ids = targetIds
        guard ids.count > 1, let client else {
            toast = "Select two conversations or more to join them (⇧ or ⌘ click)"
            return
        }
        Task {
            do {
                try await client.mergeConversations(ids)
                toast = "Joined into one conversation"
                multiSelection = []
                await loadList()
                await refreshCounts()
                await reloadDetail()
            } catch { report(error) }
        }
    }

    /// Files linked to the conversation from other sources than attachments. The source
    /// plugs in here; none is connected yet.
    func linkedDocs(for _: ThreadDetail) -> [DocEntry] { [] }

    /// Files shared anywhere, newest first, filtered by name.
    func loadFiles() async {
        guard let client else { return }
        let q = fileQuery.trimmingCharacters(in: .whitespaces)
        do {
            let f = try await client.files(query: q.isEmpty ? nil : q)
            if f != files { files = f }
        } catch { report(error) }
    }

    /// Opens the conversation a shared file came from, beside the Docs list.
    func openFileConversation(_ f: SharedFile) {
        open(f.threadId)
    }

    func openLink(_ l: SharedLink) {
        if let url = URL(string: l.url) { NSWorkspace.shared.open(url) }
    }

    /// Opens a person from a conversation in the People directory.
    func showPerson(_ email: String) {
        select(.people)
        openContact(email)
    }
}
