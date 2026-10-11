/// ⌘K palette: search threads, pick a status/priority, or run a command, all from the keyboard.
import MermozKit
import SwiftUI

struct PaletteItem: Identifiable {
    let id: String
    let title: String
    var detail: String = ""
    var icon: AnyView
    let run: @MainActor () -> Void
}

struct CommandPalette: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @State private var query = ""
    @State private var results: [ThreadSummary] = []
    @State private var cursor = 0
    @FocusState private var focused: Bool

    var body: some View {
        ZStack(alignment: .top) {
            Color(hex: "#D6E6F4").opacity(0.45).ignoresSafeArea()
                .onTapGesture { close() }
            VStack(spacing: 0) {
                HStack(spacing: 10) {
                    Image(systemName: model.paletteMode == .search ? "magnifyingglass" : "arrow.right.circle")
                        .foregroundStyle(Theme.accent)
                    TextField(placeholder, text: $query)
                        .textFieldStyle(.plain)
                        .font(.system(size: 16))
                        .focused($focused)
                        .onSubmit { runSelected() }
                        .onKeyPress(.downArrow) { cursor = min(cursor + 1, max(items.count - 1, 0)); return .handled }
                        .onKeyPress(.upArrow) { cursor = max(cursor - 1, 0); return .handled }
                        .onKeyPress(.escape) { close(); return .handled }
                }
                .padding(.horizontal, 18).frame(height: 56)
                Divider().overlay(Color(hex: "#E3EDF6"))
                ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                            Button { item.run(); close() } label: {
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
                            Text(query.isEmpty ? "Type to search your mail" : "No matches")
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
                HStack(spacing: 16) {
                    Text("↑↓ navigate"); Text("↩ run"); Text("esc close")
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
        .task {
            try? await Task.sleep(for: .milliseconds(20))
            focused = true
        }
        .onChange(of: query) { cursor = 0 }
        .task(id: query) {
            guard model.paletteMode == .search else { return }
            // Debounce keystrokes so the daemon only sees settled queries.
            try? await Task.sleep(for: .milliseconds(90))
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
        default: "Search mail or run a command"
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
            let ranked = commands(sel)
                .map { ($0, CommandMatch.score(q, title: $0.item.title, keywords: $0.keywords)) }
                .filter { $0.1 > 0 }
                .sorted { $0.1 > $1.1 }
                .prefix(8)
                .map(\.0.item)
            return ranked + mail
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
                command("c-project", "Move to project…", "folder", keywords: ["label", "folder", "tag", "file", "add"]) { switchTo(.project) },
                command("c-status", "Set status…", "circle.dashed", detail: "S", keywords: ["move", "state"]) { switchTo(.status) },
                command("c-priority", "Set priority…", "chart.bar", detail: "P", keywords: ["urgent", "important"]) { switchTo(.priority) },
                command("c-done", "Mark done", "checkmark.circle", detail: "E", keywords: ["complete", "finish"]) { model.markDoneSelected() },
                command("c-archive", "Archive", "archivebox", detail: "A", keywords: ["file", "away"]) { model.moveSelected(to: "archive") },
                command("c-delete", "Delete", "trash", detail: "⌫", keywords: ["trash", "bin", "remove"]) { model.moveSelected(to: "deleted") },
                command("c-unread", "Mark unread", "envelope.badge", detail: "U", keywords: ["new"]) { model.markUnreadSelected() },
            ]
            if model.threads.first(where: { $0.id == sel })?.suggestion != nil {
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
            command("c-settings", "Settings", "gearshape", detail: "⌘,", keywords: ["preferences", "options", "typesafe", "ai"]) { openWindow(id: "settings") },
            command("c-shortcuts", "Keyboard shortcuts", "keyboard", detail: "?", keywords: ["help", "keys"]) { model.showShortcuts = true },
            command("c-account", "Add Gmail account", "person.crop.circle.badge.plus", keywords: ["sign", "login", "connect"]) { model.addGmail() },
            command("c-people", "Go to People", "person.2", keywords: ["directory", "contacts"], navigation: true) { model.select(.people) },
        ]
        out += model.statuses.map { s in
            command("g-s-\(s.key)", "Go to \(s.name)", "circle", keywords: ["status", "open", "show"], navigation: true) { model.select(.status(s.key)) }
        }
        out += model.projects.map { p in
            command("g-p-\(p.id)", "Go to \(p.name)", "folder", keywords: ["project", "label", "open", "show"], navigation: true) { model.select(.project(p.id)) }
        }
        return out
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

    private func runSelected() {
        guard items.indices.contains(cursor) else { return }
        let item = items[cursor]
        close()
        item.run()
    }

    private func close() {
        model.paletteMode = nil
        query = ""
    }
}
