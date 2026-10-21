/// Glass sidebar: search/ask, workflow statuses, projects (Gmail labels), accounts, sync state.
///
/// The sidebar's body reads no counts, selection or sync progress: each badge observes its
/// own count cell and each row its own selection, so reading an email redraws one badge.
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

    /// Folded projects, kept across launches (JSON array of names; names may hold commas).
    @AppStorage("sidebar.collapsedProjects") private var collapsedJSON = "[]"

    private var collapsed: Set<String> {
        Set((try? JSONDecoder().decode([String].self, from: Data(collapsedJSON.utf8))) ?? [])
    }

    private func toggleFold(_ name: String) {
        var c = collapsed
        if c.contains(name) { c.remove(name) } else { c.insert(name) }
        if let d = try? JSONEncoder().encode(c.sorted()) { collapsedJSON = String(decoding: d, as: UTF8.self) }
    }

    var body: some View {
        // Computed once per body, not once per project row.
        let names = model.projects.map(\.name)
        let projectNames = Set(names)
        let projectParents = ProjectTree.parents(among: names)
        let collapsed = collapsed
        let shown = Set(ProjectTree.visible(names, collapsed: collapsed))
        let visibleProjects = model.projects.filter { shown.contains($0.name) }
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
                            StatusRow(status: s, count: closedStatusKeys.contains(s.key) ? nil
                                : model.countCells.cell(s.key == "review" ? .reviewBadge : .status(s.key)))
                        }
                    }
                    section("Browse") {
                        SidebarRow(sel: .people, title: "People", icon: .symbol("person.2", Theme.text2))
                        SidebarRow(sel: .docs, title: "Docs", icon: .symbol("doc.on.doc", Theme.text2))
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
                                        SidebarRow(sel: .project(p.id), title: place.label, icon: .square(Color(hex: p.color)),
                                                   count: model.countCells.cell(.project(p.id)))
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
                            AccountSidebarRow(account: a, color: Theme.accountColor(a.id, among: model.accounts),
                                              count: model.countCells.cell(.account(a.id)))
                        }
                    }
                }
            }
            .scrollIndicators(.never)

            SyncFooter()
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
}

/// A status in the sidebar: click to open, drag to reorder, drop thread cards on it.
struct StatusRow: View {
    @Environment(AppModel.self) private var model
    let status: Status
    /// Nil for closed statuses, which show no badge.
    let count: CountCell?
    @State private var targeted = false

    var body: some View {
        let on = model.selection == .status(status.key)
        HStack(spacing: 9) {
            SidebarIconSlot { StatusIcon(key: status.key, color: Color(hex: status.color)) }
            Text(status.name)
            Spacer(minLength: 4)
            if let count { CountBadge(cell: count) }
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
        .draggable(model.statusDragPayload(status.key)) {
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

/// A count on a sidebar row; it observes only its own cell and never animates.
struct CountBadge: View {
    let cell: CountCell

    var body: some View {
        if let n = cell.value, n > 0 {
            // Tabular digits, so counts in a column line up on their right edge.
            Text(n.formatted()).font(.system(size: 12)).monospacedDigit().foregroundStyle(Theme.text3)
                .contentTransition(.identity)
                .transaction { $0.animation = nil }
        }
    }
}

/// What sits before a sidebar row's title.
enum SidebarIcon: Equatable {
    case symbol(String, Color)
    case dot(Color)
    case square(Color)
}

/// A plain sidebar row. Inputs are plain values (no closures) so SwiftUI can tell an
/// unchanged row apart and skip it; the highlight reads the selection here, not in the list.
struct SidebarRow: View, Equatable {
    @Environment(AppModel.self) private var model
    let sel: SidebarSelection
    let title: String
    let icon: SidebarIcon
    var count: CountCell?
    /// Emails keep both ends readable; project names keep their start.
    var middle = false

    nonisolated static func == (a: SidebarRow, b: SidebarRow) -> Bool {
        a.sel == b.sel && a.title == b.title && a.icon == b.icon && a.count === b.count && a.middle == b.middle
    }

    var body: some View {
        let on = model.selection == sel
        Button { model.select(sel) } label: {
            HStack(spacing: 9) {
                // One box width for every icon, so every label starts at the same x.
                SidebarIconSlot {
                    switch icon {
                    case let .symbol(name, color): Image(systemName: name).font(.system(size: 11)).foregroundStyle(color)
                    case let .dot(color): Circle().fill(color).frame(width: 8, height: 8)
                    case let .square(color): RoundedRectangle(cornerRadius: 3).fill(color).frame(width: 9, height: 9)
                    }
                }
                Text(title).lineLimit(1).truncationMode(middle ? .middle : .tail)
                Spacer(minLength: 4)
                if let count { CountBadge(cell: count) }
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

/// An account in the sidebar, with an eye to hide its mail from the window (for screenshots
/// and demos). The eye shows on hover, and stays while the account is hidden; a hidden
/// account's row is dimmed and opens nothing until shown again.
struct AccountSidebarRow: View {
    @Environment(AppModel.self) private var model
    let account: Account
    let color: Color
    let count: CountCell
    @State private var hovering = false

    var body: some View {
        let hidden = account.isHidden
        HStack(spacing: 0) {
            if hidden {
                HStack(spacing: 9) {
                    SidebarIconSlot { Circle().fill(color).frame(width: 8, height: 8) }
                    Text(account.email).lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 4)
                }
                .font(.system(size: 13))
                .foregroundStyle(Theme.text)
                .padding(.horizontal, 8)
                .frame(height: 28)
                .opacity(0.4)
                .help("Hidden: its mail stays out of every list until you show it again")
            } else {
                SidebarRow(sel: .account(account.id), title: account.email, icon: .dot(color),
                           count: hovering || account.mustSignInAgain ? nil : count, middle: true)
                    .help(account.serviceName.map { "\($0) account" } ?? "")
            }
            if account.mustSignInAgain {
                Button(SignInNotice.button) { model.reconnectAccount(account.id) }
                    .buttonStyle(.link).font(.system(size: 11.5, weight: .semibold))
                    .foregroundStyle(Theme.danger)
                    .fixedSize()
                    .padding(.trailing, 4)
                    .help("\(SignInNotice.who(for: account)) asked you to sign in again. New mail arrives once you do.")
            }
            if hidden || hovering {
                Button { model.setAccountHidden(account.id, !hidden) } label: {
                    Image(systemName: hidden ? "eye.slash" : "eye")
                        .font(.system(size: 11))
                        .foregroundStyle(hidden ? Theme.text2 : Theme.text3)
                        .frame(width: 22, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(hidden ? "Show this account's mail" : "Hide this account's mail")
                .accessibilityLabel(hidden ? "Show \(account.email)" : "Hide \(account.email)")
            }
        }
        .onHover { hovering = $0 }
    }
}

/// Sync state at the foot of the sidebar; download progress redraws only this line.
struct SyncFooter: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let line = model.syncLine
        HStack(spacing: 7) {
            Circle().fill(line.hasPrefix("Synced") ? Theme.ok : Theme.accent).frame(width: 7, height: 7)
            Text(line).lineLimit(1)
        }
        .font(.system(size: 11.5))
        .foregroundStyle(Theme.text2)
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.accent.opacity(0.05), in: RoundedRectangle(cornerRadius: 10))
    }
}
