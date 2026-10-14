/// Conversations in the app: reloading the open one, taking a thread out, joining
/// selected rows, the reply target, and the sidebar's Docs list.
import AppKit
import Foundation
import MermozKit

extension AppModel {
    /// Opens a message's whole email in place, or folds it back into its chat bubble.
    func toggleMessage(_ id: String) {
        focusedMessageId = id
        if expandedMessages.contains(id) { expandedMessages.remove(id) } else { expandedMessages.insert(id) }
    }

    /// Fetches the open conversation again (after an AI rewrite, a split, new mail).
    func reloadDetail() async {
        guard let id = selectedThreadId, let client else { return }
        if let d = try? await client.thread(id, latest: detailWindow), selectedThreadId == id, d != detail { detail = d }
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
                refreshCounts()
            } catch { report(error) }
        }
    }

    /// Joins the selected rows into one conversation (⇧M, or the row menu).
    @discardableResult
    func mergeSelected() -> Task<Void, Never>? {
        let ids = targetIds
        guard ids.count > 1, let target = selectedThreadId ?? ids.last else {
            toast = "Select two conversations or more to join them (⇧ or ⌘ click)"
            return nil
        }
        return merge(ids.filter { $0 != target }, into: target)
    }

    /// Rows or cards dropped on a Board column, beside its cards: they take the column's
    /// value (status, priority, or its project). False when the column has none to give.
    func dropOnColumn(_ items: [String], group: ThreadGroup) -> Bool {
        dragIds = []
        defer { dropHandled(nil) }
        let ids = MergeDrop.ids(in: items)
        guard !ids.isEmpty else { return false }
        switch group.field {
        case .status: update(ids, status: group.key)
        case .priority: update(ids, priority: Int(group.key) ?? 0)
        case .project:
            if group.key.isEmpty { return false }
            update(ids, addProjects: [group.key])
        default: return false
        }
        return true
    }

    /// Tells a waiting test that a drop handler ran, with the merge it started.
    private func dropHandled(_ task: Task<Void, Never>?) {
        let observer = dropObserver
        dropObserver = nil
        observer?(task)
    }

    /// The daemon's merge calls: the client, or a stub in tests.
    var conversationCalls: (any ConversationBackend)? { conversationsStub ?? client }

    /// Whether the drag in progress would merge into `target`: anything but itself.
    func canMerge(into target: String) -> Bool { MergeDrop.merge(dragIds, onto: target) != nil }

    /// Rows or cards dropped onto another one: they join its conversation. Nil (the drop
    /// is refused) when nothing but the target itself was dropped.
    @discardableResult
    func dropMerge(_ items: [String], onto target: String) -> Task<Void, Never>? {
        dragIds = []
        let dropped = MergeDrop.ids(in: items)
        let task = MergeDrop.merge(dropped, onto: target) == nil ? nil : merge(dropped, into: target)
        dropHandled(task)
        return task
    }

    /// Joins `ids` into `target`'s conversation, then shows it: selected, in the pane, with
    /// "Merged into '…' · Undo"; ⌘Z puts the conversations back apart.
    @discardableResult
    func merge(_ ids: [String], into target: String) -> Task<Void, Never>? {
        guard let calls = conversationCalls, let all = MergeDrop.merge(ids, onto: target) else { return nil }
        let subject = summary(of: target)?.subject
        return Task {
            do {
                let r = try await calls.mergeConversations(all)
                undoStack.append(UndoEntry(label: "Merge", before: [], unmerge: r.undo))
                if undoStack.count > 100 { undoStack.removeFirst() }
                multiSelection = []
                await loadList()
                refreshCounts()
                // The conversation's row now carries one of its threads, maybe not the target.
                let shown = threads.first { r.members.contains($0.id) }?.id
                    ?? feed?.cards.first { r.members.contains($0.thread.id) }?.thread.id
                    ?? target
                if let card = feed?.cards.first(where: { $0.thread.id == shown }) { feedFocus = card.id }
                if selectedThreadId == shown { await reloadDetail() } else { open(shown) }
                let name = summary(of: shown)?.subject ?? subject ?? ""
                let text = "Merged into “\(name.isEmpty ? "(no subject)" : name)”"
                toast = text
                toastUndo = text
            } catch { report(error) }
        }
    }

    /// ⌘Z after a merge: each thread back in the conversation it was in.
    @discardableResult
    func unmerge(_ token: MergeUndo, label: String) -> Task<Void, Never>? {
        guard let calls = conversationCalls else { return nil }
        return Task {
            do {
                try await calls.unmergeConversations(token)
                toast = "Undid \(label)"
                await loadList()
                refreshCounts()
                await reloadDetail()
            } catch { report(error) }
        }
    }

    /// A thread as the List, the pane or the Feed holds it.
    func summary(of id: String) -> ThreadSummary? {
        threads.first { $0.id == id } ?? (detail?.thread.id == id ? detail?.thread : nil) ?? feedThread(id)
    }

    /// Files linked to the conversation from other sources than attachments: Google
    /// Drive files linked in its mail or shared with its people.
    func linkedDocs(for d: ThreadDetail) -> [DocEntry] { driveDocs(for: d) }

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
