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
    /// A Gmail label: (account id, label id).
    case label(String, String)
    /// The directory built from signatures.
    case people
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
    /// Pick projects for the selected thread (toggle, or create one).
    case project
}

@MainActor
@Observable
final class AppModel {
    var connection: ConnectionState = .connecting
    var accounts: [Account] = []
    var statuses: [Status] = []
    var projects: [Project] = []
    var labels: [MailLabel] = []
    var contacts: [Contact] = []
    var contactQuery = ""
    var selectedContact: ContactDetail?
    var providers: [Provider] = []
    var router = RouterState()
    /// What the AI filed a thread under when it was opened (reading confirms the
    /// suggestion); shown with an Undo in the reading pane.
    var appliedSuggestion: [String: Suggestion] = [:]
    var agentStates: [String: AgentState] = [:]
    var login = LoginState(running: false, error: nil)
    var counts = Counts()
    var agentsMaySend = false
    var feedback: [FeedbackEntry] = []
    var syncProgress: [String: (synced: Int, total: Int)] = [:]

    var selection: SidebarSelection = .status("unread")
    var layout: LayoutMode = .list
    /// Display options are kept per layout, like Linear's per-view settings.
    var listDisplay: DisplayOptions = AppModel.loadDisplay("list") ?? DisplayOptions(grouping: .priority)
    var boardDisplay: DisplayOptions = AppModel.loadDisplay("board") ?? DisplayOptions(grouping: .status)
    var triageResult: TriageResult?
    var peopleJob = JobState()
    /// Keys of AI actions in flight ("clean:<thread>", "triage:<status>").
    var aiBusy: Set<String> = []
    var threads: [ThreadSummary] = []
    var totalThreads = 0
    var selectedThreadId: String?
    /// Threads selected together with ⇧ or ⌘; empty means only `selectedThreadId`.
    var multiSelection: [String] = []
    /// Where a ⇧-range starts.
    private var anchorId: String?
    var detail: ThreadDetail?
    var compose: ComposeContext?

    var reviewDrafts: [Draft] = []
    var selectedDraftId: String?

    /// Thread states before each change, newest last; ⌘Z restores the last one.
    var undoStack: [UndoEntry] = []

    struct UndoEntry {
        var label: String
        var before: [ThreadSummary]
    }

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
            client = DaemonClient(connection: try await connectRequests())
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

    /// The connection requests go through; if it drops, a fresh one replaces it.
    private func connectRequests() async throws -> Connection {
        try await DaemonLauncher.connect(onClose: { [weak self] in
            Task { @MainActor in self?.requestsConnectionLost() }
        })
    }

    /// Without this, every later request would fail with "connection closed" while the
    /// event stream (a separate connection) kept the app looking alive.
    private func requestsConnectionLost() {
        guard connection == .connected else { return }
        Task {
            guard let c = try? await connectRequests() else { return daemonWentAway() }
            client = DaemonClient(connection: c)
            await refreshAll()
            if let id = selectedThreadId { open(id, record: false) }
        }
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
        async let l = client.labels()
        async let st = client.settings()
        do {
            accounts = try await a
            statuses = try await s.sorted { $0.order < $1.order }
            projects = try await p
            labels = try await l
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
        recordHistory()
        selection = s
        selectedThreadId = nil
        detail = nil
        compose = nil
        selectedDraftId = nil
        Task { await loadList() }
    }

    func loadList() async {
        guard let client else { return }
        if selection == .people {
            await loadContacts()
            return
        }
        if isReview && layout == .list {
            do {
                reviewDrafts = try await client.drafts(state: "review")
                if selectedDraftId == nil { selectedDraftId = reviewDrafts.first?.id }
            } catch { report(error) }
            return
        }
        let d = effectiveDisplay
        let order = d.ordering.rawValue
        let open = openStatuses.map(\.key)
        var project: String?
        var accountId: String?
        var labelId: String?
        var statuses: [String]? = open
        switch selection {
        case let .status(k): if layout == .list { statuses = [k] }
        // A project is a Gmail label: it shows everything filed there, archived mail included.
        case let .project(id): project = id; statuses = nil
        case let .account(id): accountId = id
        case let .label(acc, id): accountId = acc; labelId = id; statuses = nil
        case .people: return
        }
        do {
            var fetched: [ThreadSummary] = []
            var total = 0
            if layout == .board, d.grouping == .status, let sts = statuses, labelId == nil {
                // One page per column, so every status shows its own newest threads and real size.
                try await withThrowingTaskGroup(of: (Int, ThreadPage).self) { group in
                    for (i, k) in sts.enumerated() {
                        group.addTask { [client, project, accountId, order] in
                            (i, try await client.threads(status: [k], project: project, accountId: accountId, order: order, limit: 150))
                        }
                    }
                    var pages: [(Int, ThreadPage)] = []
                    for try await p in group { pages.append(p) }
                    for (_, p) in pages.sorted(by: { $0.0 < $1.0 }) {
                        fetched += p.threads
                        total += p.total
                    }
                }
            } else {
                let page = try await client.threads(status: statuses, project: project, accountId: accountId, labelId: labelId,
                                                    order: order, limit: layout == .board ? 600 : 300)
                fetched = page.threads
                total = page.total
            }
            // The thread being read stays in place even once it no longer matches the view
            // (reading moves it out of Unread), so j/k continue from where it was.
            if let sel = selectedThreadId, !fetched.contains(where: { $0.id == sel }),
               let i = threads.firstIndex(where: { $0.id == sel }) {
                var kept = threads[i]
                if let d = detail?.thread, d.id == sel { kept = d }
                fetched.insert(kept, at: min(i, fetched.count))
            }
            // Unchanged pages must not re-render the list (sync streams change events).
            if threads != fetched { threads = fetched }
            if totalThreads != total { totalThreads = total }
        } catch { report(error) }
    }

    // MARK: history (⌘← / ⌘→)

    struct NavState: Equatable, Sendable {
        var selection: SidebarSelection
        var threadId: String?
    }

    private var history = NavHistory<NavState>()
    private var restoring = false

    var canGoBack: Bool { history.canGoBack }
    var canGoForward: Bool { history.canGoForward }

    private var here: NavState { NavState(selection: selection, threadId: selectedThreadId) }

    private func recordHistory() {
        guard !restoring else { return }
        history.record(leaving: here)
    }

    func goBack() {
        if let prev = history.goBack(from: here) { restore(prev) }
    }

    func goForward() {
        if let next = history.goForward(from: here) { restore(next) }
    }

    private func restore(_ state: NavState) {
        restoring = true
        compose = nil
        let changedView = state.selection != selection
        selection = state.selection
        Task {
            if changedView { await loadList() }
            open(state.threadId, record: false)
            restoring = false
        }
    }

    // MARK: display

    var display: DisplayOptions {
        get { layout == .list ? listDisplay : boardDisplay }
        set {
            if layout == .list { listDisplay = newValue } else { boardDisplay = newValue }
            AppModel.saveDisplay(newValue, layout == .list ? "list" : "board")
            Task { await loadList() }
        }
    }

    /// Labels and projects read chronologically unless the user picked another grouping.
    var effectiveDisplay: DisplayOptions {
        let mailbox: Bool = switch selection {
        case .label, .project: true
        default: false
        }
        if mailbox, layout == .list, listDisplay.grouping == .priority {
            return DisplayOptions(grouping: .none, ordering: listDisplay.ordering)
        }
        return display
    }

    var groups: [ThreadGroup] {
        let d = effectiveDisplay
        return Grouper.group(threads, by: d.grouping, then: d.subGrouping) { [statuses, projects, accounts] field in
            switch field {
            case .status: statuses.map(\.key)
            case .priority: ["1", "2", "3", "4", "0"]
            case .project: projects.map(\.id)
            case .account: accounts.map(\.id)
            case .none: []
            }
        }
    }

    /// Flat order matching what's on screen, for keyboard navigation; a thread in two
    /// project groups is visited once.
    var displayOrder: [ThreadSummary] {
        var seen = Set<String>()
        return groups.flatMap(\.all).filter { seen.insert($0.id).inserted }
    }

    /// Real size of a group when the server knows it (all-open views), else what's loaded.
    func groupTotal(_ g: ThreadGroup) -> Int {
        let allOpen: Bool = { if case .status = selection { return layout == .board } else { return false } }()
        guard allOpen else { return g.all.count }
        switch g.field {
        case .status: return counts.status[g.key] ?? g.all.count
        case .priority: return counts.priority?[g.key] ?? g.all.count
        case .project: return counts.project[g.key] ?? g.all.count
        case .account: return counts.account[g.key] ?? g.all.count
        case .none: return g.all.count
        }
    }

    /// Arrow keys on the board: ←/→ change column (keeping the row), ↑/↓ move within it.
    func moveBoard(dx: Int, dy: Int, extend: Bool = false) {
        let cols = groups.map(\.all).filter { !$0.isEmpty }
        guard !cols.isEmpty else { return }
        guard let sel = selectedThreadId,
              let ci = cols.firstIndex(where: { $0.contains { $0.id == sel } }),
              let ri = cols[ci].firstIndex(where: { $0.id == sel }) else {
            open(cols[0][0].id, record: false)
            return
        }
        let nc = max(0, min(cols.count - 1, ci + dx))
        let nr = max(0, min(cols[nc].count - 1, dx != 0 ? ri : ri + dy))
        let next = cols[nc][nr].id
        if extend, let anchor = anchorId ?? selectedThreadId {
            multiSelection = SelectionRange.between(orderIds, anchor: anchor, target: next)
            open(next, record: false, keepSelection: true)
        } else {
            open(next, record: false)
        }
    }

    private static func loadDisplay(_ key: String) -> DisplayOptions? {
        guard let d = UserDefaults.standard.data(forKey: "display.\(key)") else { return nil }
        return try? JSONDecoder().decode(DisplayOptions.self, from: d)
    }

    private static func saveDisplay(_ v: DisplayOptions, _ key: String) {
        if let d = try? JSONEncoder().encode(v) { UserDefaults.standard.set(d, forKey: "display.\(key)") }
    }

    // MARK: AI actions

    /// Lets the AI merge duplicates and tidy names across the whole directory.
    func aiCleanPeople() {
        guard let client else { return }
        Task {
            do {
                let start = try await client.peopleClean()
                peopleJob = JobState(running: true, done: 0, total: start.total)
            } catch { report(error) }
        }
    }

    /// Re-cleans every message of a thread now (the ✦ button in a thread).
    func aiClean(_ threadId: String) {
        guard let client else { return }
        aiBusy.insert("clean:\(threadId)")
        Task {
            defer { aiBusy.remove("clean:\(threadId)") }
            do {
                let r = try await client.aiClean(thread: threadId)
                toast = "Cleaned \(r.cleaned) message\(r.cleaned == 1 ? "" : "s")"
                if selectedThreadId == threadId, let d = try? await client.thread(threadId) { detail = d }
            } catch { report(error) }
        }
    }

    /// Lets the AI sort the newest threads of a status column (the ✦ button on a column).
    func aiTriage(_ status: String) {
        guard let client else { return }
        aiBusy.insert("triage:\(status)")
        Task {
            defer { aiBusy.remove("triage:\(status)") }
            do {
                let r = try await client.aiTriage(status: status)
                triageResult = r
                await refreshCounts()
                await loadList()
            } catch { report(error) }
        }
    }

    /// Puts every thread the AI moved back where it was.
    func undoTriage(_ r: TriageResult) {
        for (from, moves) in Dictionary(grouping: r.moves, by: \.from) where !moves.isEmpty {
            update(moves.map(\.number), status: from)
        }
        triageResult = nil
    }

    // MARK: thread

    /// `record: false` for keyboard stepping (j/k, arrows): it updates the current history
    /// entry instead of stacking one per thread.
    func open(_ id: String?, record: Bool = true, keepSelection: Bool = false) {
        if record, id != selectedThreadId { recordHistory() }
        if !keepSelection {
            multiSelection = []
            anchorId = id
        }
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
                    // Reading an Unread thread applies the AI's suggestion; remember it for Undo.
                    if d.thread.status == "unread", let sg = d.thread.suggestion, !sg.isEmpty {
                        appliedSuggestion[id] = sg
                    }
                    try? await client.markRead(id)
                    if let fresh = try? await client.thread(id), selectedThreadId == id {
                        detail = fresh
                        setLocal(id) { $0 = fresh.thread }
                    } else {
                        setLocal(id) { $0.unread = false }
                    }
                    await refreshCounts()
                }
            } catch { report(error) }
        }
    }

    /// Accepts the AI's suggestions as they stand (the threads count as read).
    func acceptSuggestions(_ ids: [String]) {
        guard let client, !ids.isEmpty else { return }
        recordUndo(ids.count == 1 ? "Accept Suggestion" : "Accept \(ids.count) Suggestions", ids)
        Task {
            do {
                try await client.acceptSuggestions(ids)
                toast = ids.count == 1 ? "Done as suggested" : "Accepted \(ids.count) suggestions"
                await refreshCounts()
                await loadList()
            } catch { report(error) }
        }
    }

    func acceptSelected() {
        let ids = targetIds.filter { id in threads.first { $0.id == id }?.suggestion != nil }
        acceptSuggestions(ids)
    }

    // MARK: multi-selection

    /// The threads an action applies to: the ⇧/⌘ selection, else the open thread.
    var targetIds: [String] {
        multiSelection.isEmpty ? (selectedThreadId.map { [$0] } ?? []) : multiSelection
    }

    func isSelected(_ id: String) -> Bool { id == selectedThreadId || multiSelection.contains(id) }

    /// Ids in on-screen order, a thread in two project groups counted once.
    private var orderIds: [String] { (layout == .list ? displayOrder : threads).map(\.id) }

    /// Click with Mail/Finder modifiers: ⇧ selects a range, ⌘ adds or removes one.
    func click(_ id: String, shift: Bool, command: Bool) {
        if shift, let anchor = anchorId ?? selectedThreadId {
            multiSelection = SelectionRange.between(orderIds, anchor: anchor, target: id)
            open(id, record: false, keepSelection: true)
        } else if command {
            var ids = targetIds
            if let i = ids.firstIndex(of: id) { ids.remove(at: i) } else { ids.append(id) }
            multiSelection = ids.count > 1 ? ids : []
            anchorId = id
            open(ids.contains(id) ? id : ids.last, record: false, keepSelection: true)
        } else {
            open(id)
        }
    }

    /// What a drag carries: every selected id when dragging one of them.
    func dragPayload(_ id: String) -> String {
        multiSelection.count > 1 && multiSelection.contains(id) ? multiSelection.joined(separator: "\n") : id
    }

    /// Puts back what reading applied: To do, without the projects the AI added.
    func undoApplied(_ id: String) {
        guard let sg = appliedSuggestion.removeValue(forKey: id) else { return }
        update([id], status: "todo", removeProjects: sg.projects)
    }

    /// Esc peels one layer: palette first, then the open thread or draft.
    func escape() {
        if showShortcuts {
            showShortcuts = false
        } else if paletteMode != nil {
            paletteMode = nil
        } else if !multiSelection.isEmpty {
            multiSelection = []
            anchorId = selectedThreadId
        } else if selectedThreadId != nil {
            open(nil)
        } else if isReview, selectedDraftId != nil {
            selectedDraftId = nil
        }
    }

    /// j/k and arrows; with ⇧ the selection grows from the anchor.
    func moveSelection(_ delta: Int, extend: Bool = false) {
        let order = layout == .list ? displayOrder : threads
        guard !order.isEmpty else { return }
        let idx = order.firstIndex { $0.id == selectedThreadId } ?? (delta > 0 ? -1 : order.count)
        let next = order[max(0, min(order.count - 1, idx + delta))].id
        if extend, let anchor = anchorId ?? selectedThreadId {
            multiSelection = SelectionRange.between(orderIds, anchor: anchor, target: next)
            open(next, record: false, keepSelection: true)
        } else {
            open(next, record: false)
        }
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
                addProjects: [String] = [], removeProjects: [String] = [], due: Patch<String> = .keep) {
        guard let client else { return }
        let label: String = if let status {
            status == "deleted" ? "Delete" : "Move to \(statuses.first { $0.key == status }?.name ?? status)"
        } else if priority != nil { "Priority" } else if !addProjects.isEmpty || !removeProjects.isEmpty { "Project" } else { "Follow-up" }
        recordUndo(label, ids)
        let before = threads
        let beforeDetail = detail
        for id in ids {
            setLocal(id) { t in
                if let status { t.status = status }
                if let priority { t.priority = priority }
                t.projects = Array(Set(t.projects).union(addProjects).subtracting(removeProjects)).sorted()
                switch due { case let .set(v): t.due = v; case .clear: t.due = nil; case .keep: break }
            }
        }
        // A status change moves the thread out of a single-status list immediately.
        if status != nil, case let .status(k) = selection, layout == .list {
            threads.removeAll { ids.contains($0.id) && $0.status != k }
        }
        Task {
            do {
                try await client.updateThreads(ids: ids, status: status, priority: priority,
                                               addProjects: addProjects, removeProjects: removeProjects, due: due)
                await refreshCounts()
            } catch {
                threads = before
                detail = beforeDetail
                report(error)
            }
        }
    }

    func markDoneSelected() { moveSelected(to: "done") }

    /// Back to Unread, like Gmail's "mark unread": the thread waits for a fresh look.
    func markUnreadSelected() {
        let ids = targetIds
        guard !ids.isEmpty, let client else { return }
        recordUndo("Mark Unread", ids)
        for id in ids { setLocal(id) { $0.unread = true; $0.status = "unread" } }
        Task {
            do {
                for id in ids { try await client.markRead(id, read: false) }
                await refreshCounts()
            } catch { report(error) }
        }
    }

    /// Moves the selected thread to a status and lands on the next one.
    func moveSelected(to status: String) {
        let ids = targetIds
        guard !ids.isEmpty else { return }
        let order = layout == .list ? displayOrder : threads
        let idx = order.firstIndex { ids.contains($0.id) }
        update(ids, status: status)
        // Keep the keyboard flow going: land on the thread after the moved ones.
        if let idx {
            let rest = order.filter { !ids.contains($0.id) }
            open(rest.isEmpty ? nil : rest[min(idx, rest.count - 1)].id, record: false)
        }
    }

    // MARK: undo (⌘Z)

    private func recordUndo(_ label: String, _ ids: [String]) {
        let before = ids.compactMap { id in threads.first { $0.id == id } ?? (detail?.thread.id == id ? detail?.thread : nil) }
        guard !before.isEmpty else { return }
        undoStack.append(UndoEntry(label: label, before: before))
        if undoStack.count > 100 { undoStack.removeFirst() }
    }

    var undoLabel: String? { undoStack.last?.label }

    /// Puts the threads of the last change back as they were: read state, status,
    /// priority, projects and follow-up date.
    func undo() {
        guard let entry = undoStack.popLast(), let client else { toast = "Nothing to undo"; return }
        Task {
            do {
                for t in entry.before {
                    let plan = RestorePlan.between(before: t, now: try await client.thread(t.id).thread)
                    // Read state first: marking unread moves the thread to Unread, the
                    // status update below then restores the exact status.
                    if let read = plan.markRead { try await client.markRead(t.id, read: read) }
                    let due: Patch<String> = switch plan.due {
                    case .none: .keep
                    case .some(.none): .clear
                    case let .some(.some(d)): .set(d)
                    }
                    try await client.updateThreads(ids: [t.id], status: plan.status, priority: plan.priority,
                                                   addProjects: plan.addProjects, removeProjects: plan.removeProjects, due: due)
                }
                toast = "Undid \(entry.label)"
                await refreshCounts()
                await loadList()
                if let id = selectedThreadId, entry.before.contains(where: { $0.id == id }) { open(id, record: false) }
            } catch { report(error) }
        }
    }

    // MARK: statuses

    /// Drag-and-drop in the sidebar: puts `key` where `target` is.
    func moveStatus(_ key: String, before target: String) {
        guard key != target, let client,
              let from = statuses.firstIndex(where: { $0.key == key }),
              let to = statuses.firstIndex(where: { $0.key == target }) else { return }
        let before = statuses
        var next = statuses
        let s = next.remove(at: from)
        next.insert(s, at: to)
        statuses = next
        Task {
            do { statuses = try await client.reorderStatuses(next.map(\.key)).sorted { $0.order < $1.order } } catch {
                statuses = before
                report(error)
            }
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

    func renameProject(_ id: String, to name: String) {
        guard let client, !name.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        Task {
            do {
                let p = try await client.updateProject(id, name: name.trimmingCharacters(in: .whitespaces))
                if let i = projects.firstIndex(where: { $0.id == id }) { projects[i] = p }
            } catch { report(error) }
        }
    }

    func recolorProject(_ id: String, to color: String) {
        guard let client else { return }
        Task {
            do {
                let p = try await client.updateProject(id, color: color)
                if let i = projects.firstIndex(where: { $0.id == id }) { projects[i] = p }
            } catch { report(error) }
        }
    }

    func deleteProject(_ id: String) {
        guard let client else { return }
        Task {
            do {
                try await client.deleteProject(id)
                projects.removeAll { $0.id == id }
                if selection == .project(id) { select(.status("unread")) } else { await loadList() }
                await refreshCounts()
            } catch { report(error) }
        }
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

    // MARK: directory

    func loadContacts() async {
        guard let client else { return }
        let q = contactQuery.trimmingCharacters(in: .whitespaces)
        if let c = try? await client.contacts(query: q.isEmpty ? nil : q), c != contacts { contacts = c }
    }

    func openContact(_ email: String?) {
        guard let email, let client else { selectedContact = nil; return }
        Task {
            do { selectedContact = try await client.contact(email) } catch { report(error) }
        }
    }

    // MARK: built-in agent

    func loadAgent() async {
        guard let client else { return }
        do {
            providers = try await client.providers()
            var states: [String: AgentState] = [:]
            for a in accounts { states[a.id] = try await client.agentState(account: a.id) }
            agentStates = states
        } catch { report(error) }
    }

    func saveProvider(id: String?, name: String, kind: String, baseUrl: String?, model: String, apiKey: String?) async -> Bool {
        guard let client else { return false }
        do {
            _ = try await client.saveProvider(id: id, name: name, kind: kind, baseUrl: baseUrl, model: model, apiKey: apiKey)
            await loadAgent()
            return true
        } catch { report(error); return false }
    }

    var providerModels: [String: [ProviderModel]] = [:]

    func signInWithChatGPT() {
        guard let client else { return }
        login = LoginState(running: true, error: nil)
        Task {
            do {
                let start = try await client.loginChatGPT()
                if let url = URL(string: start.authUrl) { NSWorkspace.shared.open(url) }
            } catch {
                login = LoginState(running: false, error: (error as? RPCError)?.message ?? error.localizedDescription)
            }
        }
    }

    func loadModels(for p: Provider) async {
        guard let client, p.kind == "chatgpt" else { return }
        if let m = try? await client.providerModels(p.id) { providerModels[p.id] = m }
    }

    func signInWithAnthropic() {
        guard let client else { return }
        login = LoginState(running: true, error: nil)
        Task {
            do { try await client.loginAnthropic() } catch {
                login = LoginState(running: false, error: (error as? RPCError)?.message ?? error.localizedDescription)
            }
        }
    }

    func deleteProvider(_ id: String) {
        guard let client else { return }
        Task {
            do { try await client.deleteProvider(id); await loadAgent() } catch { report(error) }
        }
    }

    func testProvider(_ id: String) async -> ProviderTest? {
        guard let client else { return nil }
        do { return try await client.testProvider(id) } catch { report(error); return nil }
    }

    func setAgent(account: String, providerIds: [String]? = nil, digestEnabled: Bool? = nil) {
        guard let client else { return }
        Task {
            do {
                try await client.setAgent(account: account, providerIds: providerIds, digestEnabled: digestEnabled)
                agentStates[account] = try await client.agentState(account: account)
            } catch { report(error) }
        }
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

    // MARK: instant sorting

    func loadRouter() async {
        guard let client else { return }
        if let r = try? await client.router() { router = r }
    }

    func setRouterKey(_ key: String?) async -> Bool {
        guard let client else { return false }
        do {
            router = try await client.setRouterKey(key)
            return true
        } catch { report(error); return false }
    }

    func testRouter(key: String?) async -> RouterTest? {
        guard let client else { return nil }
        do { return try await client.testRouter(key: key) } catch { report(error); return nil }
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
        case .providersChanged:
            Task {
                if let l = try? await client?.loginStatus() { login = l }
                await loadAgent()
            }
        case .contactsChanged:
            if selection == .people { Task { await loadContacts() } }
            if peopleJob.running { Task { if let j = try? await client?.peopleStatus() { peopleJob = j } } }
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
            // New labels appear as the import reaches older mail.
            if let l = try? await client?.labels(), l != labels { labels = l }
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
