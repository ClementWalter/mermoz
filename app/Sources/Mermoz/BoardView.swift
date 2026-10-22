/// Board mode: one column per open status; dropping a card on a column changes its
/// status, dropping it on another card merges the two conversations.
import MermozKit
import SwiftUI

struct BoardView: View {
    @Environment(AppModel.self) private var model
    @FocusState private var focused: Bool

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                Text(title).font(.system(size: 15, weight: .semibold))
                Text(model.totalThreads.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3)
                Spacer()
                DisplayMenu()
                Picker("", selection: $model.layout) {
                    ForEach(LayoutMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden().frame(width: 170)
                .onChange(of: model.layout) { Task { await model.loadList() } }
            }
            .padding(.horizontal, 16)
            .frame(height: 50)
            .glassPanel()

            ScrollViewReader { proxy in
                ScrollView(.horizontal) {
                    HStack(alignment: .top, spacing: 10) {
                        ForEach(model.groups) { g in
                            BoardColumn(group: g, total: model.groupTotal(g)).id(g.id)
                        }
                        if model.effectiveDisplay.grouping == .status { DoneStrip() }
                    }
                    .padding(.bottom, 2)
                }
                .scrollIndicators(.automatic)
                .onChange(of: model.selectedThreadId) { _, id in
                    // Keep the selected card's column on screen while moving with the arrows.
                    if let id, let g = model.groups.first(where: { $0.all.contains { $0.id == id } }) {
                        withAnimation(.easeOut(duration: 0.12)) { proxy.scrollTo(g.id) }
                    }
                }
            }
        }
        .focusable()
        .focused($focused)
        .focusEffectDisabled()
        .onAppear { focused = true }
        // Overlays (⌘K, help, compose) take the keyboard; give it back when they close.
        .onChange(of: model.paletteMode) { _, m in if m == nil { focused = true } }
        .onChange(of: model.showShortcuts) { _, on in if !on { focused = true } }
        .onChange(of: model.compose == nil) { _, closed in if closed { focused = true } }
        .onKeyPress(keys: [.delete, .deleteForward]) { _ in
            guard model.selectedThreadId != nil else { return .ignored }
            model.moveSelected(to: "deleted")
            return .handled
        }
        .onKeyPress(.leftArrow) { model.moveBoard(dx: -1, dy: 0); return .handled }
        .onKeyPress(.rightArrow) { model.moveBoard(dx: 1, dy: 0); return .handled }
        .onKeyPress(keys: [.upArrow, .downArrow]) { press in
            model.moveBoard(dx: 0, dy: press.key == .downArrow ? 1 : -1, extend: press.modifiers.contains(.shift))
            return .handled
        }
    }

    private var title: String {
        switch model.selection {
        case .status: "All open"
        case let .project(id): model.project(id)?.name ?? "Project"
        case let .account(id): model.accounts.first { $0.id == id }?.email ?? "Account"
        case .people: "People"
        case .docs: "Docs"
        case let .label(acc, id): model.labels.first { $0.accountId == acc && $0.id == id }?.name ?? "Label"
        }
    }
}

struct BoardColumn: View {
    @Environment(AppModel.self) private var model
    let group: ThreadGroup
    let total: Int
    @State private var targeted = false
    /// One width for every column; drag any column's right edge to change it.
    @AppStorage("layout.boardColumnWidth") private var columnWidth = 250.0

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                if group.field == .none {
                    Text("All").font(.system(size: 12.5, weight: .semibold))
                } else {
                    GroupLabel(field: group.field, key: group.key).font(.system(size: 12.5, weight: .semibold))
                }
                Text(total.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3)
                Spacer()
                SuggestColumnButton(group: group)
                AcceptAllButton(threads: group.all)
            }
            .padding(.horizontal, 4).padding(.top, 2)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if group.subgroups.isEmpty {
                        cards(group.threads)
                    } else {
                        ForEach(group.subgroups) { sub in
                            HStack(spacing: 6) {
                                GroupLabel(field: sub.field, key: sub.key)
                                Text("\(sub.threads.count)").foregroundStyle(Theme.text3)
                            }
                            .font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.text2)
                            .padding(.top, 4).padding(.horizontal, 4)
                            cards(sub.threads)
                        }
                    }
                }
            }
            .scrollIndicators(.never)
        }
        .padding(10)
        .frame(width: columnWidth)
        .overlay(alignment: .trailing) {
            ResizeHandle(width: $columnWidth, range: 200...480, thickness: 8).offset(x: 9)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .background(Color.white.opacity(targeted ? 0.6 : 0.32), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(targeted ? Theme.accent.opacity(0.5) : Color.white.opacity(0.8)))
        .dropDestination(for: String.self) { items, _ in
            model.dropOnColumn(items, group: group)
        } isTargeted: { targeted = $0 }
    }

    private func cards(_ threads: [ThreadSummary]) -> some View {
        ForEach(threads) { t in
            BoardCard(thread: t, selected: model.isSelected(t.id))
                .id(t.id)
                .onTapGesture {
                    let mods = NSEvent.modifierFlags
                    model.click(t.id, shift: mods.contains(.shift), command: mods.contains(.command))
                }
        }
    }
}

struct BoardCard: View {
    @Environment(AppModel.self) private var model
    let thread: ThreadSummary
    let selected: Bool
    /// The title being typed, while the card is renamed in place.
    @State private var draftTitle: String?
    @FocusState private var titleFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 6) {
                if thread.priority > 0 { PriorityIcon(priority: thread.priority) }
                Text(thread.number).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.text3)
                ThreadCountPill(thread: thread)
                Spacer()
                if let due = thread.due { Chip(small: true) { Text("Due \(Format.due(due))") } } else {
                    Text(Format.short(thread.lastDate)).font(.system(size: 11)).foregroundStyle(Theme.text3)
                }
            }
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if thread.unread { Circle().fill(Theme.accent).frame(width: 7, height: 7).accessibilityLabel("Unread") }
                if draftTitle != nil {
                    TextField(thread.subject, text: Binding(get: { draftTitle ?? "" }, set: { draftTitle = $0 }), axis: .vertical)
                        .textFieldStyle(.plain).font(.system(size: 12.5, weight: .semibold)).lineLimit(1...3)
                        .focused($titleFocused)
                        .onSubmit(commitTitle)
                        .onExitCommand { draftTitle = nil }
                        .onChange(of: titleFocused) { _, focused in if !focused { commitTitle() } }
                        .onAppear { titleFocused = true }
                } else {
                    Text(thread.displayTitle)
                        .font(.system(size: 12.5, weight: thread.unread ? .semibold : .regular)).lineLimit(2)
                        // A renamed card still tells what the email was called.
                        .help(thread.title == nil ? "" : "Subject: \(thread.subject)")
                }
            }
            HStack(spacing: 6) {
                if model.visibleAccounts.count > 1 {
                    // Hollow ring = which account; kept distinct from the filled unread dot.
                    Circle().strokeBorder(Theme.accountColor(thread.accountId, among: model.accounts), lineWidth: 1.5)
                        .frame(width: 7, height: 7).help(model.accounts.first { $0.id == thread.accountId }?.email ?? "")
                }
                Text(thread.lastFrom).lineLimit(1)
                ProjectChips(names: thread.projects, limit: 1)
            }
            .font(.system(size: 11.5)).foregroundStyle(Theme.text2)
            if let sg = thread.suggestion, !sg.isEmpty { SuggestionChip(suggestion: sg, threadId: thread.id) }
            if let next = thread.nextAction, !next.isEmpty {
                Label(next, systemImage: "sparkle").font(.system(size: 11.5)).foregroundStyle(Theme.text2).lineLimit(2)
            }
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 11).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.9), in: RoundedRectangle(cornerRadius: 11))
        .overlay(RoundedRectangle(cornerRadius: 11).strokeBorder(selected ? Theme.accent : Color.white, lineWidth: selected ? 2 : 1))
        .contentShape(Rectangle())
        .draggable(model.dragPayload(thread.id))
        // The card's own drop target, inside the column's: a drop on the card merges, one
        // on the column's header, gaps or empty space sets its status.
        .mergeDropTarget(thread.id, radius: 11)
        .contextMenu {
            Button("Rename Card…") { draftTitle = thread.displayTitle }
            if thread.title != nil { Button("Show Email Subject") { model.rename(thread.id, title: nil) } }
            ConversationMenu()
        }
    }

    /// Saves what was typed; an emptied field gives the card its subject back.
    private func commitTitle() {
        guard let typed = draftTitle else { return }
        draftTitle = nil
        // Leaving the subject as it was is no rename.
        if thread.title == nil, typed.trimmingCharacters(in: .whitespacesAndNewlines) == thread.subject { return }
        model.rename(thread.id, title: typed)
    }
}

/// Collapsed Done column; also a drop target to close threads quickly.
struct DoneStrip: View {
    @Environment(AppModel.self) private var model
    @State private var targeted = false

    var body: some View {
        VStack(spacing: 10) {
            StatusIcon(key: "done", color: Color(hex: model.color(forStatus: "done")))
            Text("Done").font(.system(size: 12, weight: .semibold)).rotationEffect(.degrees(90)).fixedSize().frame(height: 50)
        }
        .padding(.vertical, 14)
        .frame(width: 44)
        .frame(maxHeight: .infinity, alignment: .top)
        .foregroundStyle(Theme.text2)
        .background(Color.white.opacity(targeted ? 0.6 : 0.32), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Color.white.opacity(0.8)))
        .dropDestination(for: String.self) { items, _ in
            model.update(items.flatMap { $0.split(separator: "\n").map(String.init) }, status: "done")
            return true
        } isTargeted: { targeted = $0 }
    }
}
