/// Main-actor store: mirrors daemon state for the views and applies optimistic edits.
import AppKit
import Foundation
import MermozKit
import Observation
import os
import UniformTypeIdentifiers

private let log = Logger(subsystem: "ai.mermoz.app", category: "model")

enum SidebarSelection: Hashable {
    case status(String)
    case project(String)
    case account(String)
    /// A Gmail label: (account id, label id).
    case label(String, String)
    /// The directory built from signatures.
    case people
    /// Files shared in every conversation.
    case docs
}

/// Tabs of the reading pane, about the open conversation.
enum ConversationTab: String, CaseIterable {
    case messages = "Messages"
    case people = "People"
    case docs = "Docs"
    case links = "Links"
    case photos = "Photos"
}

enum LayoutMode: String, CaseIterable {
    case list = "List"
    case board = "Board"
    /// Everything open as a stream of cards, one thing to do each (FeedView).
    case feed = "Feed"

    /// The layout's icon in the compact switch.
    var symbol: String {
        switch self {
        case .list: "list.bullet"
        case .board: "rectangle.split.3x1"
        case .feed: "rectangle.stack"
        }
    }
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
    var attachments = AttachmentList()
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
    /// The last Google mail sign-in, with Google's refusal explained.
    var gmailSignIn = GmailSignIn()
    /// The Settings pane shown; a sign-in problem can send the user to Developer.
    var settingsPane: SettingsPane = .agents
    /// One observable cell per badge, so a moved count redraws its badge alone.
    @ObservationIgnored let countCells = CountCells()
    /// Count refreshes merge: one in flight, asks within 100 ms folded into one fetch.
    @ObservationIgnored lazy var countsRefresh = RefreshCoalescer { [weak self] in await self?.fetchCounts() }
    var agentsMaySend = false
    var feedback: [FeedbackEntry] = []
    var syncProgress: [String: (synced: Int, total: Int)] = [:]

    var selection: SidebarSelection = .status("unread")
    /// List, Board or Feed; the last one chosen comes back at launch.
    var layout: LayoutMode = LayoutMode(rawValue: UserDefaults.standard.string(forKey: "layout.mode") ?? "") ?? .list {
        didSet { UserDefaults.standard.set(layout.rawValue, forKey: "layout.mode") }
    }
    /// Display options are kept per layout, like Linear's per-view settings.
    var listDisplay: DisplayOptions = AppModel.loadDisplay("list") ?? DisplayOptions(grouping: .priority)
    var boardDisplay: DisplayOptions = AppModel.loadDisplay("board") ?? DisplayOptions(grouping: .status)
    var peopleJob = JobState()
    /// Original / Optimized, chosen per message and remembered across launches.
    var readingModes: ReadingModes = AppModel.loadReadingModes() {
        didSet { if let d = try? JSONEncoder().encode(readingModes) { UserDefaults.standard.set(d, forKey: "reading.modes") } }
    }
    /// What suggestions learned from the user's answers (Settings › Developer).
    var learned: [LearnedItem] = []
    /// Keys of AI actions in flight ("clean:<message>", "optimize:<message>").
    var aiBusy: Set<String> = []
    var threads: [ThreadSummary] = []
    var totalThreads = 0
    var selectedThreadId: String? {
        didSet { if selectedThreadId != oldValue { selectionMoved() } }
    }
    /// Threads selected together with ⇧ or ⌘; empty means only `selectedThreadId`.
    var multiSelection: [String] = [] {
        didSet { if multiSelection != oldValue { selectionMoved() } }
    }
    /// Whether a thread is open; menus read this, not the id, so j/k leaves them alone.
    private(set) var hasSelection = false
    /// One flag per row, so a j/k step redraws the two rows it moves between.
    @ObservationIgnored let selectionCells = SelectionCells()
    /// Where a ⇧-range starts.
    private var anchorId: String?
    var detail: ThreadDetail?
    /// Conversations fetched lately (and the Feed's neighbours, fetched ahead), so the
    /// pane shows a card's mail the moment it is focused.
    var detailCache = DetailCache()
    var prefetchTask: Task<Void, Never>?
    var compose: ComposeContext?
    /// Replies being written in the chat's inline composer, by conversation (the open
    /// thread's id); each conversation keeps its own until sent or expanded.
    var inlineReplies: [String: ComposeContext] = [:]
    /// Bumped to put the keyboard in the inline composer (r).
    var replyFocusRequest = 0
    /// The inline composer has the keyboard: Esc is its own (leaves it, draft kept).
    var replyFocused = false
    /// Sends a composed message; tests put a stub here.
    @ObservationIgnored var sendOverride: ((ComposeContext) async -> Bool)?
    /// The reading pane's tab; back to Messages when another conversation opens.
    var conversationTab: ConversationTab = .messages
    /// Messages opened as the whole email; the rest read as chat bubbles.
    var expandedMessages: Set<String> = []
    /// Long bubbles whose "Show more" was pressed.
    var shownInFull: Set<String> = []
    /// The open chat shows its earlier messages too, not only the latest ones; they are
    /// fetched then, so a long conversation opens on its latest messages only.
    var chatShowsAll = false {
        didSet {
            if chatShowsAll, !oldValue, (detail?.earlier ?? 0) > 0 { Task { await reloadDetail() } }
        }
    }
    /// How many messages the pane fetches: the chat's opening screen, or all once shown.
    var detailWindow: Int? { chatShowsAll ? nil : ChatStream.opening }
    /// The message ↑/↓ moved to, which Enter opens.
    var focusedMessageId: String?
    /// Files shared anywhere (sidebar › Docs) and the filter typed above them.
    var files: [SharedFile] = []
    var fileQuery = ""

    var reviewDrafts: [Draft] = []
    var selectedDraftId: String?

    /// Thread states before each change, newest last; ⌘Z restores the last one.
    var undoStack: [UndoEntry] = []

    struct UndoEntry {
        var label: String
        var before: [ThreadSummary]
        /// Threads whose suggestions were turned down; undo makes them pending again.
        var rejected: [String] = []
        /// An answer to a card's suggestion: undo takes it back and shows the chip again.
        var answerUndo: SuggestionUndo?
        var answered: ThreadSuggestion?
        /// The answered thread's latest mail, which the chip was made for.
        var answeredDate: Int64 = 0
        /// A Later: undone by bringing these threads back into the feed.
        var unsnooze: [String] = []
        /// An "Ask Mermoz to do" run: undone as a whole by the daemon.
        var doOperation: String?
        /// A manual merge: undone by putting each thread back in its conversation.
        var unmerge: MergeUndo?
    }

    var paletteMode: PaletteMode?
    var showShortcuts = false
    var toast: String?
    /// The toast showing an Undo button (the same text as `toast` while it shows).
    var toastUndo: String?
    /// The threads of the drag in progress (or the last one), set as it starts, so a
    /// card under it can tell whether a drop would merge before anything is dropped.
    @ObservationIgnored var dragIds: [String] = []
    /// Stands in for the daemon's merge calls in tests; the client otherwise.
    @ObservationIgnored var conversationsStub: (any ConversationBackend)?

    /// Addresses seen in opened threads; feeds the compose recipient check.
    var checker = RecipientChecker()
    /// Every address seen in opened mail, once each.
    private var seenAddresses: [Address] = []
    @ObservationIgnored private var seenKeys: Set<String> = []

    private(set) var client: DaemonClient?
    /// Google Drive state (AppModel+Drive.swift).
    let drive = DriveModel()
    /// "Ask Mermoz to do" in ⌘K (AskFlow.swift, AppModel+Ask.swift).
    let askFlow = AskFlow()

    /// "Chat about this" (AppModel+Chat.swift): the panel beside the open thread.
    var chatOpen = false
    /// Each thread's conversation, loaded when its panel first shows.
    var chats: [String: [ChatMessage]] = [:]
    /// Answer streaming in, per thread; present while the AI is answering.
    var chatStreaming: [String: String] = [:]
    var chatErrors: [String: String] = [:]
    /// Whether the thread's AI can search the web.
    var chatResearchAvailable: [String: Bool] = [:]
    /// Let the AI search the web (the panel's globe toggle).
    var chatResearch = true
    /// Proposals already applied ("<message id>#<index>"), shown checked.
    var chatApplied: Set<String> = []
    /// The Feed (AppModel+Feed.swift): its cards and the one keys act on.
    var feed: Feed?
    var feedFocus: String? {
        didSet { if feedFocus != oldValue { feedFocusCells.select(Set([feedFocus].compactMap { $0 })) } }
    }
    /// One flag per Feed card, so j/k redraws the two cards it moves between.
    @ObservationIgnored let feedFocusCells = SelectionCells()
    /// Feed cards shown in full right now; the focus scrolls only to reach others.
    @ObservationIgnored var feedFullyVisible: Set<String> = []
    /// Cards just handled, shown as a "Done · Undo" strip before they fold away.
    var feedStrips: [String: String] = [:]
    /// Threads whose card words were already asked for.
    var feedAsked: Set<String> = []
    /// Cards on screen; only those get AI words and suggestions.
    var feedVisible: Set<String> = []
    /// The suggestion chips on Feed cards (AppModel+Feed.swift).
    let feedSuggester = FeedSuggester()
    var feedSuggestTask: Task<Void, Never>?
    /// Batch mails the user unticked, left out of "Archive all".
    var feedKept: Set<String> = []
    /// The Later picker, while open (AppModel+Later.swift).
    var later: LaterRequest?
    var feedRefreshTask: Task<Void, Never>?
    var feedTextsTask: Task<Void, Never>?
    /// Loads the open conversation; a newer selection cancels it.
    @ObservationIgnored var detailTask: Task<Void, Never>?
    /// How long j/k wait for the next key before loading the pane.
    static let settleMs = 60
    /// Opens the focused card's mail once j/k pause.
    var feedPaneTask: Task<Void, Never>?
    /// When the Feed started opening, until its first card is on screen.
    var feedOpening: ContinuousClock.Instant?
    private var events: Connection?
    private var refreshTask: Task<Void, Never>?

    var isReview: Bool { selection == .status("review") }

    var openStatuses: [Status] { statuses.filter { !closedStatusKeys.contains($0.key) } }

    /// Statuses the current view lists (nil: People, which lists no threads).
    var viewStatuses: [String]? {
        let kind: ViewStatuses.Kind
        switch selection {
        case let .status(k): kind = .status(k)
        case .project, .label: kind = .mailbox
        case .account: kind = .open
        case .people, .docs: return nil
        }
        return ViewStatuses.visible(kind, all: statuses.map(\.key), open: openStatuses.map(\.key), board: layout == .board)
    }

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
            // Assigning an equal value still invalidates every view reading it.
            let (na, ns, np, nl) = try await (a, s.sorted { $0.order < $1.order }, p, l)
            if na != accounts { accounts = na }
            if ns != statuses { statuses = ns }
            if np != projects { projects = np }
            if nl != labels { labels = nl }
            agentsMaySend = try await st.agentsMaySend
        } catch {
            report(error)
        }
        refreshCounts()
        await loadList()
        await refreshDriveStatus()
    }

    /// Asks for fresh sidebar counts; opens, events and edits in a burst share one fetch.
    func refreshCounts() { countsRefresh.request() }

    private func fetchCounts() async {
        guard let client else { return }
        do {
            async let c = client.counts()
            async let r = client.drafts(state: "review")
            let (fresh, review) = try await (c, r)
            if review != reviewDrafts { reviewDrafts = review }
            countCells.apply(fresh, extra: [.reviewBadge: max(review.count, fresh.status["review"] ?? 0)])
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
        if selection == .docs {
            await loadFiles()
            return
        }
        if showsFeed {
            await loadFeed()
            return
        }
        if isReview && layout != .board {
            do {
                reviewDrafts = try await client.drafts(state: "review")
                if selectedDraftId == nil { selectedDraftId = reviewDrafts.first?.id }
            } catch { report(error) }
            return
        }
        let d = effectiveDisplay
        let order = d.ordering.rawValue
        let reverse = d.reversed
        var project: String?
        var accountId: String?
        var labelId: String?
        // A project or label shows everything filed there (archived included) except the Trash.
        let statuses = viewStatuses
        switch selection {
        case .status: break
        case let .project(id): project = id
        case let .account(id): accountId = id
        case let .label(acc, id): accountId = acc; labelId = id
        case .people, .docs: return
        }
        let kind: [String]? = nil
        do {
            var fetched: [ThreadSummary] = []
            var total = 0
            if layout == .board, d.grouping == .status, let sts = statuses, labelId == nil {
                // One page per column, so every status shows its own newest threads and real size.
                try await withThrowingTaskGroup(of: (Int, ThreadPage).self) { group in
                    for (i, k) in sts.enumerated() {
                        group.addTask { [client, project, accountId, order, reverse, kind] in
                            (i, try await client.threads(status: [k], project: project, accountId: accountId, order: order,
                                                         reverse: reverse, limit: 150, kind: kind))
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
                // A refresh keeps the pages already scrolled through.
                let limit = layout == .board ? 600 : max(300, threads.count)
                pageQuery = PageQuery(statuses: statuses, project: project, accountId: accountId, labelId: labelId, order: order,
                                      reverse: reverse, kind: kind)
                let page = try await client.threads(status: statuses, project: project, accountId: accountId, labelId: labelId,
                                                    order: order, reverse: reverse, limit: limit, kind: kind)
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

    // MARK: paging

    /// The filters of the list on screen, reused to fetch its next page.
    private struct PageQuery {
        var statuses: [String]?
        var project: String?
        var accountId: String?
        var labelId: String?
        var order: String
        var reverse: Bool
        var kind: [String]?
    }

    private var pageQuery: PageQuery?
    private var loadingMore = false

    /// Appends the next 300 threads when the list is scrolled to its end.
    func loadMore() {
        guard let client, let q = pageQuery, !loadingMore, threads.count < totalThreads else { return }
        loadingMore = true
        let offset = threads.count
        Task {
            defer { loadingMore = false }
            do {
                let page = try await client.threads(status: q.statuses, project: q.project, accountId: q.accountId, labelId: q.labelId,
                                                    order: q.order, reverse: q.reverse, limit: 300, offset: offset, kind: q.kind)
                let known = Set(threads.map(\.id))
                threads += page.threads.filter { !known.contains($0.id) }
                totalThreads = page.total
            } catch { report(error) }
        }
    }

    func canOpenGroup(_ field: Grouping, _ key: String) -> Bool {
        switch field {
        case .status, .account: true
        case .project: !key.isEmpty
        case .priority, .none: false
        }
    }

    /// Clicking a group header opens that status, project or account.
    func openGroup(_ field: Grouping, _ key: String) {
        guard canOpenGroup(field, key) else { return }
        switch field {
        case .status:
            layout = .list
            select(.status(key))
        case .project: select(.project(key))
        case .account: select(.account(key))
        case .priority, .none: break
        }
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
            return DisplayOptions(grouping: .none, ordering: listDisplay.ordering, reversed: listDisplay.reversed)
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
        case .status: return countCells.value(.status(g.key)) ?? g.all.count
        case .priority: return countCells.value(.priority(g.key)) ?? g.all.count
        case .project: return countCells.value(.project(g.key)) ?? g.all.count
        case .account: return countCells.value(.account(g.key)) ?? g.all.count
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
            open(next, record: false, keepSelection: true, settle: true)
        } else {
            open(next, record: false, settle: true)
        }
    }

    private static func loadDisplay(_ key: String) -> DisplayOptions? {
        guard let d = UserDefaults.standard.data(forKey: "display.\(key)") else { return nil }
        return try? JSONDecoder().decode(DisplayOptions.self, from: d)
    }

    private static func loadReadingModes() -> ReadingModes {
        guard let d = UserDefaults.standard.data(forKey: "reading.modes") else { return ReadingModes() }
        return (try? JSONDecoder().decode(ReadingModes.self, from: d)) ?? ReadingModes()
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

    /// Rewrites one message's Optimized version now (the ✦ button on a message).
    func aiClean(_ threadId: String, message messageId: String) {
        guard let client else { return }
        aiBusy.insert("clean:\(messageId)")
        Task {
            defer { aiBusy.remove("clean:\(messageId)") }
            do {
                _ = try await client.aiClean(thread: threadId, message: messageId)
                await reloadDetail()
            } catch { report(error) }
        }
    }

    /// Messages already sent for an Optimized version this session, so a failure (no AI
    /// set up, quota) is not retried on every render.
    private var optimizeRequested: Set<String> = []

    /// Builds a message's Optimized version if it has none. Stored versions are reused
    /// forever; the AI is never asked twice for a message.
    func optimize(_ threadId: String, message messageId: String) {
        guard let client, optimizeRequested.insert(messageId).inserted else { return }
        aiBusy.insert("optimize:\(messageId)")
        Task {
            defer { aiBusy.remove("optimize:\(messageId)") }
            do {
                _ = try await client.aiClean(thread: threadId, message: messageId, missingOnly: true)
                await reloadDetail()
            } catch { report(error) }
        }
    }

    // MARK: thread

    /// `record: false` for keyboard stepping (j/k, arrows): it updates the current history
    /// entry instead of stacking one per thread. `settle`: the row is selected at once and
    /// its conversation loads once the keys pause, so holding j never draws passing panes.
    func open(_ id: String?, record: Bool = true, keepSelection: Bool = false, settle: Bool = false) {
        if record, id != selectedThreadId { recordHistory() }
        // Writing an equal value still redraws every view reading it, so each is guarded.
        if !keepSelection {
            if !multiSelection.isEmpty { multiSelection = [] }
            anchorId = id
        }
        if id != selectedThreadId {
            if conversationTab != .messages { conversationTab = .messages }
            if !expandedMessages.isEmpty { expandedMessages = [] }
            if chatShowsAll { chatShowsAll = false }
            if focusedMessageId != nil { focusedMessageId = nil }
        }
        if id != selectedThreadId { selectedThreadId = id }
        if compose != nil { compose = nil }
        detailTask?.cancel()
        guard let id, let client else { detail = nil; return }
        // Shown at once from the cache; the fetch below refreshes it and marks it read.
        if detail?.thread.id != id, let cached = detailCache.get(id) { detail = cached }
        detailTask = Task {
            if settle {
                try? await Task.sleep(for: .milliseconds(Self.settleMs))
                guard !Task.isCancelled else { return }
            }
            do {
                let d = try await client.thread(id, latest: detailWindow)
                guard selectedThreadId == id else { return }
                // Bubble text is cleaned up, built and measured off the main thread before it
                // shows; the chat then finds every piece in the caches.
                let full = shownInFull
                let messages = d.messages
                let subject = d.thread.subject.isEmpty ? "(no subject)" : d.thread.subject
                await Task.detached(priority: .userInitiated) {
                    let bubbles = messages.map { (id: $0.id, content: BubbleContent.compact($0, showAll: full.contains($0.id))) }
                    BubbleTextCache.shared.prepare(bubbles)
                    BubbleTextCache.shared.prepareTitle(subject)
                }.value
                guard selectedThreadId == id else { return }
                detail = d
                detailCache.put(d, for: id)
                learnAddresses(from: d)
                if d.thread.unread {
                    // Reading an Unread thread applies the AI's suggestion; remember it for Undo.
                    if d.thread.status == "unread", let sg = d.thread.suggestion, !sg.isEmpty {
                        appliedSuggestion[id] = sg
                    }
                    // The answer carries the thread as reading left it: patched in place,
                    // the pane keeps its messages instead of loading them all again.
                    let read = try? await client.markRead(id)
                    if let read, selectedThreadId == id, let current = detail, current.thread.id == id {
                        detail = current.afterReading(read)
                        // The row only: setLocal would write the pane's detail a second time.
                        if let i = threads.firstIndex(where: { $0.id == id }), threads[i] != read { threads[i] = read }
                    } else if let read {
                        setLocal(id) { $0 = read }
                    } else {
                        setLocal(id) { $0.unread = false }
                    }
                    refreshCounts()
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
                refreshCounts()
                await loadList()
            } catch { report(error) }
        }
    }

    /// `y`: accepts the selected threads' suggestions and lands on the next row.
    func acceptSelected() {
        let ids = targetIds.filter { id in threads.first { $0.id == id }?.suggestion != nil }
        guard !ids.isEmpty else { return }
        // Undo is recorded from the rows on screen, before they move on.
        acceptSuggestions(ids)
        advance(past: ids)
    }

    /// `x`: turns the selected threads' suggestions down (they stay where they are and
    /// are not suggested again); ⌘Z brings the suggestions back.
    func rejectSelected() {
        let ids = targetIds.filter { id in threads.first { $0.id == id }?.suggestion != nil }
        guard !ids.isEmpty, let client else { return }
        reject(ids, client: client)
    }

    /// Turns one thread's suggestion down (the × on its chip).
    func rejectSuggestion(_ id: String) {
        guard let client else { return }
        reject([id], client: client)
    }

    private func reject(_ ids: [String], client: DaemonClient) {
        for id in ids { setLocal(id) { $0.suggestion = nil } }
        undoStack.append(UndoEntry(label: ids.count == 1 ? "Reject Suggestion" : "Reject \(ids.count) Suggestions",
                                   before: [], rejected: ids))
        Task {
            do {
                try await client.rejectSuggestions(ids)
                toast = ids.count == 1 ? "Suggestion dismissed" : "Dismissed \(ids.count) suggestions"
                await loadList()
            } catch { report(error) }
        }
    }

    /// Opens the row after `ids`.
    private func advance(past ids: [String]) {
        let order = layout == .list ? displayOrder : threads
        let idx = order.firstIndex { ids.contains($0.id) }
        let rest = order.filter { !ids.contains($0.id) }
        multiSelection = []
        guard let idx, ids.contains(selectedThreadId ?? "") else { return }
        open(rest.isEmpty ? nil : rest[min(idx, rest.count - 1)].id, record: false)
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
        dragIds = multiSelection.count > 1 && multiSelection.contains(id) ? multiSelection : [id]
        return dragIds.joined(separator: "\n")
    }

    /// What dragging a sidebar status carries; it is no thread, so no card offers a merge.
    func statusDragPayload(_ key: String) -> String {
        dragIds = []
        return "status:\(key)"
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
        } else if later != nil {
            later = nil
        } else if paletteMode != nil {
            paletteMode = nil
        } else if !expandedMessages.isEmpty {
            // Esc folds opened emails back into the chat first.
            expandedMessages = []
        } else if chatOpen {
            chatOpen = false
        } else if !multiSelection.isEmpty {
            multiSelection = []
            anchorId = selectedThreadId
        } else if selectedThreadId != nil {
            open(nil)
        } else if isReview, selectedDraftId != nil {
            selectedDraftId = nil
        } else if layout == .board, !isAllOpenBoard {
            // Like Linear: the last Esc lands back on every open thread.
            select(.status(statuses.first?.key ?? "unread"))
        }
    }

    /// The board of every open thread (the board ignores which status is selected).
    var isAllOpenBoard: Bool {
        if case .status = selection { return layout == .board } else { return false }
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
        let fresh = d.messages.flatMap { [$0.from] + $0.to + $0.cc }.filter { seenKeys.insert($0.email.lowercased()).inserted }
        // Rebuilt only when this conversation brings someone new.
        guard !fresh.isEmpty else { return }
        seenAddresses += fresh
        checker = RecipientChecker(addresses: seenAddresses)
    }

    private func selectionMoved() {
        let ids = multiSelection.isEmpty ? Set([selectedThreadId].compactMap { $0 }) : Set(multiSelection)
        selectionCells.select(ids)
        if hasSelection != (selectedThreadId != nil) { hasSelection = selectedThreadId != nil }
        revealSelection?()
    }

    /// Set by the List's table: brings the selected row into view in the same update as
    /// the selection moves, so the frame that shows the new highlight shows its row.
    @ObservationIgnored var revealSelection: (() -> Void)?

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
        // A thread moved to a status this view doesn't show leaves it at once, and the
        // total follows so no "Showing n of n+1" flashes until the refresh.
        let beforeTotal = totalThreads
        if let status, let visible = viewStatuses, !visible.contains(status) {
            let count = threads.count
            threads.removeAll { ids.contains($0.id) }
            totalThreads = max(0, totalThreads - (count - threads.count))
        }
        Task {
            do {
                try await client.updateThreads(ids: ids, status: status, priority: priority,
                                               addProjects: addProjects, removeProjects: removeProjects, due: due)
                refreshCounts()
            } catch {
                threads = before
                totalThreads = beforeTotal
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
                refreshCounts()
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
        let before = ids.compactMap { id in
            threads.first { $0.id == id } ?? (detail?.thread.id == id ? detail?.thread : nil) ?? feedThread(id)
        }
        guard !before.isEmpty else { return }
        undoStack.append(UndoEntry(label: label, before: before))
        if undoStack.count > 100 { undoStack.removeFirst() }
    }

    var undoLabel: String? { undoStack.last?.label }

    /// Puts the threads of the last change back as they were: read state, status,
    /// priority, projects and follow-up date.
    @discardableResult
    func undo() -> Task<Void, Never>? {
        guard let entry = undoStack.popLast() else { toast = "Nothing to undo"; return nil }
        if let token = entry.unmerge { return unmerge(token, label: entry.label) }
        guard let client else { toast = "Nothing to undo"; return nil }
        return Task {
            do {
                if let op = entry.doOperation {
                    try await client.doUndo(op)
                    toast = "Undid \(entry.label)"
                    refreshCounts()
                    await loadList()
                    return
                }
                if let token = entry.answerUndo, let s = entry.answered {
                    try await feedSuggester.undo(token, s, lastDate: entry.answeredDate, client: client)
                    toast = "Undid \(entry.label)"
                    if showsFeed {
                        await loadFeed()
                        feedRestore([s.id])
                    }
                    return
                }
                if !entry.rejected.isEmpty { try await client.reopenSuggestions(entry.rejected) }
                if !entry.unsnooze.isEmpty { try await client.unsnooze(entry.unsnooze) }
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
                refreshCounts()
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
                refreshCounts()
            } catch { report(error) }
        }
    }

    /// Drag source for a message's attachment: Finder (or a composer) gets the
    /// local file, looked up only when the drop happens.
    func dragProvider(for a: Attachment) -> NSItemProvider {
        let p = NSItemProvider()
        p.suggestedName = a.filename
        let client = client
        p.registerDataRepresentation(forTypeIdentifier: UTType.fileURL.identifier, visibility: .all) { done in
            Task {
                do {
                    guard let client else { throw CocoaError(.fileNoSuchFile) }
                    let f = try await client.attachment(a.id)
                    done(URL(fileURLWithPath: f.path).dataRepresentation, nil)
                } catch { done(nil, error) }
            }
            return nil
        }
        return p
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

    func html(for messageId: String, full: Bool = false, images: Bool = false) async -> HTMLBody? {
        try? await client?.html(messageId: messageId, full: full, images: images)
    }

    // MARK: compose

    /// The full compose window on a reply to the open conversation.
    func startReply() {
        guard let d = detail else { return }
        compose = replyContext(d)
    }

    /// A reply-all to the conversation's latest message, in the Gmail thread it continues.
    func replyContext(_ d: ThreadDetail) -> ComposeContext {
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
        // In a conversation the reply goes to the Gmail thread of its latest message.
        let target = replyTarget(d)
        let subj = target.subject.lowercased().hasPrefix("re:") ? target.subject : "Re: \(target.subject)"
        return ComposeContext(threadId: target.id, accountId: d.thread.accountId, to: to, cc: cc, subject: subj)
    }

    func startNew() {
        selectedThreadId = nil
        detail = nil
        compose = ComposeContext(threadId: nil, accountId: accounts.first?.id, to: [], cc: [], subject: "")
    }

    func editDraft(_ d: Draft) {
        compose = ComposeContext(threadId: d.threadId, accountId: d.accountId, to: d.to, cc: d.cc,
                                 subject: d.subject, body: d.body, draftId: d.id,
                                 attachments: AttachmentList(stored: d.attachments))
    }

    /// Creates (or updates) the draft and sends it; the app is a trusted caller so it sends directly.
    func send(_ c: ComposeContext, thenWaiting: Bool) async -> Bool {
        guard let client else { return false }
        if let tooBig = c.attachments.limitMessage {
            toast = tooBig
            return false
        }
        do {
            var draft: Draft
            if let id = c.draftId {
                draft = try await client.updateDraft(id, to: c.to, cc: c.cc, subject: c.subject, body: c.body)
                for removed in c.attachments.removedStored {
                    draft = try await client.detach(draft: id, attachmentId: removed)
                }
                if !c.attachments.newSpecs.isEmpty {
                    draft = try await client.attach(draft: id, c.attachments.newSpecs)
                }
            } else {
                draft = try await client.createDraft(accountId: c.accountId, threadId: c.threadId, to: c.to,
                                                     cc: c.cc, subject: c.subject, body: c.body,
                                                     attachments: c.attachments.newSpecs)
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
            refreshCounts()
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
        refreshCounts()
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

    /// `ownClient`: the user's own Google sign-in client (Settings › Developer).
    func addGmail(ownClient: OwnGoogleClient? = nil) {
        guard let client else { return }
        Task {
            do {
                let start = try await client.addGmail(ownClient: ownClient)
                await loadSignInStatus()
                if let url = URL(string: start.authUrl) { NSWorkspace.shared.open(url) }
            } catch {
                gmailSignIn.running = false
                gmailSignIn.error = (error as? RPCError)?.message ?? error.localizedDescription
            }
        }
    }

    func loadSignInStatus() async {
        if let s = try? await client?.signInStatus() { gmailSignIn = s }
    }

    func reconnectAccount(_ id: String) {
        guard let client else { return }
        Task {
            do {
                let start = try await client.reconnectAccount(id)
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

    /// Saves (or with `nil` removes) one provider's key; the provider in use stays.
    func setRouterKey(_ key: String?, provider: String) async -> Bool {
        guard let client else { return false }
        do {
            router = try await client.setRouterKey(key, provider: provider)
            return true
        } catch { report(error); return false }
    }

    /// Switches instant sorting to `provider`; only ever on the user's say.
    func setRouterProvider(_ provider: String) async {
        guard let client, provider != router.provider else { return }
        // Shown at once: a picker still reading the old value snaps back and sends it again.
        router.provider = provider
        do {
            router = try await client.setRouterProvider(provider)
        } catch {
            report(error)
            await loadRouter()
        }
    }

    /// Daily allowance of background sorting requests.
    /// Clears every suggestion on read threads nobody reviewed.
    func clearSuggestions() async {
        guard let client else { return }
        do {
            let n = try await client.clearSuggestions()
            toast = n == 1 ? "Cleared 1 suggestion" : "Cleared \(n.formatted()) suggestions"
            await loadRouter()
            await loadList()
        } catch { report(error) }
    }

    func setDailyCap(_ cap: Int) {
        guard let client else { return }
        Task {
            do { router = try await client.setDailyCap(cap) } catch { report(error) }
        }
    }

    func testRouter(key: String?, provider: String) async -> RouterTest? {
        guard let client else { return nil }
        do { return try await client.testRouter(key: key, provider: provider) } catch { report(error); return nil }
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
        return (try? await client.search(q, limit: 30, typing: true)) ?? []
    }

    /// The fallback when nothing local matches: Gmail's own search.
    func searchGmail(_ q: String) async -> [GmailHit] {
        guard let client else { return [] }
        do { return try await client.searchGmail(q) } catch { report(error); return [] }
    }

    /// Brings a message found on Gmail into Mermoz and opens its thread.
    func pullFromGmail(_ hit: GmailHit) {
        guard let client else { return }
        Task {
            do {
                let pulled = try await client.pullFromGmail(accountId: hit.accountId, ids: [hit.id])
                if let id = pulled.first { reveal(try await client.thread(id, latest: 1).thread) }
            } catch { report(error) }
        }
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
            // A changed thread is fetched again before the pane shows it from memory.
            for id in ids { detailCache.drop(id) }
            scheduleRefresh()
            // Any thread of the open conversation changing (one joining, too) refreshes it.
            let shown = Set([selectedThreadId].compactMap { $0 } + (detail?.conversation?.threads.map(\.id) ?? []))
            if let sel = selectedThreadId, !shown.isDisjoint(with: ids), compose == nil {
                Task {
                    if let d = try? await client?.thread(sel, latest: detailWindow), selectedThreadId == sel, d != detail { detail = d }
                }
            }
        case .accountsChanged:
            Task { await refreshAll(); await loadSignInStatus() }
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
            Task { refreshCounts(); if isReview { await loadList() } }
        case .suggestionsChanged:
            break
        case .feedChanged:
            if showsFeed { scheduleFeedRefresh() }
        case let .chatDelta(threadId, text):
            // Late events after the answer landed must not reopen the streaming bubble.
            if chatStreaming[threadId] != nil { chatStreaming[threadId] = text }
        case let .driveChanged(accountId):
            driveChanged(accountId)
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
            refreshCounts()
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
