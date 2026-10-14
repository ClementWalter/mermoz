/// ⌘K palette: search threads, pick a status/priority, or run a command, all from the keyboard.
import MermozKit
import SwiftUI

struct PaletteItem: Identifiable {
    let id: String
    let title: String
    var detail: String = ""
    var icon: AnyView
    /// The palette stays open (the item fills it with more results).
    var keepsOpen = false
    let run: @MainActor () -> Void
}

struct CommandPalette: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @State private var results: [ThreadSummary] = []
    /// Gmail's own results for this query, once asked.
    @State private var gmail: [GmailHit]?
    @State private var cursor = 0
    @State private var navCache = NavCache()
    @FocusState private var focused: Bool

    var body: some View {
        // Rows are worked out once per render, not once per use.
        let items = self.items
        return ZStack(alignment: .top) {
            Color(hex: "#D6E6F4").opacity(0.45).ignoresSafeArea()
                .onTapGesture { close() }
            VStack(spacing: 0) {
                HStack(spacing: 10) {
                    Image(systemName: model.paletteMode == .search ? "magnifyingglass" : "arrow.right.circle")
                        .foregroundStyle(Theme.accent)
                    TextField(placeholder, text: Binding(get: { askFlow.query }, set: { askFlow.query = $0 }))
                        .textFieldStyle(.plain)
                        .font(.system(size: 16))
                        .focused($focused)
                        // While a plan shows, Enter confirms it; the hidden list never runs.
                        .onSubmit { if askFlow.isActive { model.enterAsk() } else { runSelected(items) } }
                        .onKeyPress(.downArrow) {
                            if askFlow.isActive { askFlow.moveChoice(1) } else { cursor = min(cursor + 1, max(items.count - 1, 0)) }
                            return .handled
                        }
                        .onKeyPress(.upArrow) {
                            if askFlow.isActive { askFlow.moveChoice(-1) } else { cursor = max(cursor - 1, 0) }
                            return .handled
                        }
                        // Esc drops a plan first, then closes the palette.
                        .onKeyPress(.escape) { askFlow.isActive ? askFlow.cancel() : close(); return .handled }
                }
                .padding(.horizontal, 18).frame(height: 56)
                Divider().overlay(Color(hex: "#E3EDF6"))
                if askFlow.isActive {
                    AskPanel(flow: askFlow)
                } else {
                ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                            Button { item.run(); if !item.keepsOpen { close() } } label: {
                                HStack(spacing: 10) {
                                    item.icon.frame(width: 16)
                                    Text(item.title).lineLimit(1)
                                    Spacer()
                                    Text(item.detail).font(.system(size: 11.5)).foregroundStyle(Theme.text3).lineLimit(1)
                                }
                                .font(.system(size: 13.5))
                                .foregroundStyle(Theme.text)
                                .padding(.horizontal, 18).frame(height: 36)
                                .background(i == cursor ? Theme.accent.opacity(0.11) : .clear)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .id(item.id)
                        }
                        if items.isEmpty {
                            Text(query.isEmpty ? "Type to search your mail" : gmail == nil ? "No matches" : "No matches on Gmail either")
                                .font(.system(size: 13)).foregroundStyle(Theme.text3).padding(18)
                        }
                    }
                }
                .frame(maxHeight: 380)
                // Keep the highlighted row on screen while moving with the arrows.
                .onChange(of: cursor) { _, c in
                    if items.indices.contains(c) { proxy.scrollTo(items[c].id) }
                }
                }
                }
                HStack(spacing: 16) {
                    if askFlow.canConfirm || askFlow.preview?.needsTypedCount == true {
                        Text(askFlow.preview?.needsTypedCount == true ? "type the count, then ↩ delete" : "↩ do it")
                        Text("esc cancel")
                    } else if askFlow.takesAnswer {
                        Text("↩ send the answer"); Text("↑↓ choose"); Text("esc cancel")
                    } else if !askFlow.choices.isEmpty {
                        Text("↑↓ choose"); Text("↩ use it"); Text("esc cancel")
                    } else if askFlow.isActive {
                        Text("esc back")
                    } else {
                        Text("↑↓ navigate"); Text("↩ run"); Text("esc close")
                    }
                    Spacer()
                }
                .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                .padding(.horizontal, 18).padding(.vertical, 10)
            }
            .frame(width: 640)
            .glassEffect(.regular.tint(Color.white.opacity(0.55)), in: RoundedRectangle(cornerRadius: 18))
            .shadow(color: Theme.accent.opacity(0.22), radius: 35, y: 15)
            .padding(.top, 96)
        }
        // Focus once the palette is in the window; setting it during appear loses to the list.
        .defaultFocus($focused, true)
        // Refocus the field when a command opens another picker (status, project…).
        .task(id: model.paletteMode) {
            try? await Task.sleep(for: .milliseconds(30))
            focused = true
        }
        .onChange(of: query) {
            // Only real changes are written: each write re-renders the palette.
            if cursor != 0 { cursor = 0 }
            if gmail != nil { gmail = nil }
            // Editing the sentence drops a plan made for the old one.
            askFlow.fieldEdited()
        }
        .task { await askFlow.refreshAvailability(model.client) }
        .task(id: query) {
            // Answers to the AI are not searches.
            guard model.paletteMode == .search, !askFlow.isActive else { return }
            // Debounce keystrokes so the daemon only sees settled queries.
            try? await Task.sleep(for: .milliseconds(80))
            guard !Task.isCancelled else { return }
            results = await model.search(query)
            cursor = 0
        }
    }

    private var placeholder: String {
        switch model.paletteMode {
        case .status: "Set status…"
        case .priority: "Set priority…"
        case .project: "Add to project, or type a new name…"
        default: askFlow.takesAnswer ? "Answer, or pick one below…" : "Search mail or run a command"
        }
    }

    private var items: [PaletteItem] {
        let q = query.lowercased()
        let sel = model.selectedThreadId
        let targets = model.targetIds
        switch model.paletteMode {
        case .status:
            return model.statuses.filter { q.isEmpty || $0.name.lowercased().contains(q) }.map { s in
                PaletteItem(id: "s-\(s.key)", title: s.name, icon: AnyView(StatusIcon(key: s.key, color: Color(hex: s.color)))) {
                    if !targets.isEmpty { model.update(targets, status: s.key) }
                }
            }
        case .priority:
            return [1, 2, 3, 4, 0].filter { q.isEmpty || (Theme.priorityNames[$0] ?? "").lowercased().contains(q) }.map { p in
                PaletteItem(id: "p-\(p)", title: Theme.priorityNames[p] ?? "", icon: AnyView(PriorityIcon(priority: p))) {
                    if !targets.isEmpty { model.update(targets, priority: p) }
                }
            }
        case .project:
            return projectItems(q)
        default:
            // Commands that match come first; mail results follow.
            let mail: [PaletteItem] = results.map { t in
                PaletteItem(id: t.id, title: t.subject.isEmpty ? "(no subject)" : t.subject,
                            detail: "\(t.number) · \(t.lastFrom)",
                            icon: AnyView(StatusIcon(key: t.status, color: Color(hex: model.color(forStatus: t.status))))) {
                    model.reveal(t)
                }
            }
            if q.isEmpty { return commands(sel).filter { !$0.navigation }.map(\.item) }
            // Nothing here: Gmail's own search, for mail Mermoz doesn't hold.
            let typed = query
            let fallback: [PaletteItem] = !results.isEmpty ? [] : gmail.map { hits in
                hits.map { h in
                    PaletteItem(id: "g-\(h.id)", title: h.subject.isEmpty ? "(no subject)" : h.subject,
                                detail: h.inMermoz ? h.from : "\(h.from) · on Gmail",
                                icon: AnyView(Image(systemName: "envelope"))) { model.pullFromGmail(h) }
                }
            } ?? [PaletteItem(id: "g-search", title: "No matches in Mermoz. Search all of Gmail for “\(typed)”",
                              icon: AnyView(Image(systemName: "magnifyingglass")), keepsOpen: true) {
                Task { gmail = await model.searchGmail(typed) }
            }]
            let ranked = rankedCommands(q, sel)
            // An instruction gets "Ask Mermoz to do" on top; the usual rows stay below.
            let ask = Self.askItem(typed, otherMatches: !ranked.isEmpty || !results.isEmpty,
                                   availability: askFlow.availability) {
                if askFlow.availability?.available == false { openWindow(id: "settings") } else { model.startAsk(typed) }
            }
            return (ask.map { [$0] } ?? []) + ranked + mail + fallback
        }
    }

    private struct Command {
        var item: PaletteItem
        var keywords: [String] = []
        /// "Go to …" entries only show up once the user types.
        var navigation = false
    }

    private func command(_ id: String, _ title: String, _ symbol: String, detail: String = "", keywords: [String] = [],
                         navigation: Bool = false, run: @escaping @MainActor () -> Void) -> Command {
        Command(item: PaletteItem(id: id, title: title, detail: detail, icon: AnyView(Image(systemName: symbol)), run: run),
                keywords: keywords, navigation: navigation)
    }

    private func commands(_ sel: String?) -> [Command] {
        // Opening another palette mode must wait until this one has closed.
        let switchTo: (PaletteMode) -> Void = { m in DispatchQueue.main.async { model.paletteMode = m } }
        var out: [Command] = []
        if let sel {
            out += [
                command("c-reply", "Reply", "arrowshape.turn.up.left", detail: "R", keywords: ["answer", "respond"]) { model.startReply() },
                command("c-chat", "Chat about this", "bubble.left.and.text.bubble.right", detail: "T",
                        keywords: ["ask", "ai", "help", "decide", "advice", "talk", "think", "research", "chatgpt", "claude"]) { model.openChat() },
                command("c-project", "Move to project…", "folder", keywords: ["label", "folder", "tag", "file", "add"]) { switchTo(.project) },
                command("c-status", "Set status…", "circle.dashed", detail: "S", keywords: ["move", "state"]) { switchTo(.status) },
                command("c-priority", "Set priority…", "chart.bar", detail: "P", keywords: ["urgent", "important"]) { switchTo(.priority) },
                command("c-done", "Mark done", "checkmark.circle", detail: "E", keywords: ["complete", "finish"]) { model.markDoneSelected() },
                command("c-archive", "Archive", "archivebox", detail: "A", keywords: ["file", "away"]) { model.moveSelected(to: "archive") },
                command("c-delete", "Delete", "trash", detail: "⌫", keywords: ["trash", "bin", "remove"]) { model.moveSelected(to: "deleted") },
                command("c-unread", "Mark unread", "envelope.badge", detail: "U", keywords: ["new"]) { model.markUnreadSelected() },
                command("c-later", "Later…", "clock.arrow.circlepath", detail: "L",
                        keywords: ["snooze", "remind", "postpone", "defer", "plus tard", "tomorrow", "friday", "weekend"]) {
                    DispatchQueue.main.async { model.openLaterForSelection(typing: false) }
                },
            ]
            if model.threads.first(where: { $0.id == sel })?.suggestion != nil {
                out.insert(command("c-reject", "Dismiss AI suggestion", "xmark", detail: "X", keywords: ["reject", "no"]) { model.rejectSelected() }, at: 0)
                out.insert(command("c-accept", "Accept AI suggestion", "sparkle", detail: "Y", keywords: ["sort", "apply"]) { model.acceptSelected() }, at: 0)
            }
            if !(currentThread?.projects.isEmpty ?? true) {
                out.append(command("c-unproject", "Remove from project…", "folder.badge.minus", keywords: ["label", "folder", "untag"]) { switchTo(.project) })
            }
        }
        if let label = model.undoLabel {
            out.append(command("c-undo", "Undo \(label)", "arrow.uturn.backward", detail: "⌘Z", keywords: ["revert", "back"]) { model.undo() })
        }
        out += [
            command("c-new", "New message", "square.and.pencil", detail: "C", keywords: ["compose", "write", "email"]) { model.startNew() },
            command("c-layout", model.layout == .list ? "Switch to board" : "Switch to list", "rectangle.split.3x1", keywords: ["view", "layout"]) {
                model.layout = model.layout == .list ? .board : .list
                Task { await model.loadList() }
            },
            command("c-feed", "Show the Feed", "text.line.first.and.arrowtriangle.forward", keywords: ["view", "layout", "cards", "todo", "stream"]) {
                model.layout = .feed
                Task { await model.loadList() }
            },
            command("c-settings", "Settings", "gearshape", detail: "⌘,", keywords: ["preferences", "options", "typesafe", "subq", "ai"]) { openWindow(id: "settings") },
            command("c-shortcuts", "Keyboard shortcuts", "keyboard", detail: "?", keywords: ["help", "keys"]) { model.showShortcuts = true },
            command("c-account", "Add Gmail account", "person.crop.circle.badge.plus", keywords: ["sign", "login", "connect"]) { model.addGmail() },
            command("c-people", "Go to People", "person.2", keywords: ["directory", "contacts"], navigation: true) { model.select(.people) },
        ]
        return out
    }

    /// "Go to <status / project>" targets with their words folded once, rebuilt only
    /// when the statuses or projects change, so a keystroke only compares prefixes.
    private final class NavCache {
        struct Target {
            let id: String
            let title: String
            let symbol: String
            let selection: SidebarSelection
            let prepared: CommandMatch.Prepared
        }

        private var statuses: [Status] = []
        private var projects: [Project] = []
        private var cached: [Target] = []

        func targets(_ statuses: [Status], _ projects: [Project]) -> [Target] {
            if statuses == self.statuses, projects == self.projects, !cached.isEmpty { return cached }
            self.statuses = statuses
            self.projects = projects
            let go = { (id: String, name: String, symbol: String, sel: SidebarSelection, kw: [String]) in
                Target(id: id, title: "Go to \(name)", symbol: symbol, selection: sel,
                       prepared: CommandMatch.Prepared(title: "Go to \(name)", keywords: kw))
            }
            cached = statuses.map { go("g-s-\($0.key)", $0.name, "circle", .status($0.key), ["status", "open", "show"]) }
                + projects.map { go("g-p-\($0.id)", $0.name, "folder", .project($0.id), ["project", "label", "open", "show"]) }
            return cached
        }
    }

    /// The best commands for the typed words: actions and "Go to …" targets, top 8.
    private func rankedCommands(_ q: String, _ sel: String?) -> [PaletteItem] {
        let words = CommandMatch.words(q)
        var scored: [(Int, () -> PaletteItem)] = commands(sel).compactMap { c in
            let s = CommandMatch.score(words, CommandMatch.Prepared(title: c.item.title, keywords: c.keywords))
            return s > 0 ? (s, { c.item }) : nil
        }
        for t in navCache.targets(model.statuses, model.projects) {
            let s = CommandMatch.score(words, t.prepared)
            if s > 0 {
                scored.append((s, {
                    PaletteItem(id: t.id, title: t.title, icon: AnyView(Image(systemName: t.symbol))) { model.select(t.selection) }
                }))
            }
        }
        // Stable: equal scores keep their order (actions first, then statuses, projects).
        return scored.enumerated()
            .sorted { $0.element.0 != $1.element.0 ? $0.element.0 > $1.element.0 : $0.offset < $1.offset }
            .prefix(8)
            .map { $0.element.1() }
    }

    private var currentThread: ThreadSummary? {
        guard let sel = model.selectedThreadId else { return nil }
        return model.threads.first { $0.id == sel } ?? (model.detail?.thread.id == sel ? model.detail?.thread : nil)
    }

    /// Toggle the selected threads in or out of projects; a new name creates the project.
    private func projectItems(_ q: String) -> [PaletteItem] {
        let targets = model.targetIds
        let selected = targets.compactMap { id in model.threads.first { $0.id == id } ?? (model.detail?.thread.id == id ? model.detail?.thread : nil) }
        guard !selected.isEmpty else { return [] }
        var out: [PaletteItem] = model.projects
            .map { ($0, CommandMatch.score(q, title: $0.name.replacingOccurrences(of: "/", with: " "))) }
            .filter { $0.1 > 0 }
            .sorted { q.isEmpty ? $0.0.name < $1.0.name : $0.1 > $1.1 }
            .map { p, _ in
                // Checked only when every selected thread is in it.
                let inIt = selected.allSatisfy { $0.projects.contains(p.id) }
                return PaletteItem(id: "pr-\(p.id)", title: p.name, detail: inIt ? "✓ remove" : "",
                                   icon: AnyView(RoundedRectangle(cornerRadius: 3).fill(Color(hex: p.color)).frame(width: 10, height: 10))) {
                    if inIt { model.update(targets, removeProjects: [p.id]) } else { model.update(targets, addProjects: [p.id]) }
                }
            }
        let name = query.trimmingCharacters(in: .whitespaces)
        if !name.isEmpty, !model.projects.contains(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) {
            out.append(PaletteItem(id: "pr-new", title: "Create project “\(name)”", icon: AnyView(Image(systemName: "folder.badge.plus"))) {
                model.update(targets, addProjects: [name])
                Task { await model.refreshAll() }
            })
        }
        return out
    }

    private var askFlow: AskFlow { model.askFlow }

    /// The field's text (kept in the ask flow, which also uses it for answers).
    private var query: String {
        get { askFlow.query }
        nonmutating set { askFlow.query = newValue }
    }

    /// The "Ask Mermoz to do" row, when the text reads as an instruction. Without an AI
    /// the row says where to connect one, and opens Settings.
    static func askItem(_ text: String, otherMatches: Bool, availability: DoAvailability?,
                        run: @escaping @MainActor () -> Void) -> PaletteItem? {
        let t = text.trimmingCharacters(in: .whitespaces)
        guard AskIntent.isInstruction(t, otherMatches: otherMatches) else { return nil }
        let noAI = availability?.available == false
        return PaletteItem(id: "ask", title: "✦ Ask Mermoz to do: “\(t)”",
                           detail: noAI ? "Connect an AI first: Settings › Agents" : "",
                           icon: AnyView(Image(systemName: "sparkles").foregroundStyle(Theme.accent)),
                           keepsOpen: true, run: run)
    }

    private func runSelected(_ items: [PaletteItem]) {
        guard items.indices.contains(cursor) else { return }
        let item = items[cursor]
        if !item.keepsOpen { close() }
        item.run()
    }

    private func close() {
        askFlow.cancel()
        model.paletteMode = nil
        query = ""
    }
}
