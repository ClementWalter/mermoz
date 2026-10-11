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
                        }
                        if items.isEmpty {
                            Text(query.isEmpty ? "Type to search your mail" : "No matches")
                                .font(.system(size: 13)).foregroundStyle(Theme.text3).padding(18)
                        }
                    }
                }
                .frame(maxHeight: 380)
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
        default: "Search mail or run a command"
        }
    }

    private var items: [PaletteItem] {
        let q = query.lowercased()
        let sel = model.selectedThreadId
        switch model.paletteMode {
        case .status:
            return model.statuses.filter { q.isEmpty || $0.name.lowercased().contains(q) }.map { s in
                PaletteItem(id: "s-\(s.key)", title: s.name, icon: AnyView(StatusIcon(key: s.key, color: Color(hex: s.color)))) {
                    if let sel { model.update([sel], status: s.key) }
                }
            }
        case .priority:
            return [1, 2, 3, 4, 0].filter { q.isEmpty || (Theme.priorityNames[$0] ?? "").lowercased().contains(q) }.map { p in
                PaletteItem(id: "p-\(p)", title: Theme.priorityNames[p] ?? "", icon: AnyView(PriorityIcon(priority: p))) {
                    if let sel { model.update([sel], priority: p) }
                }
            }
        default:
            var out: [PaletteItem] = results.map { t in
                PaletteItem(id: t.id, title: t.subject.isEmpty ? "(no subject)" : t.subject,
                            detail: "\(t.number) · \(t.lastFrom)",
                            icon: AnyView(StatusIcon(key: t.status, color: Color(hex: model.color(forStatus: t.status))))) {
                    model.reveal(t)
                }
            }
            var commands: [PaletteItem] = [
                PaletteItem(id: "c-new", title: "New message", detail: "C",
                            icon: AnyView(Image(systemName: "square.and.pencil"))) { model.startNew() },
                PaletteItem(id: "c-layout", title: model.layout == .list ? "Switch to board" : "Switch to list",
                            icon: AnyView(Image(systemName: "rectangle.split.3x1"))) {
                    model.layout = model.layout == .list ? .board : .list
                    Task { await model.loadList() }
                },
                PaletteItem(id: "c-account", title: "Add Gmail account",
                            icon: AnyView(Image(systemName: "person.crop.circle.badge.plus"))) { model.addGmail() },
            ]
            if sel != nil {
                commands.insert(PaletteItem(id: "c-reply", title: "Reply", detail: "R",
                                            icon: AnyView(Image(systemName: "arrowshape.turn.up.left"))) { model.startReply() }, at: 0)
                commands.insert(PaletteItem(id: "c-status", title: "Set status…", detail: "S",
                                            icon: AnyView(Image(systemName: "circle.dashed"))) {
                    DispatchQueue.main.async { model.paletteMode = .status }
                }, at: 1)
                commands.insert(PaletteItem(id: "c-done", title: "Mark done", detail: "E",
                                            icon: AnyView(Image(systemName: "checkmark.circle"))) { model.markDoneSelected() }, at: 2)
            }
            out += commands.filter { q.isEmpty || $0.title.lowercased().contains(q) }
            return out
        }
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
