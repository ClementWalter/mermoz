/// A row or card that other rows and cards can be dropped onto to join its conversation.
/// While a drop would merge, it is outlined and labelled "Merge into this conversation",
/// unlike a status or project target, which only tints.
import MermozKit
import SwiftUI

struct MergeDropTarget: ViewModifier {
    @Environment(AppModel.self) private var model
    let threadId: String
    let radius: CGFloat
    @State private var hovering = false

    func body(content: Content) -> some View {
        content
            .overlay {
                if hovering { MergeHighlight(radius: radius) }
            }
            .dropDestination(for: String.self) { items, _ in
                hovering = false
                return model.dropMerge(items, onto: threadId) != nil
            } isTargeted: { on in
                // The dragged card itself, or a selection holding it, is no target.
                hovering = on && model.canMerge(into: threadId)
            }
    }
}

/// The merge affordance: an accent outline and a label over the target.
struct MergeHighlight: View {
    let radius: CGFloat

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: radius).fill(Theme.accent.opacity(0.08))
            RoundedRectangle(cornerRadius: radius).strokeBorder(Theme.accent, style: StrokeStyle(lineWidth: 2, dash: [6, 3]))
            Label("Merge into this conversation", systemImage: "arrow.triangle.merge")
                .font(.system(size: 11.5, weight: .semibold))
                .foregroundStyle(.white)
                .padding(.horizontal, 9).padding(.vertical, 4)
                .background(Theme.accent, in: Capsule())
                .lineLimit(1).fixedSize()
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

extension View {
    /// Dropping rows or cards here merges them into `threadId`'s conversation.
    func mergeDropTarget(_ threadId: String, radius: CGFloat = 0) -> some View {
        modifier(MergeDropTarget(threadId: threadId, radius: radius))
    }
}
