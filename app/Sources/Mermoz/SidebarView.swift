/// Glass sidebar: search/ask, workflow statuses, projects, accounts, sync state.
import MermozKit
import SwiftUI

struct SidebarView: View {
    @Environment(AppModel.self) private var model
    @State private var addingProject = false
    @State private var newProject = ""
    @AppStorage("sidebar.labelsOpen") private var labelsOpen = true

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Button { model.paletteMode = .search } label: {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").font(.system(size: 12, weight: .semibold))
                    Text("Search or ask")
                    Spacer()
                    KeyHint("⌘K")
                }
                .foregroundStyle(Theme.text2)
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(Theme.accent.opacity(0.07), in: RoundedRectangle(cornerRadius: 9))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    section("Workflow") {
                        ForEach(model.statuses.filter { $0.key != "canceled" }) { s in
                            row(.status(s.key), count: model.key(s)) {
                                StatusIcon(key: s.key, color: Color(hex: s.color))
                                Text(s.name)
                            }
                        }
                    }
                    section("Projects") {
                        ForEach(model.projects) { p in
                            row(.project(p.id), count: model.counts.project[p.id]) {
                                RoundedRectangle(cornerRadius: 3).fill(Color(hex: p.color)).frame(width: 9, height: 9)
                                Text(p.name)
                            }
                        }
                        if addingProject {
                            TextField("Project name", text: $newProject)
                                .textFieldStyle(.plain)
                                .padding(.horizontal, 8).frame(height: 28)
                                .onSubmit {
                                    let name = newProject.trimmingCharacters(in: .whitespaces)
                                    addingProject = false
                                    newProject = ""
                                    if !name.isEmpty { Task { _ = await model.createProject(name) } }
                                }
                        } else {
                            Button { addingProject = true } label: {
                                Label("New project", systemImage: "plus").foregroundStyle(Theme.text3)
                                    .padding(.horizontal, 8).frame(height: 26)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    if !model.labels.isEmpty {
                        VStack(alignment: .leading, spacing: 1) {
                            Button { labelsOpen.toggle() } label: {
                                HStack(spacing: 4) {
                                    Text("LABELS").font(.system(size: 11, weight: .semibold)).kerning(0.6)
                                    Image(systemName: labelsOpen ? "chevron.down" : "chevron.right").font(.system(size: 8, weight: .bold))
                                    Spacer()
                                }
                                .foregroundStyle(Theme.text3)
                                .padding(.horizontal, 8).padding(.bottom, 4)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            if labelsOpen {
                                ForEach(model.labels) { l in
                                    row(.label(l.accountId, l.id), count: nil) {
                                        Image(systemName: "folder").font(.system(size: 11)).foregroundStyle(Theme.text3)
                                        Text(l.leaf).lineLimit(1)
                                        if model.accounts.count > 1 {
                                            Circle().strokeBorder(Theme.accountColor(l.accountId, among: model.accounts), lineWidth: 1.5)
                                                .frame(width: 7, height: 7)
                                        }
                                    }
                                    .padding(.leading, CGFloat(l.depth) * 14)
                                    .help(l.name)
                                }
                            }
                        }
                    }
                    section("Accounts") {
                        ForEach(model.accounts) { a in
                            row(.account(a.id), count: model.counts.account[a.id]) {
                                Circle().fill(Theme.accountColor(a.id, among: model.accounts)).frame(width: 8, height: 8)
                                Text(a.email).lineLimit(1).truncationMode(.middle)
                            }
                        }
                    }
                }
            }
            .scrollIndicators(.never)

            HStack(spacing: 7) {
                Circle().fill(model.syncLine.hasPrefix("Synced") ? Theme.ok : Theme.accent).frame(width: 7, height: 7)
                Text(model.syncLine).lineLimit(1)
            }
            .font(.system(size: 11.5))
            .foregroundStyle(Theme.text2)
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.accent.opacity(0.05), in: RoundedRectangle(cornerRadius: 10))
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 12)
        .frame(maxHeight: .infinity)
        .glassPanel()
    }

    private func section<C: View>(_ title: String, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(title.uppercased())
                .font(.system(size: 11, weight: .semibold)).kerning(0.6)
                .foregroundStyle(Theme.text3)
                .padding(.horizontal, 8).padding(.bottom, 4)
            content()
        }
    }

    private func row<L: View>(_ sel: SidebarSelection, count: Int?, @ViewBuilder label: () -> L) -> some View {
        let on = model.selection == sel
        return Button { model.select(sel) } label: {
            HStack(spacing: 9) {
                label()
                Spacer(minLength: 4)
                if let count, count > 0 {
                    Text(count.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3)
                }
            }
            .font(.system(size: 13, weight: on ? .semibold : .regular))
            .foregroundStyle(Theme.text)
            .padding(.horizontal, 8)
            .frame(height: 28)
            .background(on ? Theme.accent.opacity(0.13) : .clear, in: RoundedRectangle(cornerRadius: 8))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

extension AppModel {
    /// Sidebar badge: review counts drafts awaiting approval, others count threads.
    func key(_ s: Status) -> Int? {
        if s.key == "review" { return max(reviewDrafts.count, counts.status["review"] ?? 0) }
        if s.key == "done" { return nil }
        return counts.status[s.key]
    }
}

struct KeyHint: View {
    let text: String
    init(_ t: String) { text = t }

    var body: some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundStyle(Theme.text2)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background(Color.white.opacity(0.85), in: RoundedRectangle(cornerRadius: 5))
            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(Theme.line))
    }
}
