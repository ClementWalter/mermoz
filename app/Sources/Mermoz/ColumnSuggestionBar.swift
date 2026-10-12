/// "✦ 2,693 suggestions · Review" on top of a status column, one chip per destination
/// ("→ Archive 758") that opens a review of those threads, and a quiet sorting progress
/// line. Reviewing is the main path; "Accept all" stays as the quick one.
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
                    if c.pending > 0 {
                        summary(c)
                        chips(c)
                    }
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
            Button { model.startReview(status) } label: {
                HStack(spacing: 5) {
                    Image(systemName: "sparkle").font(.system(size: 10, weight: .semibold))
                    Text(ColumnSortText.pending(c.pending)).font(.system(size: 12, weight: .semibold))
                    Text("Review").font(.system(size: 12))
                    Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold))
                }
                .foregroundStyle(Theme.accent)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("See every suggested thread, grouped by where it would go")
            Spacer(minLength: 4)
            Button("Accept all") { model.acceptColumn(status) }
                .buttonStyle(.plain)
                .font(.system(size: 11.5))
                .foregroundStyle(Theme.text2)
                .help("Move all \(c.pending.formatted()) threads where the AI suggests, without reviewing — ⌘Z to undo")
        }
    }

    /// One chip per destination, largest first, wrapping onto more lines as needed.
    private func chips(_ c: ColumnSuggestions) -> some View {
        FlowLayout(spacing: 6) {
            ForEach(c.groups) { g in
                Button { model.startReview(status, group: g) } label: {
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
                .help("Review these \(g.count.formatted()) threads")
            }
        }
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

/// While reviewing: what is shown, how to decide, and the way out.
struct SuggestionReviewBar: View {
    @Environment(AppModel.self) private var model
    let review: SuggestionReview

    var body: some View {
        let count = model.totalThreads
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 8) {
                Image(systemName: "sparkle").font(.system(size: 10, weight: .semibold)).foregroundStyle(Theme.accent)
                Text(review.title(count: count, statusName: statusName))
                    .font(.system(size: 12.5, weight: .semibold)).lineLimit(1)
                Spacer(minLength: 4)
                if model.multiSelection.count > 1 {
                    Button("Accept \(model.multiSelection.count) selected") { model.acceptSelected() }
                        .buttonStyle(.glass).controlSize(.small)
                }
                if count > 0 {
                    Button(review.acceptLabel(count: count)) { model.acceptColumn(review.status, group: review.group) }
                        .buttonStyle(.glassProminent).tint(Theme.accent).controlSize(.small)
                        .help("Move them where the AI suggests — ⌘Z to undo")
                }
                Button { model.endReview() } label: { Image(systemName: "xmark").font(.system(size: 11, weight: .semibold)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.text2)
                    .help("Stop reviewing (Esc)")
                    .accessibilityLabel("Stop reviewing")
            }
            Text("y accept · x dismiss · ↑↓ next · Esc done")
                .font(.system(size: 11)).foregroundStyle(Theme.text3)
        }
        .padding(.horizontal, 10).padding(.vertical, 7)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.accentSoft.opacity(0.7), in: RoundedRectangle(cornerRadius: 8))
    }

    private func statusName(_ key: String) -> String { model.statuses.first { $0.key == key }?.name ?? key }
}

/// Lays children out left to right, starting a new line when one would overflow.
struct FlowLayout: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(width: proposal.width ?? .infinity, subviews: subviews)
        let height = rows.last.map { $0.y + $0.height } ?? 0
        let width = rows.map(\.width).max() ?? 0
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for row in arrange(width: bounds.width, subviews: subviews) {
            var x = bounds.minX
            for i in row.items {
                let size = subviews[i].sizeThatFits(.unspecified)
                // A chip wider than the column alone gets the column's width and truncates.
                let w = min(size.width, bounds.width)
                subviews[i].place(at: CGPoint(x: x, y: bounds.minY + row.y), proposal: ProposedViewSize(width: w, height: size.height))
                x += w + spacing
            }
        }
    }

    private struct Row { var items: [Int] = []; var y: CGFloat = 0; var width: CGFloat = 0; var height: CGFloat = 0 }

    private func arrange(width: CGFloat, subviews: Subviews) -> [Row] {
        var rows: [Row] = [Row()]
        for i in subviews.indices {
            let size = subviews[i].sizeThatFits(.unspecified)
            var row = rows[rows.count - 1]
            let needed = row.items.isEmpty ? size.width : row.width + spacing + size.width
            if needed > width, !row.items.isEmpty {
                rows.append(Row(items: [i], y: row.y + row.height + spacing, width: size.width, height: size.height))
                continue
            }
            row.items.append(i)
            row.width = needed
            row.height = max(row.height, size.height)
            rows[rows.count - 1] = row
        }
        return rows.filter { !$0.items.isEmpty }
    }
}
