/// Glass sidebar: search/ask, workflow statuses, projects (Gmail labels), accounts, sync state.
import MermozKit
import SwiftUI

struct SidebarView: View {
    @Environment(AppModel.self) private var model
    @State private var addingProject = false
    @State private var newProject = ""
    @State private var renaming: String?
    @State private var renameText = ""
    @State private var deleting: Project?
    @AppStorage("sidebar.projectsOpen") private var projectsOpen = true

    private var projectNames: Set<String> { Set(model.projects.map(\.name)) }
    private var projectParents: Set<String> { ProjectTree.parents(among: model.projects.map(\.name)) }

    /// Folded projects, kept across launches (JSON array of names; names may hold commas).
    @AppStorage("sidebar.collapsedProjects") private var collapsedJSON = "[]"

    private var collapsed: Set<String> {
        Set((try? JSONDecoder().decode([String].self, from: Data(collapsedJSON.utf8))) ?? [])
    }

    private var visibleProjects: [Project] {
        let shown = Set(ProjectTree.visible(model.projects.map(\.name), collapsed: collapsed))
        return model.projects.filter { shown.contains($0.name) }
    }

    private func toggleFold(_ name: String) {
        var c = collapsed
        if c.contains(name) { c.remove(name) } else { c.insert(name) }
        if let d = try? JSONEncoder().encode(c.sorted()) { collapsedJSON = String(decoding: d, as: UTF8.self) }
    }

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
                        ForEach(model.statuses) { s in
                            StatusRow(status: s, count: model.key(s))
                        }
                    }
                    section("Directory") {
                        row(.people, count: nil) {
                            Image(systemName: "person.2").font(.system(size: 11)).foregroundStyle(Theme.text2)
                            Text("People")
                        }
                    }
                    VStack(alignment: .leading, spacing: 1) {
                        // Projects are the user's Gmail labels, nested with "/".
                        Button { projectsOpen.toggle() } label: {
                            HStack(spacing: 4) {
                                Text("PROJECTS").font(.system(size: 11, weight: .semibold)).kerning(0.6)
                                Image(systemName: projectsOpen ? "chevron.down" : "chevron.right").font(.system(size: 8, weight: .bold))
                                Spacer()
                            }
                            .foregroundStyle(Theme.text3)
                            .padding(.horizontal, 8).padding(.bottom, 4)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        if projectsOpen {
                            ForEach(visibleProjects) { p in
                                if renaming == p.id {
                                    TextField("Project name", text: $renameText)
                                        .textFieldStyle(.plain)
                                        .padding(.horizontal, 8).frame(height: 28)
                                        .onSubmit { model.renameProject(p.id, to: renameText); renaming = nil }
                                        .onExitCommand { renaming = nil }
                                } else {
                                    let place = ProjectTree.place(p.name, among: projectNames)
                                    HStack(spacing: 0) {
                                        // Fold arrow on projects with sub-projects; same width elsewhere to keep rows aligned.
                                        if projectParents.contains(p.name) {
                                            Button { toggleFold(p.name) } label: {
                                                Image(systemName: collapsed.contains(p.name) ? "chevron.right" : "chevron.down")
                                                    .font(.system(size: 8, weight: .bold)).foregroundStyle(Theme.text3)
                                                    .frame(width: 14, height: 28).contentShape(Rectangle())
                                            }
                                            .buttonStyle(.plain)
                                            .accessibilityLabel(collapsed.contains(p.name) ? "Show sub-projects" : "Hide sub-projects")
                                        } else {
                                            Color.clear.frame(width: 14, height: 28)
                                        }
                                        row(.project(p.id), count: model.counts.project[p.id]) {
                                            RoundedRectangle(cornerRadius: 3).fill(Color(hex: p.color)).frame(width: 9, height: 9)
                                            Text(place.label).lineLimit(1)
                                        }
                                    }
                                    .padding(.leading, CGFloat(place.depth) * 14)
                                    .modifier(DropIntoProject(project: p.id))
                                    .help(p.name)
                                    .contextMenu {
                                        Button("Rename…") { renameText = p.name; renaming = p.id }
                                        Menu("Color") {
                                            ForEach(Theme.projectColors, id: \.self) { c in
                                                Button(Theme.colorName(c)) { model.recolorProject(p.id, to: c) }
                                            }
                                        }
                                        Divider()
                                        Button("Delete project…", role: .destructive) { deleting = p }
                                    }
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
        .confirmationDialog("Delete “\(deleting?.name ?? "")”?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } })) {
            Button("Delete project", role: .destructive) {
                if let p = deleting { model.deleteProject(p.id) }
                deleting = nil
            }
        } message: {
            Text("This deletes the Gmail label “\(deleting?.name ?? "")” on every account. The mail itself stays.")
        }
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
        if closedStatusKeys.contains(s.key) { return nil }
        return counts.status[s.key]
    }
}

/// A status in the sidebar: click to open, drag to reorder, drop thread cards on it.
struct StatusRow: View {
    @Environment(AppModel.self) private var model
    let status: Status
    let count: Int?
    @State private var targeted = false

    var body: some View {
        let on = model.selection == .status(status.key)
        HStack(spacing: 9) {
            StatusIcon(key: status.key, color: Color(hex: status.color))
            Text(status.name)
            Spacer(minLength: 4)
            if let count, count > 0 {
                Text(count.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
        }
        .font(.system(size: 13, weight: on ? .semibold : .regular))
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 8)
        .frame(height: 28)
        .background(on || targeted ? Theme.accent.opacity(targeted ? 0.2 : 0.13) : .clear, in: RoundedRectangle(cornerRadius: 8))
        .contentShape(Rectangle())
        .onTapGesture { model.select(.status(status.key)) }
        .accessibilityAddTraits(.isButton)
        // Drag a status onto another to reorder; drop thread cards to move them here.
        .draggable("status:\(status.key)") {
            HStack(spacing: 6) { StatusIcon(key: status.key, color: Color(hex: status.color)); Text(status.name) }
                .padding(6).background(.white, in: RoundedRectangle(cornerRadius: 6))
        }
        .dropDestination(for: String.self) { items, _ in
            for item in items {
                if item.hasPrefix("status:") {
                    model.moveStatus(String(item.dropFirst(7)), before: status.key)
                } else {
                    // A multi-selection drag carries its ids one per line.
                    model.update(item.split(separator: "\n").map(String.init), status: status.key)
                }
            }
            return !items.isEmpty
        } isTargeted: { targeted = $0 }
    }
}

/// Dropping thread cards or rows on a project adds them to it (its Gmail label).
struct DropIntoProject: ViewModifier {
    @Environment(AppModel.self) private var model
    let project: String
    @State private var targeted = false

    func body(content: Content) -> some View {
        content
            .background(targeted ? Theme.accent.opacity(0.2) : .clear, in: RoundedRectangle(cornerRadius: 8))
            .dropDestination(for: String.self) { items, _ in
                // A multi-selection drag carries its ids one per line; statuses are not threads.
                let ids = items.filter { !$0.hasPrefix("status:") }.flatMap { $0.split(separator: "\n").map(String.init) }
                guard !ids.isEmpty else { return false }
                model.update(ids, addProjects: [project])
                model.toast = ids.count == 1 ? "Added to \(project)" : "Added \(ids.count) threads to \(project)"
                return true
            } isTargeted: { targeted = $0 }
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
