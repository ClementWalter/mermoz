/// "Ask Mermoz to do" in ⌘K: the sentence goes to the daemon, which has the AI plan it;
/// the plan is shown, Enter (or the typed count, for a delete) runs it, Esc drops it.
import MermozKit
import SwiftUI

@MainActor
@Observable
final class AskFlow {
    enum Phase: Equatable {
        case idle
        /// The AI is reading the sentence.
        case thinking(String)
        case preview(DoPreview)
        case running(done: Int, total: Int)
        /// No plan: what the AI or Mermoz says instead (unclear, can't be done, no AI).
        case answer(String)
    }

    var phase: Phase = .idle
    /// Whether an AI is connected; nil until asked.
    var availability: DoAvailability?
    /// What the user typed to confirm a delete.
    var typed = ""

    var isActive: Bool { phase != .idle }

    var preview: DoPreview? {
        if case let .preview(p) = phase { return p }
        return nil
    }

    /// Enter runs the plan: something matches, and a delete has its count typed.
    var canConfirm: Bool {
        guard let p = preview, p.isPlan, let total = p.total, total > 0 else { return false }
        return !p.needsTypedCount || AskIntent.confirms(typed, total: total)
    }

    func refreshAvailability(_ backend: AskBackend?) async {
        guard let backend else { return }
        availability = try? await backend.doStatus()
    }

    /// Asks for a plan; the preview replaces the palette's list once it arrives.
    @discardableResult
    func start(_ text: String, backend: AskBackend) -> Task<Void, Never> {
        typed = ""
        phase = .thinking(text)
        return Task {
            do {
                let p = try await backend.doAsk(text)
                guard phase == .thinking(text) else { return }
                phase = p.isPlan ? .preview(p) : .answer(p.message ?? "Mermoz couldn't plan that.")
            } catch {
                guard phase == .thinking(text) else { return }
                phase = .answer((error as? RPCError)?.message ?? error.localizedDescription)
            }
        }
    }

    /// Runs the previewed plan in the daemon, showing its progress, then hands the
    /// result over (for the Undo entry); nil when it could not run.
    @discardableResult
    func confirm(backend: AskBackend, onDone: @escaping @MainActor (DoRunResult?) -> Void) -> Task<Void, Never>? {
        guard canConfirm, let p = preview, let id = p.id, let total = p.total else { return nil }
        let confirm = p.needsTypedCount ? total : nil
        phase = .running(done: 0, total: total)
        // Progress comes from the run's log while the run call is out.
        let poll = Task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(250))
                guard let op = try? await backend.doGet(id), case .running = phase else { continue }
                phase = .running(done: op.done, total: op.total)
            }
        }
        return Task {
            defer { poll.cancel() }
            do {
                let r = try await backend.doRun(id, confirm: confirm)
                phase = .idle
                typed = ""
                onDone(r)
            } catch {
                phase = .answer((error as? RPCError)?.message ?? error.localizedDescription)
                onDone(nil)
            }
        }
    }

    /// Esc: drops the plan (nothing ran). A run already going keeps going.
    func cancel() {
        if case .running = phase { return }
        phase = .idle
        typed = ""
    }
}

/// The palette's body while asking: the plan in plain words, how it was understood,
/// counts per status, the first subjects, and the delete confirmation.
struct AskPanel: View {
    @Environment(AppModel.self) private var model
    let flow: AskFlow
    @FocusState private var countFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            switch flow.phase {
            case .idle:
                EmptyView()
            case let .thinking(text):
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Reading “\(text)”…").foregroundStyle(Theme.text2)
                }
            case let .preview(p):
                preview(p)
            case let .running(done, total):
                ProgressView(value: Double(done), total: Double(max(total, 1)))
                Text("\(AskIntent.count(done)) of \(AskIntent.count(total)) threads…").foregroundStyle(Theme.text2)
            case let .answer(message):
                Label(message, systemImage: "sparkles").foregroundStyle(Theme.text)
            }
        }
        .font(.system(size: 13))
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(18)
    }

    @ViewBuilder private func preview(_ p: DoPreview) -> some View {
        ForEach(Array((p.steps ?? []).enumerated()), id: \.offset) { _, step in
            Text(step.summary).font(.system(size: 14.5, weight: .semibold)).foregroundStyle(Theme.text)
            Text("I understood: \(step.interpretation)").font(.system(size: 12)).foregroundStyle(Theme.text3)
            if !step.byStatus.isEmpty {
                HStack(spacing: 12) {
                    ForEach(step.byStatus, id: \.status) { c in
                        HStack(spacing: 4) {
                            StatusIcon(key: c.status, color: Color(hex: model.color(forStatus: c.status)))
                            Text("\(c.name) \(AskIntent.count(c.count))")
                        }
                    }
                }
                .font(.system(size: 12)).foregroundStyle(Theme.text2)
            }
            VStack(alignment: .leading, spacing: 3) {
                ForEach(step.sample) { t in
                    HStack(spacing: 8) {
                        Text(t.subject.isEmpty ? "(no subject)" : t.subject).lineLimit(1)
                        Spacer()
                        Text(t.lastFrom).foregroundStyle(Theme.text3).lineLimit(1)
                    }
                    .font(.system(size: 12.5))
                }
                if step.count > step.sample.count {
                    Text("and \(AskIntent.count(step.count - step.sample.count)) more")
                        .font(.system(size: 12)).foregroundStyle(Theme.text3)
                }
            }
        }
        if let w = p.warning {
            Label(w, systemImage: "exclamationmark.triangle").font(.system(size: 12.5)).foregroundStyle(Theme.text2)
        }
        if p.needsTypedCount, let total = p.total, total > 0 {
            @Bindable var flow = flow
            TextField("Type \(AskIntent.count(total)) to delete", text: $flow.typed)
                .textFieldStyle(.roundedBorder)
                .focused($countFocused)
                .onSubmit { model.confirmAsk() }
                .onKeyPress(.escape) { flow.cancel(); return .handled }
                .task { countFocused = true }
        }
    }
}
