/// Board mode: one column per open status; dropping a card changes its status.
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
                Picker("", selection: $model.layout) {
                    ForEach(LayoutMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden().frame(width: 120)
                .onChange(of: model.layout) { Task { await model.loadList() } }
            }
            .padding(.horizontal, 16)
            .frame(height: 50)
            .glassPanel()

            ScrollView(.horizontal) {
                HStack(alignment: .top, spacing: 10) {
                    ForEach(model.openStatuses) { s in
                        BoardColumn(status: s, threads: model.threads.filter { $0.status == s.key })
                    }
                    DoneStrip()
                }
                .padding(.bottom, 2)
            }
            .scrollIndicators(.automatic)
        }
        .focusable()
        .focused($focused)
        .focusEffectDisabled()
        .onAppear { focused = true }
        .onKeyPress(characters: .init(charactersIn: "jkesprc"), phases: .down) { press in
            ThreadKeys.handle(press.characters, model: model) ? .handled : .ignored
        }
    }

    private var title: String {
        switch model.selection {
        case .status: "All open"
        case let .project(id): model.project(id)?.name ?? "Project"
        case let .account(id): model.accounts.first { $0.id == id }?.email ?? "Account"
        }
    }
}

struct BoardColumn: View {
    @Environment(AppModel.self) private var model
    let status: Status
    let threads: [ThreadSummary]
    @State private var targeted = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                StatusIcon(key: status.key, color: Color(hex: status.color))
                Text(status.name).font(.system(size: 12.5, weight: .semibold))
                Text("\(threads.count)").font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            .padding(.horizontal, 4).padding(.top, 2)
            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(threads) { t in
                        BoardCard(thread: t, selected: model.selectedThreadId == t.id)
                            .onTapGesture { model.open(t.id) }
                    }
                }
            }
            .scrollIndicators(.never)
        }
        .padding(10)
        .frame(width: 250)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(Color.white.opacity(targeted ? 0.6 : 0.32), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(targeted ? Theme.accent.opacity(0.5) : Color.white.opacity(0.8)))
        .dropDestination(for: String.self) { ids, _ in
            model.update(ids, status: status.key)
            return true
        } isTargeted: { targeted = $0 }
    }
}

struct BoardCard: View {
    @Environment(AppModel.self) private var model
    let thread: ThreadSummary
    let selected: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 6) {
                if thread.priority > 0 { PriorityIcon(priority: thread.priority) }
                Text(thread.number).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.text3)
                Spacer()
                if let due = thread.due { Chip(small: true) { Text("Due \(Format.due(due))") } } else {
                    Text(Format.short(thread.lastDate)).font(.system(size: 11)).foregroundStyle(Theme.text3)
                }
            }
            Text(thread.subject.isEmpty ? "(no subject)" : thread.subject)
                .font(.system(size: 12.5, weight: .semibold)).lineLimit(2)
            HStack(spacing: 6) {
                Circle().fill(Theme.accountColor(thread.accountId, among: model.accounts)).frame(width: 6, height: 6)
                Text(thread.lastFrom).lineLimit(1)
                if let p = model.project(thread.projectId) {
                    Chip(small: true) {
                        RoundedRectangle(cornerRadius: 2).fill(Color(hex: p.color)).frame(width: 7, height: 7)
                        Text(p.name)
                    }
                }
            }
            .font(.system(size: 11.5)).foregroundStyle(Theme.text2)
            if let next = thread.nextAction, !next.isEmpty {
                Label(next, systemImage: "sparkle").font(.system(size: 11.5)).foregroundStyle(Theme.text2).lineLimit(2)
            }
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 11).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.9), in: RoundedRectangle(cornerRadius: 11))
        .overlay(RoundedRectangle(cornerRadius: 11).strokeBorder(selected ? Theme.accent : Color.white, lineWidth: selected ? 2 : 1))
        .shadow(color: Theme.accent.opacity(0.06), radius: 8, y: 3)
        .contentShape(Rectangle())
        .draggable(thread.id)
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
        .dropDestination(for: String.self) { ids, _ in
            model.update(ids, status: "done")
            return true
        } isTargeted: { targeted = $0 }
    }
}
