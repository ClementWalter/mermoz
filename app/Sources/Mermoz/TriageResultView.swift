/// What the AI changed after a column triage, with one-click undo.
import MermozKit
import SwiftUI

struct TriageResultView: View {
    @Environment(AppModel.self) private var model
    let result: TriageResult

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
                                HStack(spacing: 4) {
                                    StatusIcon(key: m.from, color: Color(hex: model.color(forStatus: m.from)))
                                    Image(systemName: "arrow.right").font(.system(size: 9)).foregroundStyle(Theme.text3)
                                    StatusIcon(key: m.to, color: Color(hex: model.color(forStatus: m.to)))
                                }
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
