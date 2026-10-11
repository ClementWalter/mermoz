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
            if model.threads.isEmpty {
                VStack(spacing: 6) {
                    Image(systemName: "checkmark.circle").font(.system(size: 24, weight: .light))
                    Text("Nothing here").font(.system(size: 13))
                }
                .foregroundStyle(Theme.text3)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 0, pinnedViews: [.sectionHeaders]) {
                            ForEach(model.groups) { g in
                                Section {
                                    if g.subgroups.isEmpty {
                                        rows(g.threads)
                                    } else {
                                        ForEach(g.subgroups) { sub in
                                            SubGroupHeader(group: sub)
                                            rows(sub.threads)
                                        }
                                    }
                                } header: {
                                    if g.field != .none { GroupHeaderView(group: g, total: model.groupTotal(g)) }
                                }
                            }
                            if model.totalThreads > model.threads.count {
                                Text("Showing \(model.threads.count) of \(model.totalThreads.formatted())")
                                    .font(.system(size: 11.5)).foregroundStyle(Theme.text3).padding(12)
                            }
                        }
                    }
                    .onChange(of: model.selectedThreadId) { _, id in
                        if let id { withAnimation(.easeOut(duration: 0.1)) { proxy.scrollTo(id) } }
                    }
                }
            }
        }
        .listPanel()
        .focusable()
        .focused($focused)
        .focusEffectDisabled()
        .onAppear { focused = true }
        .onKeyPress(characters: .init(charactersIn: "jkesprc?/"), phases: .down) { press in
            ThreadKeys.handle(press.characters, model: model) ? .handled : .ignored
        }
        .onKeyPress(.downArrow) { model.moveSelection(1); return .handled }
        .onKeyPress(.upArrow) { model.moveSelection(-1); return .handled }
        .onKeyPress(.return) {
            if model.selectedThreadId == nil { model.moveSelection(1) }
            return .handled
        }
    }
}

extension ThreadListView {
    private func rows(_ threads: [ThreadSummary]) -> some View {
        ForEach(threads) { t in
            ThreadRow(thread: t, selected: model.selectedThreadId == t.id)
                .id(t.id)
                .onTapGesture { model.open(t.id); focused = true }
        }
    }
}

/// Single-key shortcuts shared by list and board.
enum ThreadKeys {
    @MainActor
    static func handle(_ c: String, model: AppModel) -> Bool {
        switch c {
        case "j": model.moveSelection(1)
        case "k": model.moveSelection(-1)
        case "e": model.markDoneSelected()
        case "s": if model.selectedThreadId != nil { model.paletteMode = .status }
        case "p": if model.selectedThreadId != nil { model.paletteMode = .priority }
        case "r": model.startReply()
        case "c": model.startNew()
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
        @Bindable var model = model
        HStack(spacing: 10) {
            title
            Text(model.isReview ? "\(model.reviewDrafts.count)" : model.totalThreads.formatted())
                .font(.system(size: 12)).foregroundStyle(Theme.text3)
            Spacer()
            if !model.isReview {
                DisplayMenu()
                Picker("", selection: $model.layout) {
                    ForEach(LayoutMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 120)
                .onChange(of: model.layout) { Task { await model.loadList() } }
            }
            Button { model.startNew() } label: { Image(systemName: "square.and.pencil") }
                .buttonStyle(.plain)
                .foregroundStyle(Theme.text2)
                .help("New message (c)")
                .accessibilityLabel("New message")
        }
        .padding(.leading, 16).padding(.trailing, 12)
        .frame(height: 50)
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
    let total: Int

    var body: some View {
        HStack(spacing: 8) {
            GroupLabel(field: group.field, key: group.key).font(.system(size: 12, weight: .semibold))
            Text(total.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3)
            Spacer()
            if group.field == .status { AITriageButton(status: group.key) }
        }
        .foregroundStyle(Theme.text.opacity(0.85))
        .padding(.horizontal, 16).padding(.vertical, 7)
        .background(.ultraThinMaterial)
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

/// ✦ on a status: the AI sorts the newest threads of that status and moves them.
struct AITriageButton: View {
    @Environment(AppModel.self) private var model
    let status: String

    var body: some View {
        let busy = model.aiBusy.contains("triage:\(status)")
        Button { model.aiTriage(status) } label: {
            if busy { ProgressView().controlSize(.mini) } else { Image(systemName: "sparkles") }
        }
        .buttonStyle(.plain)
        .foregroundStyle(Theme.accent)
        .disabled(busy)
        .help("Let AI triage this column and move threads where they belong")
        .accessibilityLabel("AI triage")
    }
}

/// Linear-style Display menu: grouping, sub-grouping and ordering.
struct DisplayMenu: View {
    @Environment(AppModel.self) private var model

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
        } label: {
            Label("Display", systemImage: "slider.horizontal.3").font(.system(size: 12))
        }
        .menuStyle(.button).fixedSize()
        .help("Grouping, sub-grouping and ordering")
    }

    private func binding<T>(_ kp: WritableKeyPath<DisplayOptions, T>) -> Binding<T> {
        Binding(get: { model.display[keyPath: kp] }, set: { v in
            var d = model.display
            d[keyPath: kp] = v
            model.display = d
        })
    }
}

struct ThreadRow: View {
    @Environment(AppModel.self) private var model
    let thread: ThreadSummary
    let selected: Bool

    var body: some View {
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
                Spacer(minLength: 4)
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
        .draggable(thread.id)
    }
}
