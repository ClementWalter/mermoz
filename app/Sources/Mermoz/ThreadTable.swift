/// The List's rows in an AppKit table: only rows on screen exist, rows are reused while
/// scrolling, heights are measured once per kind of row, and each row is the same SwiftUI
/// view the list always drew (click with ⇧/⌘, drag to the sidebar or a Board column,
/// context menu, the ✦ suggestion chip). A j/k step redraws the two rows it moves
/// between and scrolls, in the same update, only when the selected row is not fully
/// shown; nothing else in the window is laid out again.
///
/// The keyboard stays with the SwiftUI list around the table (↑/↓, ⇧↑/⇧↓, ⌫, Enter) and
/// with the window-wide shortcuts (j/k and the rest).
import AppKit
import MermozKit
import SwiftUI

struct ThreadTable: NSViewRepresentable, Equatable {
    @Environment(AppModel.self) private var model
    let groups: [ThreadGroup]
    let loaded: Int
    let total: Int
    /// Gives the keyboard back to the list after a click.
    let refocus: () -> Void

    nonisolated static func == (a: ThreadTable, b: ThreadTable) -> Bool {
        a.groups == b.groups && a.loaded == b.loaded && a.total == b.total
    }

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }

    func makeNSView(context: Context) -> NSScrollView {
        let c = context.coordinator
        c.refocus = refocus
        let scroll = c.makeScrollView()
        c.apply(ListItems.flatten(groups, loaded: loaded, total: total))
        c.revealSelection()
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let c = context.coordinator
        c.model = model
        c.refocus = refocus
        c.apply(ListItems.flatten(groups, loaded: loaded, total: total))
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.detach()
    }

    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        var model: AppModel {
            didSet { if model !== oldValue { attach() } }
        }
        weak var table: NSTableView?
        var refocus: () -> Void = {}
        /// Asks for the next page; the "loading more" row calls it as it comes on screen.
        var loadMore: () -> Void
        /// The modifier keys of a click (the event's own when injected by a test).
        var modifiers: () -> NSEvent.ModifierFlags = { NSApp.currentEvent?.modifierFlags ?? NSEvent.modifierFlags }
        private(set) var items: [ListItem] = []
        /// Each kind of row's height, measured once from a real row.
        private var heights: [String: CGFloat] = [:]

        init(model: AppModel) {
            self.model = model
            loadMore = { [weak model] in model?.loadMore() }
            super.init()
            attach()
        }

        func makeScrollView() -> NSScrollView {
            let table = PassiveTableView()
            table.headerView = nil
            table.style = .plain
            table.intercellSpacing = .zero
            table.backgroundColor = .clear
            table.selectionHighlightStyle = .none
            table.gridStyleMask = []
            // Group headers stay pinned at the top while their threads scroll under them.
            table.floatsGroupRows = true
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("row"))
            column.resizingMask = .autoresizingMask
            table.addTableColumn(column)
            table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
            table.dataSource = self
            table.delegate = self
            let scroll = NSScrollView()
            scroll.documentView = table
            scroll.drawsBackground = false
            scroll.hasVerticalScroller = true
            scroll.autohidesScrollers = true
            self.table = table
            return scroll
        }

        /// The model calls back as the selection moves, before the frame is drawn.
        private func attach() {
            model.revealSelection = { [weak self] in self?.revealSelection() }
        }

        func detach() {
            model.revealSelection = nil
        }

        func apply(_ new: [ListItem]) {
            let change = ListItems.change(from: items, to: new)
            items = new
            guard let table else { return }
            switch change {
            case .none: break
            case let .rows(rows):
                table.reloadData(forRowIndexes: rows, columnIndexes: [0])
                // A row that gained or lost a chip changes height.
                table.noteHeightOfRows(withIndexesChanged: rows)
            case .all:
                // A new list measures again (text size or accounts may have changed).
                heights.removeAll()
                verified.removeAll()
                table.reloadData()
            }
        }

        /// Scrolls just enough to show the selected thread's row whole, clear of a pinned
        /// group header.
        func revealSelection() {
            guard let table, let id = model.selectedThreadId, let row = ListItems.row(of: id, in: items),
                  let clip = table.enclosingScrollView?.contentView else { return }
            // Rows must have their final heights before one is placed.
            if table.numberOfRows != items.count { table.reloadData() }
            let rect = table.rect(ofRow: row)
            let header = items.first { if case .group = $0 { true } else { false } }
            let pinned = header.map { height(of: $0) } ?? 0
            let visible = clip.bounds
            var y = visible.minY
            if rect.minY < visible.minY + pinned {
                y = rect.minY - pinned
            } else if rect.maxY > visible.maxY {
                y = rect.maxY - visible.height
            } else {
                return
            }
            clip.scroll(to: CGPoint(x: visible.minX, y: max(0, y)))
            table.enclosingScrollView?.reflectScrolledClipView(clip)
        }

        // MARK: table

        func numberOfRows(in tableView: NSTableView) -> Int { items.count }

        func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool {
            if case .group = items[row] { true } else { false }
        }

        func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat { height(of: items[row]) }

        func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }

        func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
            let v = tableView.makeView(withIdentifier: PlainRowView.id, owner: nil) as? PlainRowView ?? PlainRowView()
            // The row is named; its SwiftUI content (the ✦ chip's ×) stays reachable inside.
            v.setAccessibilityLabel(Self.accessibilityLabel(items[row]))
            return v
        }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            let item = items[row]
            let cell = tableView.makeView(withIdentifier: HostCell.id, owner: nil) as? HostCell ?? HostCell()
            cell.show(view(for: item), model: model)
            if case .loadMore = item { loadMore() }
            if case let .thread(t, _) = item { verify(Self.shape(t, accounts: model.accounts.count), cell) }
            return cell
        }

        /// Shapes whose height was checked against a row on screen.
        private var verified: Set<String> = []

        /// A row measured before any like it was drawn can come out a point short (its
        /// SF Symbol not loaded yet). The first row of each shape on screen is measured
        /// again once drawn, and rows of that shape take its height if it differs.
        private func verify(_ shape: String, _ cell: HostCell) {
            guard let root = cell.host?.rootView, verified.insert(shape).inserted else { return }
            DispatchQueue.main.async { [weak self] in
                guard let self, let table = self.table else { return }
                // The cell's own host is pinned to the row; a fresh one reads the natural size.
                let h = ceil(NSHostingView(rootView: root).fittingSize.height)
                guard h > 0, h != self.heights[shape] else { return }
                self.heights[shape] = h
                let rows = IndexSet(self.items.indices.filter {
                    if case let .thread(t, _) = self.items[$0] { Self.shape(t, accounts: self.model.accounts.count) == shape } else { false }
                })
                table.noteHeightOfRows(withIndexesChanged: rows)
            }
        }

        /// What VoiceOver says for a row.
        static func accessibilityLabel(_ item: ListItem) -> String {
            switch item {
            case let .group(g), let .subgroup(g): "\(g.key), \(g.all.count)"
            case let .loadMore(loaded, total): "Loading more, \(loaded) of \(total)"
            case let .thread(t, _):
                [t.unread ? "Unread" : nil, t.lastFrom, t.subject.isEmpty ? "(no subject)" : t.subject,
                 t.suggestion.map { _ in "AI suggestion" }]
                    .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: ", ")
            }
        }

        private func view(for item: ListItem) -> AnyView {
            switch item {
            case let .group(g):
                return AnyView(GroupHeaderView(group: g))
            case let .subgroup(g):
                return AnyView(SubGroupHeader(group: g))
            case let .loadMore(loaded, total):
                return AnyView(LoadingMoreRow(loaded: loaded, total: total))
            case let .thread(t, _):
                return AnyView(ThreadRow(thread: t, selection: model.selectionCells.flag(t.id))
                    .equatable()
                    .onTapGesture { [weak self] in self?.click(t.id) })
            }
        }

        /// A click on a thread row: ⇧ extends to a range, ⌘ adds or removes one.
        func click(_ id: String) {
            let mods = modifiers()
            model.click(id, shift: mods.contains(.shift), command: mods.contains(.command))
            refocus()
        }

        /// A row's height, measured from the row drawn by SwiftUI once per kind of row: a
        /// thread row's lines never wrap, so only what it shows (chips, a next step, an
        /// empty snippet, a pill, an account ring, a paperclip) moves its height.
        func height(of item: ListItem) -> CGFloat {
            let kind: String = switch item {
            case .group: "group"
            case .subgroup: "subgroup"
            case .loadMore: "more"
            case let .thread(t, _): Self.shape(t, accounts: model.accounts.count)
            }
            if let h = heights[kind] { return h }
            let h: CGFloat = switch item {
            case let .group(g): Self.fitting(GroupHeaderView(group: g), model: model)
            case let .subgroup(g): Self.fitting(SubGroupHeader(group: g), model: model)
            case let .loadMore(loaded, total): Self.fitting(LoadingMoreRow(loaded: loaded, total: total), model: model)
            case let .thread(t, _): Self.fitting(ThreadRow(thread: t), model: model)
            }
            heights[kind] = h
            return h
        }

        /// What decides a thread row's height.
        static func shape(_ t: ThreadSummary, accounts: Int) -> String {
            let chips = !t.projects.isEmpty || t.due != nil || !(t.suggestion?.isEmpty ?? true)
            let next = !(t.nextAction?.isEmpty ?? true)
            let pill = t.groupsThreads && t.conversationThreads != nil
            return "t|\(chips)|\(next)|\(!next && t.snippet.isEmpty)|\(pill)|\(accounts > 1)|\(t.hasAttachments)"
        }

        static func fitting(_ view: some View, model: AppModel) -> CGFloat {
            ceil(NSHostingView(rootView: view.environment(model)).fittingSize.height)
        }
    }
}

/// "Loading more · 50 of 1,234": the last row while pages remain.
struct LoadingMoreRow: View {
    let loaded: Int
    let total: Int

    var body: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Loading more · \(loaded) of \(total.formatted())")
        }
        .font(.system(size: 11.5)).foregroundStyle(Theme.text3).padding(12)
        .frame(maxWidth: .infinity)
    }
}

/// A table that never takes the keyboard: the SwiftUI list around it keeps it, so ↑/↓,
/// ⌫ and Enter work as before.
final class PassiveTableView: NSTableView {
    override var acceptsFirstResponder: Bool { false }
}

/// A reusable cell holding one SwiftUI row.
final class HostCell: NSTableCellView {
    static let id = NSUserInterfaceItemIdentifier("host")
    private(set) var host: NSHostingView<AnyView>?

    func show(_ view: AnyView, model: AppModel) {
        let root = AnyView(view.environment(model))
        if let host {
            host.rootView = root
            return
        }
        let h = NSHostingView(rootView: root)
        // The table sets the row's size; the row never asks the window to resize.
        h.sizingOptions = []
        h.translatesAutoresizingMaskIntoConstraints = false
        addSubview(h)
        NSLayoutConstraint.activate([
            h.leadingAnchor.constraint(equalTo: leadingAnchor), h.trailingAnchor.constraint(equalTo: trailingAnchor),
            h.topAnchor.constraint(equalTo: topAnchor), h.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        host = h
        identifier = Self.id
    }
}

/// A row with no selection or group-row chrome of its own: the SwiftUI row draws both.
final class PlainRowView: NSTableRowView {
    static let id = NSUserInterfaceItemIdentifier("plain")

    override init(frame: NSRect) {
        super.init(frame: frame)
        identifier = Self.id
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func drawBackground(in dirtyRect: NSRect) {}
    override func drawSelection(in dirtyRect: NSRect) {}
    override func drawSeparator(in dirtyRect: NSRect) {}
}
