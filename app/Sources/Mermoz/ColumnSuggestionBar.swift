/// "✦ 312 suggestions · Accept all" on top of a status column, with one button per
/// destination ("→ Archive · Receipts 240") and a quiet sorting progress line.
import MermozKit
import SwiftUI

struct ColumnSuggestionBar: View {
    @Environment(AppModel.self) private var model
    let status: String

    var body: some View {
        if let c = model.columnSuggestions[status] {
            let progress = ColumnSortText.progress(c, sorting: model.sorting, paused: model.sortingPaused)
            if c.pending > 0 || progress != nil {
                VStack(alignment: .leading, spacing: 6) {
                    if c.pending > 0 { summary(c) }
                    if c.groups.count > 1 { groups(c) }
                    if let progress { progressLine(progress) }
                }
                .padding(.horizontal, 10).padding(.vertical, 7)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.accentSoft.opacity(0.45), in: RoundedRectangle(cornerRadius: 8))
            }
        }
    }

    private func summary(_ c: ColumnSuggestions) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "sparkle").font(.system(size: 10, weight: .semibold)).foregroundStyle(Theme.accent)
            Text(ColumnSortText.pending(c.pending)).font(.system(size: 12, weight: .medium))
            Spacer(minLength: 4)
            Button { model.acceptColumn(status) } label: {
                Text(c.groups.count == 1 ? "Accept → \(label(c.groups[0]))" : "Accept all").lineLimit(1)
            }
            .buttonStyle(.plain)
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(Theme.accent)
            .help("Move these \(c.pending.formatted()) threads where the AI suggests — ⌘Z to undo")
        }
    }

    /// One button per destination, largest first.
    private func groups(_ c: ColumnSuggestions) -> some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                ForEach(c.groups) { g in
                    Button { model.acceptColumn(status, group: g) } label: {
                        HStack(spacing: 4) {
                            Text("→ \(label(g))").lineLimit(1)
                            Text(g.count.formatted()).foregroundStyle(Theme.text3)
                        }
                        .font(.system(size: 11.5))
                        .padding(.horizontal, 7).frame(height: 20)
                        .background(Theme.paper, in: RoundedRectangle(cornerRadius: 5))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(Theme.accent)
                    .help("Accept these \(g.count.formatted()) — ⌘Z to undo")
                }
            }
        }
        .scrollIndicators(.never)
    }

    private func progressLine(_ text: String) -> some View {
        HStack(spacing: 6) {
            if model.sorting && model.sortingPaused == nil { ProgressView().controlSize(.mini) }
            Text(text).font(.system(size: 11)).foregroundStyle(Theme.text3)
        }
    }

    private func label(_ g: SuggestionGroup) -> String {
        g.label { key in model.statuses.first { $0.key == key }?.name ?? key }
    }
}
