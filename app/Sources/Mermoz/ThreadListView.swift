/// Middle pane in list mode: threads grouped by priority, keyboard-driven.
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
                            ForEach(model.priorityGroups, id: \.priority) { g in
                                Section {
                                    ForEach(g.threads) { t in
                                        ThreadRow(thread: t, selected: model.selectedThreadId == t.id)
                                            .id(t.id)
                                            .onTapGesture { model.open(t.id); focused = true }
                                    }
                                } header: {
                                    GroupHeader(priority: g.priority, count: g.threads.count)
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
        .glassPanel()
        .focusable()
        .focused($focused)
        .focusEffectDisabled()
        .onAppear { focused = true }
        .onKeyPress(characters: .init(charactersIn: "jkesprc"), phases: .down) { press in
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
        }
    }
}

struct GroupHeader: View {
    let priority: Int
    let count: Int

    var body: some View {
        HStack(spacing: 8) {
            PriorityIcon(priority: priority)
            Text(Theme.priorityNames[priority] ?? "").font(.system(size: 12, weight: .semibold))
            Text("\(count)").font(.system(size: 12)).foregroundStyle(Theme.text3)
            Spacer()
        }
        .foregroundStyle(Theme.text.opacity(0.85))
        .padding(.horizontal, 16).padding(.vertical, 7)
        .background(.ultraThinMaterial)
    }
}

struct ThreadRow: View {
    @Environment(AppModel.self) private var model
    let thread: ThreadSummary
    let selected: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text(thread.number).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.text3)
                Text(thread.lastFrom).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                Circle().fill(Theme.accountColor(thread.accountId, among: model.accounts)).frame(width: 6, height: 6)
                if thread.hasAttachments {
                    Image(systemName: "paperclip").font(.system(size: 10)).foregroundStyle(Theme.text3)
                }
                Spacer(minLength: 4)
                if let due = thread.due { Chip(small: true) { Text("Due \(Format.due(due))") } }
                Text(Format.short(thread.lastDate)).font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            Text(thread.subject.isEmpty ? "(no subject)" : thread.subject)
                .font(.system(size: 13, weight: thread.unread ? .semibold : .regular))
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
