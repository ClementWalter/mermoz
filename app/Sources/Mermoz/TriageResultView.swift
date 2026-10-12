/// What the AI changed after a column triage, with one-click undo.
import MermozKit
import SwiftUI

struct TriageResultView: View {
    @Environment(AppModel.self) private var model
    let result: TriageResult

    private func statusName(_ key: String) -> String {
        model.statuses.first { $0.key == key }?.name ?? key
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                Image(systemName: "sparkles").foregroundStyle(Theme.accent)
                Text("AI moved \(result.moved) of \(result.examined) threads").font(.system(size: 15, weight: .semibold))
            }
            if result.moves.isEmpty {
                Text("Everything already looked in the right place.").foregroundStyle(Theme.text2)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(result.moves) { m in
                            HStack(alignment: .firstTextBaseline, spacing: 10) {
                                Text(m.number).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.text3)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(m.subject.isEmpty ? "(no subject)" : m.subject).lineLimit(1)
                                    Text(m.reason).font(.system(size: 11.5)).foregroundStyle(Theme.text2).lineLimit(2)
                                }
                                Spacer(minLength: 8)
                                // Names, not just glyphs: Backlog and Todo are both empty rings.
                                HStack(spacing: 5) {
                                    if m.priorityOnly {
                                        Text("Priority").foregroundStyle(Theme.text2)
                                        Image(systemName: "arrow.right").font(.system(size: 9)).foregroundStyle(Theme.text3)
                                        PriorityIcon(priority: m.priority)
                                        Text(Theme.priorityNames[m.priority] ?? "").fontWeight(.medium)
                                    } else {
                                        StatusIcon(key: m.from, color: Color(hex: model.color(forStatus: m.from)))
                                        Text(statusName(m.from)).foregroundStyle(Theme.text2)
                                        Image(systemName: "arrow.right").font(.system(size: 9)).foregroundStyle(Theme.text3)
                                        StatusIcon(key: m.to, color: Color(hex: model.color(forStatus: m.to)))
                                        Text(statusName(m.to)).fontWeight(.medium)
                                    }
                                }
                                .font(.system(size: 12))
                                .fixedSize()
                            }
                            .font(.system(size: 12.5))
                            .padding(.vertical, 7)
                            Divider().overlay(Theme.line)
                        }
                    }
                }
                .frame(maxHeight: 360)
            }
            HStack {
                if !result.moves.isEmpty {
                    Button("Undo") { model.undoTriage(result) }.buttonStyle(.glass)
                }
                Spacer()
                Button("Done") { model.triageResult = nil }.buttonStyle(.glassProminent).tint(Theme.accent).keyboardShortcut(.defaultAction)
            }
        }
        .foregroundStyle(Theme.text)
        .padding(22)
        .frame(width: 560)
    }
}
