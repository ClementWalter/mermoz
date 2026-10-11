/// Main-actor store: mirrors daemon state for the views and applies optimistic edits.
import AppKit
import Foundation
import MermozKit
import Observation
import os

private let log = Logger(subsystem: "ai.mermoz.app", category: "model")

enum SidebarSelection: Hashable {
    case status(String)
    case project(String)
    case account(String)
}

enum LayoutMode: String, CaseIterable {
    case list = "List"
    case board = "Board"
}

enum ConnectionState: Equatable {
    case connecting
    case connected
    case unreachable(String)
}

/// What the right pane is composing, if anything.
struct ComposeContext: Equatable, Identifiable {
    var id = UUID()
    var threadId: String?
    var accountId: String?
    var to: [String]
    var cc: [String]
    var subject: String
    var body: String = ""
    /// Existing draft being edited (Review → Edit).
    var draftId: String?
}

enum PaletteMode: Equatable {
    case search
    case status
    case priority
}

@MainActor
@Observable
final class AppModel {
    var connection: ConnectionState = .connecting
    var accounts: [Account] = []
    var statuses: [Status] = []
    var projects: [Project] = []
    var counts = Counts()
    var agentsMaySend = false
    var feedback: [FeedbackEntry] = []
    var syncProgress: [String: (synced: Int, total: Int)] = [:]

    var selection: SidebarSelection = .status("triage")
    var layout: LayoutMode = .list
    var threads: [ThreadSummary] = []
    var totalThreads = 0
    var selectedThreadId: String?
    var detail: ThreadDetail?
    var compose: ComposeContext?

    var reviewDrafts: [Draft] = []
    var selectedDraftId: String?

    var paletteMode: PaletteMode?
    var showShortcuts = false
    var toast: String?

    /// Addresses seen in opened threads; feeds the compose recipient check.
    var checker = RecipientChecker()
    private var seenAddresses: [Address] = []

    private var client: DaemonClient?
    private var events: Connection?
    private var refreshTask: Task<Void, Never>?

    var isReview: Bool { selection == .status("review") }

    var openStatuses: [Status] { statuses.filter { !closedStatusKeys.contains($0.key) } }

    func color(forStatus key: String) -> String {
        statuses.first { $0.key == key }?.color ?? Theme.statusColors[key] ?? "#5A7386"
    }

    func project(_ id: String?) -> Project? { id.flatMap { pid in projects.first { $0.id == pid } } }

    // MARK: lifecycle

    func start() async {
        connection = .connecting
        do {
            let c = try await DaemonLauncher.connect()
            client = DaemonClient(connection: c)
            try await openEventStream()
            connection = .connected
            await refreshAll()
        } catch {
            log.error("daemon unreachable: \(error.localizedDescription)")
            connection = .unreachable(error.localizedDescription)
        }
    }

    private func openEventStream() async throws {
        let conn = try Connection(path: DaemonPaths.socket, onEvent: { [weak self] e in
            Task { @MainActor in self?.handle(e) }
        }, onClose: { [weak self] in
            Task { @MainActor in self?.daemonWentAway() }
        })
        events = conn
        try await DaemonClient(connection: conn).subscribe()
    }

    private func daemonWentAway() {
        guard connection == .connected else { return }
        connection = .connecting
        client?.connection.close()
        client = nil
        // The daemon may be restarting; relaunch and reconnect transparently.
        Task { await start() }
    }

    func refreshAll() async {
        guard let client else { return }
        async let a = client.accounts()
        async let s = client.statuses()
        async let p = client.projects()
        async let st = client.settings()
        do {
            accounts = try await a
            statuses = try await s.sorted { $0.order < $1.order }
            projects = try await p
            agentsMaySend = try await st.agentsMaySend
        } catch {
            report(error)
        }
        await refreshCounts()
        await loadList()
    }

    func refreshCounts() async {
        guard let client else { return }
        do {
            let c = try await client.counts()
            if c != counts { counts = c }
            let r = try await client.drafts(state: "review")
            if r != reviewDrafts { reviewDrafts = r }
        } catch { report(error) }
    }

    // MARK: list

    func select(_ s: SidebarSelection) {
        selection = s
        selectedThreadId = nil
        detail = nil
        compose = nil
        selectedDraftId = nil
        Task { await loadList() }
    }

    func loadList() async {
        guard let client else { return }
        if isReview && layout == .list {
            do {
                reviewDrafts = try await client.drafts(state: "review")
                if selectedDraftId == nil { selectedDraftId = reviewDrafts.first?.id }
            } catch { report(error) }
            return
        }
        do {
            let page: ThreadPage
            let open = openStatuses.map(\.key)
            switch (selection, layout) {
            case let (.status(k), .list):
                page = try await client.threads(status: [k])
            case (.status, .board):
                page = try await client.threads(status: open)
            case let (.project(id), _):
                page = try await client.threads(status: open, projectId: id)
            case let (.account(id), _):
                page = try await client.threads(status: open, accountId: id)
            }
            // Unchanged pages must not re-render the list (sync streams change events).
            if threads != page.threads { threads = page.threads }
            if totalThreads != page.total { totalThreads = page.total }
            if let sel = selectedThreadId, !threads.contains(where: { $0.id == sel }) {
                selectedThreadId = nil
                detail = nil
            }
        } catch { report(error) }
    }

    /// Threads grouped for list mode: priority buckets in Linear order (urgent first, none last).
    var priorityGroups: [(priority: Int, threads: [ThreadSummary])] {
        let order = [1, 2, 3, 4, 0]
        let byP = Dictionary(grouping: threads, by: \.priority)
        return order.compactMap { p in byP[p].map { (p, $0) } }
    }

    /// Flat order matching what list mode displays, for j/k navigation.
    var displayOrder: [ThreadSummary] { priorityGroups.flatMap(\.threads) }

    // MARK: thread

    func open(_ id: String?) {
        selectedThreadId = id
        compose = nil
        guard let id, let client else { detail = nil; return }
        Task {
            do {
                let d = try await client.thread(id)
                guard selectedThreadId == id else { return }
                detail = d
                learnAddresses(from: d)
                if d.thread.unread {
                    try? await client.markRead(id)
                    setLocal(id) { $0.unread = false }
                }
            } catch { report(error) }
        }
    }

    /// Esc peels one layer: palette first, then the open thread or draft.
    func escape() {
        if showShortcuts {
            showShortcuts = false
        } else if paletteMode != nil {
            paletteMode = nil
        } else if selectedThreadId != nil {
            open(nil)
        } else if isReview, selectedDraftId != nil {
            selectedDraftId = nil
        }
    }

    func moveSelection(_ delta: Int) {
        let order = layout == .list ? displayOrder : threads
        guard !order.isEmpty else { return }
        let idx = order.firstIndex { $0.id == selectedThreadId } ?? (delta > 0 ? -1 : order.count)
        let next = max(0, min(order.count - 1, idx + delta))
        open(order[next].id)
    }

    private func learnAddresses(from d: ThreadDetail) {
        for m in d.messages { seenAddresses.append(m.from); seenAddresses += m.to; seenAddresses += m.cc }
        checker = RecipientChecker(addresses: seenAddresses)
    }

    private func setLocal(_ id: String, _ f: (inout ThreadSummary) -> Void) {
        if let i = threads.firstIndex(where: { $0.id == id }) { f(&threads[i]) }
        if detail?.thread.id == id { f(&detail!.thread) }
    }

    /// Optimistic update: change local state now, roll back if the daemon refuses.
    func update(_ ids: [String], status: String? = nil, priority: Int? = nil,
                project: Patch<String> = .keep, due: Patch<String> = .keep) {
        guard let client else { return }
        let before = threads
        let beforeDetail = detail
        for id in ids {
            setLocal(id) { t in
                if let status { t.status = status }
                if let priority { t.priority = priority }
                switch project { case let .set(v): t.projectId = v; case .clear: t.projectId = nil; case .keep: break }
                switch due { case let .set(v): t.due = v; case .clear: t.due = nil; case .keep: break }
            }
        }
        // A status change moves the thread out of a single-status list immediately.
        if status != nil, case let .status(k) = selection, layout == .list {
            threads.removeAll { ids.contains($0.id) && $0.status != k }
        }
        Task {
            do {
                try await client.updateThreads(ids: ids, status: status, priority: priority, project: project, due: due)
                await refreshCounts()
            } catch {
                threads = before
                detail = beforeDetail
                report(error)
            }
        }
    }

    func markDoneSelected() {
        guard let id = selectedThreadId else { return }
        let order = layout == .list ? displayOrder : threads
        let idx = order.firstIndex { $0.id == id }
        update([id], status: "done")
        // Keep the keyboard flow going: land on the next thread.
        if let idx {
            let rest = order.filter { $0.id != id }
            open(rest.isEmpty ? nil : rest[min(idx, rest.count - 1)].id)
        }
    }

    func createProject(_ name: String) async -> Project? {
        guard let client else { return nil }
        do {
            let p = try await client.createProject(name: name, color: nil)
            projects.append(p)
            return p
        } catch { report(error); return nil }
    }

    func openAttachment(_ a: Attachment) {
        guard let client else { return }
        Task {
            do {
                let f = try await client.attachment(a.id)
                NSWorkspace.shared.open(URL(fileURLWithPath: f.path))
            } catch { report(error) }
        }
    }

    func html(for messageId: String, full: Bool = false) async -> String? {
        try? await client?.html(messageId: messageId, full: full)
    }

    // MARK: compose

    func startReply() {
        guard let d = detail else { return }
        let last = d.messages.last
        let myAccount = accounts.first { $0.id == d.thread.accountId }?.email.lowercased()
        var to: [String] = []
        var cc: [String] = []
        if let last {
            if last.isFromMe {
                to = last.to.map(\.email)
                cc = last.cc.map(\.email)
            } else {
                to = [last.from.email]
                cc = (last.to + last.cc).map(\.email).filter { $0.lowercased() != myAccount && $0.lowercased() != last.from.email.lowercased() }
            }
        }
        let subj = d.thread.subject.lowercased().hasPrefix("re:") ? d.thread.subject : "Re: \(d.thread.subject)"
        compose = ComposeContext(threadId: d.thread.id, accountId: d.thread.accountId, to: to, cc: cc, subject: subj)
    }

    func startNew() {
        selectedThreadId = nil
        detail = nil
        compose = ComposeContext(threadId: nil, accountId: accounts.first?.id, to: [], cc: [], subject: "")
    }

    func editDraft(_ d: Draft) {
        compose = ComposeContext(threadId: d.threadId, accountId: d.accountId, to: d.to, cc: d.cc,
                                 subject: d.subject, body: d.body, draftId: d.id)
    }

    /// Creates (or updates) the draft and sends it; the app is a trusted caller so it sends directly.
    func send(_ c: ComposeContext, thenWaiting: Bool) async -> Bool {
        guard let client else { return false }
        do {
            let draft: Draft
            if let id = c.draftId {
                draft = try await client.updateDraft(id, to: c.to, cc: c.cc, subject: c.subject, body: c.body)
            } else {
                draft = try await client.createDraft(accountId: c.accountId, threadId: c.threadId, to: c.to,
                                                     cc: c.cc, subject: c.subject, body: c.body)
            }
            if c.draftId != nil, isReview {
                _ = try await client.approve(draft.id)
            } else {
                _ = try await client.sendDraft(draft.id)
            }
            if thenWaiting, let tid = c.threadId { update([tid], status: "waiting") }
            compose = nil
            toast = "Sent"
            if let tid = c.threadId, selectedThreadId == tid { open(tid) }
            await refreshCounts()
            return true
        } catch {
            report(error)
            return false
        }
    }

    // MARK: review

    var selectedDraft: Draft? { reviewDrafts.first { $0.id == selectedDraftId } }

    func approve(_ d: Draft) {
        guard let client else { return }
        Task {
            do {
                _ = try await client.approve(d.id)
                toast = "Sent"
                afterReview(d)
            } catch { report(error) }
        }
    }

    func reject(_ d: Draft) {
        guard let client else { return }
        Task {
            do {
                try await client.reject(d.id)
                afterReview(d)
            } catch { report(error) }
        }
    }

    private func afterReview(_ d: Draft) {
        reviewDrafts.removeAll { $0.id == d.id }
        selectedDraftId = reviewDrafts.first?.id
        Task { await refreshCounts() }
    }

    // MARK: settings

    func addGmail() {
        guard let client else { return }
        Task {
            do {
                let start = try await client.addGmail()
                if let url = URL(string: start.authUrl) { NSWorkspace.shared.open(url) }
            } catch { report(error) }
        }
    }

    func removeAccount(_ id: String) {
        guard let client else { return }
        Task {
            do {
                try await client.removeAccount(id)
                await refreshAll()
            } catch { report(error) }
        }
    }

    func setAgentsMaySend(_ v: Bool) {
        guard let client else { return }
        let before = agentsMaySend
        agentsMaySend = v
        Task {
            do { try await client.setAgentsMaySend(v) } catch {
                agentsMaySend = before
                report(error)
            }
        }
    }

    func loadFeedback() async {
        guard let client else { return }
        do { feedback = try await client.feedback() } catch { report(error) }
    }

    func search(_ q: String) async -> [ThreadSummary] {
        guard let client, !q.trimmingCharacters(in: .whitespaces).isEmpty else { return [] }
        return (try? await client.search(q, limit: 30)) ?? []
    }

    /// Jumps to a thread that may not be in the current list (palette results).
    func reveal(_ t: ThreadSummary) {
        if !threads.contains(where: { $0.id == t.id }) {
            selection = .status(t.status)
            layout = .list
            Task {
                await loadList()
                open(t.id)
            }
        } else {
            open(t.id)
        }
    }

    // MARK: events

    private func handle(_ e: DaemonEvent) {
        switch e {
        case let .threadsChanged(ids):
            scheduleRefresh()
            if let sel = selectedThreadId, ids.contains(sel), compose == nil {
                Task {
                    if let d = try? await client?.thread(sel), selectedThreadId == sel, d != detail { detail = d }
                }
            }
        case .accountsChanged:
            Task { await refreshAll() }
        case let .syncProgress(acc, synced, total):
            syncProgress[acc] = (synced, total)
        case .draftsChanged:
            Task { await refreshCounts(); if isReview { await loadList() } }
        case .unknown:
            break
        }
    }

    /// Coalesces bursts of change events (initial sync) into one refresh.
    private func scheduleRefresh() {
        refreshTask?.cancel()
        refreshTask = Task {
            // Initial sync emits a burst every few seconds; refresh at most once a second.
            try? await Task.sleep(for: .milliseconds(1000))
            guard !Task.isCancelled else { return }
            await refreshCounts()
            await loadList()
        }
    }

    func report(_ error: Error) {
        log.error("\(error.localizedDescription)")
        toast = (error as? RPCError)?.message ?? error.localizedDescription
    }

    // MARK: sync footer

    var syncLine: String {
        let syncing = accounts.compactMap { a -> (Int, Int)? in
            if let p = syncProgress[a.id], p.total > 0, p.synced < p.total { return (p.synced, p.total) }
            if a.sync.state == "initial" || a.sync.state == "syncing", a.sync.total > 0, a.sync.synced < a.sync.total {
                return (a.sync.synced, a.sync.total)
            }
            return nil
        }
        if syncing.isEmpty {
            if accounts.contains(where: { $0.sync.state == "error" }) { return "Can't reach your mail right now" }
            return "Synced · available offline"
        }
        let s = syncing.reduce(0) { $0 + $1.0 }, t = syncing.reduce(0) { $0 + $1.1 }
        // The import mirrors the whole mailbox (archive included), not just the inbox.
        return "Downloading your mail · \(s.formatted()) of \(t.formatted())"
    }
}
