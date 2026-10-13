/// Middle pane in list mode: threads grouped and ordered per the Display menu, keyboard-driven.
import MermozKit
import SwiftUI

struct ThreadListView: View {
    @Environment(AppModel.self) private var model
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            ListHeader()
            Divider().overlay(Theme.line)
            if case let .status(k) = model.selection, model.sortInfo.count(k) > 0 {
                HStack { SortStartButton(status: k); Spacer() }.padding(.horizontal, 16).padding(.vertical, 8)
            }
            if model.threads.isEmpty {
                VStack(spacing: 6) {
                    Image(systemName: "checkmark.circle").font(.system(size: 24, weight: .light))
                    Text(model.selection == .feeds ? Subscriptions.empty : "Nothing here").font(.system(size: 13))
                }
                .foregroundStyle(Theme.text3)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        ThreadListRows(groups: model.groups, loaded: model.threads.count, total: model.totalThreads) {
                            focused = true
                        }
                        .equatable()
                    }
                    .onScrollTargetVisibilityChange(idType: String.self, threshold: 0.99) { model.visibleRows = Set($0) }
                    .background { SelectionScroller(proxy: proxy) }
                }
            }
        }
        .listPanel()
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
        .onKeyPress(keys: [.downArrow, .upArrow]) { press in
            model.moveSelection(press.key == .downArrow ? 1 : -1, extend: press.modifiers.contains(.shift))
            return .handled
        }
        .onKeyPress(.return) {
            if model.selectedThreadId == nil { model.moveSelection(1) }
            return .handled
        }
    }
}

/// The list's rows. Its inputs are plain values, so a parent redraw that leaves the
/// groups as they were skips it; the selection reaches rows through their own flags.
struct ThreadListRows: View, Equatable {
    @Environment(AppModel.self) private var model
    let groups: [ThreadGroup]
    let loaded: Int
    let total: Int
    /// Gives the keyboard back to the list after a click.
    let refocus: () -> Void

    nonisolated static func == (a: ThreadListRows, b: ThreadListRows) -> Bool {
        a.groups == b.groups && a.loaded == b.loaded && a.total == b.total
    }

    var body: some View {
        LazyVStack(spacing: 0, pinnedViews: [.sectionHeaders]) {
            ForEach(groups) { g in
                Section {
                    if g.subgroups.isEmpty {
                        rows(g.threads, in: g.id)
                    } else {
                        ForEach(g.subgroups) { sub in
                            SubGroupHeader(group: sub)
                            rows(sub.threads, in: g.id + "/" + sub.id)
                        }
                    }
                } header: {
                    if g.field != .none { GroupHeaderView(group: g) }
                }
            }
            if total > loaded {
                // Reaching the end loads the next page.
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading more · \(loaded) of \(total.formatted())")
                }
                .font(.system(size: 11.5)).foregroundStyle(Theme.text3).padding(12)
                .onAppear { model.loadMore() }
            }
        }
        // Reports which rows show in full, so the selection scrolls only to reveal one.
        .scrollTargetLayout()
    }

    /// A thread in two project groups appears in both: each copy gets its own id, or
    /// SwiftUI draws only one and leaves an empty slot.
    private func rows(_ threads: [ThreadSummary], in group: String) -> some View {
        ForEach(threads) { t in
            ThreadRow(thread: t, selection: model.selectionCells.flag(t.id))
                .equatable()
                .id(group + "|" + t.id)
                .onTapGesture {
                    let mods = NSEvent.modifierFlags
                    model.click(t.id, shift: mods.contains(.shift), command: mods.contains(.command))
                    refocus()
                }
        }
    }
}

/// Single-key shortcuts, handled for the whole main window (see `install`) so they
/// work wherever the focus is: list, board or a message being read.
enum ThreadKeys {
    static let characters = "jkeay#usmMprct?/xn"

    /// Routes key presses to the shortcuts unless the user is typing in a field, a
    /// draft is open, or another window (Settings) is in front.
    @MainActor
    static func install(model: AppModel) -> Any? {
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            let f = event.modifierFlags
            guard let window = event.window, window.title != "Mermoz Settings",
                  !(window.firstResponder?.isTyping ?? false),
                  model.compose == nil, model.paletteMode == nil, model.later == nil,
                  let c = ShortcutKey.normalize(event.characters ?? "", command: f.contains(.command),
                                                control: f.contains(.control), option: f.contains(.option),
                                                known: characters + FeedKeys.characters),
                  FeedKeys.handle(c, shift: f.contains(.shift), model: model)
                    || handle(c, shift: f.contains(.shift), model: model)
            else { return event }
            return nil
        }
    }

    @MainActor
    static func handle(_ c: String, shift: Bool = false, model: AppModel) -> Bool {
        // The Sort queue answers with y / n / c / s; the other keys work as everywhere.
        if model.selection == .sort {
            switch c {
            case "y": model.sortYes(); return true
            case "n": model.sortNo(); return true
            case "c": model.paletteMode = .sortChange; return true
            case "s": model.sortSkip(); return true
            default: break
            }
        }
        switch c {
        case "j": model.moveSelection(1)
        case "k": model.moveSelection(-1)
        case "e": model.markDoneSelected()
        case "a": model.moveSelected(to: "archive")
        // Typed character, so it is Shift+3 on QWERTY and a plain key on AZERTY alike.
        case "#": model.moveSelected(to: "deleted")
        case "y": model.acceptSelected()
        case "x": model.rejectSelected()
        // Only the Sort queue answers No; elsewhere the key types as usual.
        case "n": return false
        case "u": model.markUnreadSelected()
        case "s": if model.selectedThreadId != nil { model.paletteMode = .status }
        case "p": if model.selectedThreadId != nil { model.paletteMode = .priority }
        case "m": if model.selectedThreadId != nil { model.paletteMode = .project }
        case "M": model.mergeSelected()
        case "r": model.startReply()
        case "c": model.startNew()
        case "t": if model.selectedThreadId != nil { model.toggleChat() }
        // Later: l on the quick choices, ⇧L straight to typing when.
        case "l": if model.selectedThreadId != nil { model.openLaterForSelection(typing: shift) }
        // Matched by typed character, so it works on AZERTY (Shift+,) as on QWERTY.
        case "?": model.showShortcuts.toggle()
        case "/": model.paletteMode = .search
        default: return false
        }
        return true
    }
}

struct ListHeader: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        // The title reads in full first: the controls shrink to icons, then the count goes,
        // and only then does the title end with an ellipsis. Never a column of letters.
        ViewThatFits(in: .horizontal) {
            row(count: true, compact: false)
            row(count: true, compact: true)
            row(count: false, compact: true)
        }
        .padding(.leading, 16).padding(.trailing, 12)
        .frame(height: 50)
        .onChange(of: model.layout) { Task { await model.loadList() } }
    }

    private func row(count: Bool, compact: Bool) -> some View {
        HStack(spacing: 10) {
            HStack(spacing: 10) {
                title
                if count {
                    Text(model.isReview ? "\(model.reviewDrafts.count)" : model.totalThreads.formatted())
                        .font(.system(size: 12)).foregroundStyle(Theme.text3)
                }
            }
            .lineLimit(1)
            .truncationMode(.tail)
            .layoutPriority(1)
            if model.multiSelection.count > 1 {
                Chip(small: true) { Text("\(model.multiSelection.count) selected") }
                    .help("Actions apply to all of them · Esc to clear")
            }
            Spacer(minLength: 0)
            if !model.isReview {
                DisplayMenu(compact: compact)
                LayoutSwitch(compact: compact)
            }
            Button { model.startNew() } label: { Image(systemName: "square.and.pencil") }
                .buttonStyle(.plain)
                .foregroundStyle(Theme.text2)
                .help("New message (c)")
                .accessibilityLabel("New message")
        }
    }

    @ViewBuilder private var title: some View {
        switch model.selection {
        case let .status(k):
            StatusIcon(key: k, color: Color(hex: model.color(forStatus: k)))
            Text(model.statuses.first { $0.key == k }?.name ?? k).font(.system(size: 15, weight: .semibold))
        case let .project(id):
            RoundedRectangle(cornerRadius: 3).fill(Color(hex: model.project(id)?.color ?? "#5B8DEF")).frame(width: 10, height: 10)
            Text(model.project(id)?.name ?? "Project").font(.system(size: 15, weight: .semibold))
        case let .account(id):
            Circle().fill(Theme.accountColor(id, among: model.accounts)).frame(width: 9, height: 9)
            Text(model.accounts.first { $0.id == id }?.email ?? "Account").font(.system(size: 15, weight: .semibold)).lineLimit(1)
        case .people:
            Text("People").font(.system(size: 15, weight: .semibold))
        case .feeds:
            Image(systemName: "dot.radiowaves.up.forward").font(.system(size: 12)).foregroundStyle(Theme.text2)
            Text(Subscriptions.title).font(.system(size: 15, weight: .semibold)).help(Subscriptions.tooltip)
        case .docs:
            Text("Docs").font(.system(size: 15, weight: .semibold))
        case .sort:
            Text("Sort").font(.system(size: 15, weight: .semibold))
        case let .label(acc, id):
            Image(systemName: "folder").font(.system(size: 13)).foregroundStyle(Theme.text2)
            Text(model.labels.first { $0.accountId == acc && $0.id == id }?.name ?? "Label")
                .font(.system(size: 15, weight: .semibold)).lineLimit(1)
        }
    }
}

/// Icon + name of a group value (a status, a priority, a project, an account).
struct GroupLabel: View {
    @Environment(AppModel.self) private var model
    let field: Grouping
    let key: String

    var body: some View {
        HStack(spacing: 8) {
            switch field {
            case .status:
                StatusIcon(key: key, color: Color(hex: model.color(forStatus: key)))
                Text(model.statuses.first { $0.key == key }?.name ?? key)
            case .priority:
                PriorityIcon(priority: Int(key) ?? 0)
                Text(Theme.priorityNames[Int(key) ?? 0] ?? "")
            case .project:
                RoundedRectangle(cornerRadius: 3).fill(Color(hex: model.project(key)?.color ?? "#C6D7E6")).frame(width: 9, height: 9)
                Text(model.project(key)?.name ?? "No project")
            case .account:
                Circle().fill(Theme.accountColor(key, among: model.accounts)).frame(width: 8, height: 8)
                Text(model.accounts.first { $0.id == key }?.email ?? key).lineLimit(1)
            case .none:
                EmptyView()
            }
        }
    }
}

struct GroupHeaderView: View {
    @Environment(AppModel.self) private var model
    let group: ThreadGroup

    var body: some View {
        // Read here, so a count moving redraws this header alone.
        let total = model.groupTotal(group)
        HStack(spacing: 8) {
            GroupLabel(field: group.field, key: group.key).font(.system(size: 12, weight: .semibold))
            Text(total.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3)
            Spacer()
            if group.field == .status { SortStartButton(status: group.key) }
            AcceptAllButton(threads: group.all)
        }
        .foregroundStyle(Theme.text.opacity(0.85))
        .padding(.horizontal, 16).padding(.vertical, 7)
        .background(.ultraThinMaterial)
        .contentShape(Rectangle())
        // Clicking a group opens it: its status, project or account.
        .onTapGesture { model.openGroup(group.field, group.key) }
        .help(model.canOpenGroup(group.field, group.key) ? "Open" : "")
    }
}

struct SubGroupHeader: View {
    let group: ThreadGroup

    var body: some View {
        HStack(spacing: 6) {
            GroupLabel(field: group.field, key: group.key)
            Text("\(group.threads.count)").foregroundStyle(Theme.text3)
            Spacer()
        }
        .font(.system(size: 11.5, weight: .medium))
        .foregroundStyle(Theme.text2)
        .padding(.horizontal, 24).padding(.top, 8).padding(.bottom, 4)
    }
}

/// "Accept n": applies every pending AI suggestion in a group at once.
struct AcceptAllButton: View {
    @Environment(AppModel.self) private var model
    let threads: [ThreadSummary]

    var body: some View {
        let ids = threads.filter { $0.suggestion.map { !$0.isEmpty } ?? false }.map(\.id)
        if !ids.isEmpty {
            Button { model.acceptSuggestions(ids) } label: {
                Label("Accept \(ids.count)", systemImage: "checkmark.circle").font(.system(size: 11.5))
            }
            .buttonStyle(.plain)
            .foregroundStyle(Theme.accent)
            .help("Apply the AI's suggestion to these \(ids.count) threads")
        }
    }
}

/// Linear-style Display menu: grouping, sub-grouping and ordering.
struct DisplayMenu: View {
    @Environment(AppModel.self) private var model
    /// Icon only, the label in the tooltip, when the list is narrow.
    var compact = false

    var body: some View {
        Menu {
            Picker("Grouping", selection: binding(\.grouping)) {
                ForEach(Grouping.allCases, id: \.self) { Text($0.label).tag($0) }
            }
            Picker("Sub-grouping", selection: binding(\.subGrouping)) {
                ForEach(Grouping.allCases, id: \.self) { Text($0 == .none ? "None" : $0.label).tag($0) }
            }
            Picker("Ordering", selection: binding(\.ordering)) {
                ForEach(Ordering.allCases, id: \.self) { Text($0.label).tag($0) }
            }
            Picker("Direction", selection: binding(\.reversed)) {
                Text(model.display.ordering.directions.0).tag(false)
                Text(model.display.ordering.directions.1).tag(true)
            }
        } label: {
            if compact {
                Image(systemName: "slider.horizontal.3").font(.system(size: 12)).accessibilityLabel("Display")
            } else {
                Label("Display", systemImage: "slider.horizontal.3").font(.system(size: 12))
            }
        }
        .menuStyle(.button).fixedSize()
        .help(compact ? "Display: grouping, sub-grouping, ordering and direction" : "Grouping, sub-grouping, ordering and direction")
    }

    private func binding<T>(_ kp: WritableKeyPath<DisplayOptions, T>) -> Binding<T> {
        Binding(get: { model.display[keyPath: kp] }, set: { v in
            var d = model.display
            d[keyPath: kp] = v
            model.display = d
        })
    }
}

/// Keeps the selected row in view. It alone observes the selected id, so a j/k step
/// does not redraw the list around it.
struct SelectionScroller: View {
    @Environment(AppModel.self) private var model
    let proxy: ScrollViewProxy

    var body: some View {
        Color.clear
            .onChange(of: model.selectedThreadId) { _, id in
                // Only to reveal a row not fully shown, and without animation: a scroll
                // lays the list out again, an animated one on every frame of it.
                if let id, let anchor = model.rowAnchor(for: id), !model.visibleRows.contains(anchor) { proxy.scrollTo(anchor) }
            }
    }
}

struct ThreadRow: View, Equatable {
    @Environment(AppModel.self) private var model
    let thread: ThreadSummary
    /// The row's own selection flag (nil: never selected, as in People), so stepping
    /// redraws only the rows it leaves and reaches.
    var selection: SelectionFlag?

    nonisolated static func == (a: ThreadRow, b: ThreadRow) -> Bool {
        a.thread == b.thread && a.selection === b.selection
    }

    var body: some View {
        let selected = selection?.on ?? false
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                // Unread is carried by a dot plus weight; read rows stay quiet so unread ones stand out.
                Circle().fill(thread.unread ? Theme.accent : .clear).frame(width: 7, height: 7)
                    .accessibilityLabel(thread.unread ? "Unread" : "")
                Text(thread.number).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.text3)
                Text(thread.lastFrom).font(.system(size: 13, weight: thread.unread ? .semibold : .regular)).lineLimit(1)
                if model.accounts.count > 1 {
                    // Hollow ring = which account; kept distinct from the filled unread dot.
                    Circle().strokeBorder(Theme.accountColor(thread.accountId, among: model.accounts), lineWidth: 1.5)
                        .frame(width: 7, height: 7).help(model.accounts.first { $0.id == thread.accountId }?.email ?? "")
                }
                if thread.hasAttachments {
                    Image(systemName: "paperclip").font(.system(size: 10)).foregroundStyle(Theme.text3)
                }
                ThreadCountPill(thread: thread)
                Spacer(minLength: 4)
                if let sg = thread.suggestion, !sg.isEmpty { SuggestionChip(suggestion: sg, threadId: thread.id) }
                ProjectChips(names: thread.projects)
                if let due = thread.due { Chip(small: true) { Text("Due \(Format.due(due))") } }
                Text(Format.short(thread.lastDate)).font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            Text(thread.subject.isEmpty ? "(no subject)" : thread.subject)
                .font(.system(size: 13, weight: thread.unread ? .semibold : .regular))
                .foregroundStyle(thread.unread ? Theme.text : Theme.text2)
                .lineLimit(1)
            if let next = thread.nextAction, !next.isEmpty {
                Label(next, systemImage: "sparkle").font(.system(size: 12)).foregroundStyle(Theme.text2).lineLimit(1)
            } else {
                Text(thread.snippet).font(.system(size: 12)).foregroundStyle(Theme.text2).lineLimit(1)
            }
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 16).padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(selected ? Theme.accent.opacity(0.11) : .clear)
        .overlay(alignment: .leading) {
            if selected { Rectangle().fill(Theme.accent).frame(width: 3) }
        }
        .overlay(alignment: .top) { Rectangle().fill(Theme.line.opacity(0.8)).frame(height: 1) }
        .contentShape(Rectangle())
        .draggable(model.dragPayload(thread.id))
        .contextMenu { ConversationMenu() }
    }
}

/// "3 threads": this row reads several Gmail threads as one conversation.
struct ThreadCountPill: View {
    let thread: ThreadSummary

    var body: some View {
        if thread.groupsThreads, let n = thread.conversationThreads {
            Text("\(n) threads")
                .font(.system(size: 10.5, weight: .medium)).foregroundStyle(Theme.accent)
                .padding(.horizontal, 6).padding(.vertical, 1)
                .background(Theme.accentSoft, in: Capsule())
                .help("\(n) emails on the same subject with the same people, read as one conversation")
        }
    }
}

/// Right-click on a row or card: join the selected conversations.
struct ConversationMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Button("Join into one conversation (⇧M)") { model.mergeSelected() }
            .disabled(model.targetIds.count < 2)
    }
}

extension NSResponder {
    /// Text is being typed here: a field or the composer, not a read-only bubble.
    var isTyping: Bool { (self as? NSText)?.isEditable ?? false }
}

/// List / Board / Feed. Words when there is room; icons with a tooltip each when the
/// list is narrow.
struct LayoutSwitch: View {
    @Environment(AppModel.self) private var model
    let compact: Bool

    var body: some View {
        @Bindable var model = model
        Picker("", selection: $model.layout) {
            ForEach(LayoutMode.allCases, id: \.self) { mode in
                Group {
                    if compact {
                        Image(systemName: mode.symbol).help(mode.rawValue).accessibilityLabel(mode.rawValue)
                    } else {
                        Text(mode.rawValue)
                    }
                }
                .tag(mode)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(width: compact ? 96 : 170)
        .help(compact ? "List, Board or Feed" : "")
    }
}
