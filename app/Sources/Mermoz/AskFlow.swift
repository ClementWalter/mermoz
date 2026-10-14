/// "Ask Mermoz to do" in ⌘K: the sentence goes to the daemon, which has the AI plan it
/// from what the mailbox holds; the plan is shown, Enter (or the typed count, for a
/// delete) runs it, Esc drops it. When the AI asks back, the field takes the answer
/// (two rounds at most) and the mailbox's candidates are offered as choices, so there is
/// always a way forward besides Esc.
import MermozKit
import SwiftUI

@MainActor
@Observable
final class AskFlow {
    enum Phase: Equatable {
        case idle
        /// The AI is reading the sentence (or the answer, or the pick).
        case thinking(String)
        case preview(DoPreview)
        /// The AI asked back; `choices` and an answer in the field lead on.
        case question(DoPreview)
        case running(done: Int, total: Int)
        /// No plan and nothing to pick: what went wrong, in a sentence.
        case answer(String)
    }

    /// Rounds of answers the bar takes before only the choices remain.
    static let maxRounds = 2

    var phase: Phase = .idle
    /// What the ⌘K field holds: the search, the instruction, or an answer to a question.
    var query = ""
    /// Whether an AI is connected; nil until asked.
    var availability: DoAvailability?
    /// What the user typed to confirm a delete.
    var typed = ""
    /// The highlighted choice (↑/↓), picked by Enter when the field is empty.
    var choiceCursor = 0
    /// The preview's full list, once "and N more" is opened; nil while only the sample shows.
    var rows: [DoSample]?
    /// Threads unticked in the list: the run leaves them alone.
    var excluded: Set<String> = []
    /// The highlighted row of the full list (↑/↓).
    var rowCursor = 0
    /// The instruction being planned and the questions answered so far.
    private(set) var sentence = ""
    private(set) var rounds: [DoRound] = []

    var isActive: Bool { phase != .idle }

    var preview: DoPreview? {
        if case let .preview(p) = phase { return p }
        return nil
    }

    var question: DoPreview? {
        if case let .question(q) = phase { return q }
        return nil
    }

    /// Choices on screen: under a question, or under a preview that matched nothing.
    var choices: [DoChoice] {
        if let q = question { return q.choices ?? [] }
        if let p = preview, (p.total ?? 0) == 0 { return p.choices ?? [] }
        return []
    }

    /// The field answers the question (not yet two rounds, and the daemon takes more).
    var takesAnswer: Bool {
        guard let q = question else { return false }
        return (q.canAnswer ?? true) && rounds.count < Self.maxRounds
    }

    /// Threads the run will change: the plan's, less the unticked ones.
    var selectedCount: Int {
        max(0, (preview?.total ?? 0) - excluded.count)
    }

    /// Enter runs the plan: something is ticked, and a delete has that count typed.
    var canConfirm: Bool {
        guard let p = preview, p.isPlan, selectedCount > 0 else { return false }
        return !p.needsTypedCount || AskIntent.confirms(typed, total: selectedCount)
    }

    /// "and N more": loads every thread of the plan into the preview.
    @discardableResult
    func expand(backend: AskBackend) -> Task<Void, Never>? {
        guard rows == nil, let id = preview?.id else { return nil }
        return Task {
            guard let all = try? await backend.doTargets(id), preview?.id == id else { return }
            rows = all
            rowCursor = 0
        }
    }

    /// Ticks or unticks a thread of the list.
    func toggle(_ id: String) {
        if excluded.contains(id) { excluded.remove(id) } else { excluded.insert(id) }
    }

    func moveRow(_ by: Int) {
        guard let rows, !rows.isEmpty else { return }
        rowCursor = max(0, min(rows.count - 1, rowCursor + by))
    }

    /// The highlighted thread of the full list.
    var cursorRow: DoSample? {
        guard let rows, rows.indices.contains(rowCursor) else { return nil }
        return rows[rowCursor]
    }

    func refreshAvailability(_ backend: AskBackend?) async {
        guard let backend else { return }
        availability = try? await backend.doStatus()
    }

    /// Asks for a plan; the preview replaces the palette's list once it arrives.
    @discardableResult
    func start(_ text: String, backend: AskBackend) -> Task<Void, Never> {
        sentence = text
        rounds = []
        return ask(label: text) { try await backend.doAsk(text, rounds: [], pick: nil) }
    }

    /// Sends the field's text as the answer to the AI's question.
    @discardableResult
    func reply(_ answer: String, backend: AskBackend) -> Task<Void, Never>? {
        let a = answer.trimmingCharacters(in: .whitespaces)
        guard takesAnswer, !a.isEmpty, let q = question?.message else { return nil }
        rounds.append(DoRound(question: q, answer: a))
        query = ""
        let (s, r) = (sentence, rounds)
        return ask(label: a) { try await backend.doAsk(s, rounds: r, pick: nil) }
    }

    /// Re-plans with exactly the picked sender or subject.
    @discardableResult
    func pick(_ c: DoChoice, backend: AskBackend) -> Task<Void, Never> {
        let (s, r) = (sentence, rounds)
        if let id = preview?.id {
            return ask(label: c.label) { try await backend.doReplan(id, pick: c.pick) }
        }
        return ask(label: c.label) { try await backend.doAsk(s, rounds: r, pick: c.pick) }
    }

    /// Enter while asking: run the plan, send the answer, pick the highlighted choice,
    /// or start over with what the field now says.
    @discardableResult
    func enter(backend: AskBackend, onDone: @escaping @MainActor (DoRunResult?) -> Void) -> Task<Void, Never>? {
        let text = query.trimmingCharacters(in: .whitespaces)
        if preview != nil, canConfirm { return confirm(backend: backend, onDone: onDone) }
        if question != nil, takesAnswer, !text.isEmpty { return reply(text, backend: backend) }
        if !choices.isEmpty, text.isEmpty || !takesAnswer {
            return pick(choices[min(choiceCursor, choices.count - 1)], backend: backend)
        }
        // No more answers taken and nothing to pick: the field's words are a new instruction.
        var stuck = question != nil
        if case .answer = phase { stuck = true }
        if stuck, !text.isEmpty { return start(text, backend: backend) }
        return nil
    }

    /// ↑/↓ over the choices.
    func moveChoice(_ by: Int) {
        guard !choices.isEmpty else { return }
        choiceCursor = max(0, min(choices.count - 1, choiceCursor + by))
    }

    private func ask(label: String, _ call: @escaping @MainActor () async throws -> DoPreview) -> Task<Void, Never> {
        typed = ""
        choiceCursor = 0
        rows = nil
        excluded = []
        rowCursor = 0
        let mark = Phase.thinking(label)
        phase = mark
        return Task {
            do {
                let p = try await call()
                guard phase == mark else { return }
                if p.isPlan {
                    phase = .preview(p)
                } else if p.isQuestion {
                    // The field now takes the answer.
                    query = ""
                    phase = .question(p)
                } else {
                    phase = .answer(p.message ?? "Mermoz couldn't plan that.")
                }
            } catch {
                guard phase == mark else { return }
                phase = .answer((error as? RPCError)?.message ?? error.localizedDescription)
            }
        }
    }

    /// Runs the previewed plan in the daemon, showing its progress, then hands the
    /// result over (for the Undo entry); nil when it could not run.
    @discardableResult
    func confirm(backend: AskBackend, onDone: @escaping @MainActor (DoRunResult?) -> Void) -> Task<Void, Never>? {
        guard canConfirm, let p = preview, let id = p.id else { return nil }
        let total = selectedCount
        let confirm = p.needsTypedCount ? total : nil
        let exclude = excluded.sorted()
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
                let r = try await backend.doRun(id, confirm: confirm, exclude: exclude)
                phase = .idle
                typed = ""
                onDone(r)
            } catch {
                phase = .answer((error as? RPCError)?.message ?? error.localizedDescription)
                onDone(nil)
            }
        }
    }

    /// The field changed: a plan made for other words is dropped. A question keeps
    /// going (the field holds its answer), and so does a run.
    func fieldEdited() {
        switch phase {
        case .preview, .answer: cancel()
        default: break
        }
    }

    /// Esc: drops the plan or the question (nothing ran). A run already going keeps going.
    func cancel() {
        if case .running = phase { return }
        if phase != .idle { phase = .idle }
        if !typed.isEmpty { typed = "" }
        rounds = []
        if rows != nil { rows = nil }
        if !excluded.isEmpty { excluded = [] }
    }
}

/// The palette's body while asking: the plan in plain words, how it was understood,
/// who the sender words matched, counts per status, the first subjects, the delete
/// confirmation; or the AI's question, the answers so far and the choices.
struct AskPanel: View {
    @Environment(AppModel.self) private var model
    let flow: AskFlow
    @FocusState private var countFocused: Bool
    @FocusState private var listFocused: Bool

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
            case let .question(q):
                questionView(q)
            case let .running(done, total):
                ProgressView(value: Double(done), total: Double(max(total, 1)))
                Text("\(AskIntent.count(done)) of \(AskIntent.count(total)) threads…").foregroundStyle(Theme.text2)
            case let .answer(message):
                Label(message, systemImage: "sparkles").foregroundStyle(Theme.text)
                Text("Type another way to say it and press ↩.").font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
        }
        .font(.system(size: 13))
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(18)
    }

    @ViewBuilder private func questionView(_ q: DoPreview) -> some View {
        Text("“\(flow.sentence)”").foregroundStyle(Theme.text3)
        ForEach(Array(flow.rounds.enumerated()), id: \.offset) { _, r in
            Text("✦ \(r.question)").foregroundStyle(Theme.text2)
            Text(r.answer).foregroundStyle(Theme.text3)
        }
        Label(q.message ?? "", systemImage: "sparkles").font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.text)
        if flow.takesAnswer {
            Text("Type your answer above and press ↩, or pick one:").font(.system(size: 12)).foregroundStyle(Theme.text3)
        }
        choicesView
    }

    @ViewBuilder private var choicesView: some View {
        if !flow.choices.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                ForEach(Array(flow.choices.enumerated()), id: \.element.id) { i, c in
                    Button { model.pickAsk(c) } label: {
                        HStack(spacing: 8) {
                            Image(systemName: c.pick.from != nil ? "person.crop.circle" : "text.alignleft")
                                .foregroundStyle(Theme.accent).frame(width: 16)
                            Text(c.label).lineLimit(1)
                            Spacer()
                        }
                        .padding(.horizontal, 8).frame(height: 28)
                        .background(i == flow.choiceCursor ? Theme.accent.opacity(0.11) : .clear)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    @ViewBuilder private func preview(_ p: DoPreview) -> some View {
        ForEach(Array((p.steps ?? []).enumerated()), id: \.offset) { _, step in
            Text(step.summary).font(.system(size: 14.5, weight: .semibold)).foregroundStyle(Theme.text)
            Text("I understood: \(step.interpretation)").font(.system(size: 12)).foregroundStyle(Theme.text3)
            if let r = step.resolved {
                Text("Matched \(r)").font(.system(size: 12)).foregroundStyle(Theme.text2)
            }
            if !step.byStatus.isEmpty {
                // Where the threads are now, before the plan: never the action itself.
                Text(AskIntent.currentLine(step.byStatus))
                    .font(.system(size: 12)).foregroundStyle(Theme.text2)
            }
            if flow.rows == nil {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(step.sample) { t in
                        HStack(spacing: 8) {
                            Text(t.subject.isEmpty ? "(no subject)" : t.subject).lineLimit(1)
                            Spacer()
                            Text(t.lastFrom).foregroundStyle(Theme.text3).lineLimit(1)
                        }
                        .font(.system(size: 12.5))
                    }
                }
            }
        }
        if let rows = flow.rows {
            fullList(rows)
        } else if (p.total ?? 0) > sampleCount(p) {
            Button { model.expandAsk() } label: {
                Text("and \(AskIntent.count((p.total ?? 0) - sampleCount(p))) more · review the list")
                    .font(.system(size: 12)).foregroundStyle(Theme.accent)
            }
            .buttonStyle(.plain)
        }
        if !flow.excluded.isEmpty, let total = p.total {
            Text("\(AskIntent.count(flow.selectedCount)) of \(AskIntent.count(total)) ticked")
                .font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.text2)
        }
        if let w = p.warning {
            Label(w, systemImage: "exclamationmark.triangle").font(.system(size: 12.5)).foregroundStyle(Theme.text2)
        }
        if !flow.choices.isEmpty {
            Text("Did you mean:").font(.system(size: 12)).foregroundStyle(Theme.text3)
            choicesView
        }
        if p.needsTypedCount, flow.selectedCount > 0 {
            @Bindable var flow = flow
            TextField("Type \(AskIntent.count(flow.selectedCount)) to delete", text: $flow.typed)
                .textFieldStyle(.roundedBorder)
                .focused($countFocused)
                .onSubmit { model.confirmAsk() }
                .onKeyPress(.escape) { flow.cancel(); return .handled }
                // The list takes the keys while it is open; the count is typed after.
                .task { if flow.rows == nil { countFocused = true } }
        }
    }

    /// Threads shown under the steps before "and N more" (each step's sample, once each).
    private func sampleCount(_ p: DoPreview) -> Int {
        Set((p.steps ?? []).flatMap { $0.sample.map(\.id) }).count
    }

    /// Every thread of the plan: tick or untick each, ↑/↓ to move, Space or ⌘↩ to open
    /// one in the reading pane, x or ⌫ to untick, ↩ to run.
    private func fullList(_ rows: [DoSample]) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.element.id) { i, t in
                        AskRow(thread: t, ticked: !flow.excluded.contains(t.id), highlighted: i == flow.rowCursor,
                               statusName: model.statuses.first { $0.key == t.status }?.name ?? t.status,
                               statusColor: Color(hex: model.color(forStatus: t.status))) {
                            flow.toggle(t.id)
                        }
                        .id(t.id)
                        .onTapGesture { flow.rowCursor = i; model.openAskRow() }
                    }
                }
            }
            .frame(maxHeight: 240)
            .focusable()
            .focusEffectDisabled()
            .focused($listFocused)
            .onKeyPress(.downArrow) { flow.moveRow(1); return .handled }
            .onKeyPress(.upArrow) { flow.moveRow(-1); return .handled }
            .onKeyPress(.space) { model.openAskRow(); return .handled }
            .onKeyPress(.delete) { if let r = flow.cursorRow { flow.toggle(r.id) }; return .handled }
            .onKeyPress(characters: .init(charactersIn: "xX")) { _ in
                if let r = flow.cursorRow { flow.toggle(r.id) }
                return .handled
            }
            .onKeyPress(keys: [.return]) { press in
                if press.modifiers.contains(.command) { model.openAskRow() } else { model.enterAsk() }
                return .handled
            }
            .onKeyPress(.escape) { flow.cancel(); return .handled }
            .onChange(of: flow.rowCursor) { _, c in
                if rows.indices.contains(c) { proxy.scrollTo(rows[c].id) }
            }
            .task { listFocused = true }
        }
    }
}

/// One thread of the plan's full list: tick box, subject, sender, date, current status.
private struct AskRow: View {
    let thread: DoSample
    let ticked: Bool
    let highlighted: Bool
    let statusName: String
    let statusColor: Color
    let toggle: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Toggle("", isOn: Binding(get: { ticked }, set: { _ in toggle() }))
                .toggleStyle(.checkbox).labelsHidden()
            Text(thread.subject.isEmpty ? "(no subject)" : thread.subject).lineLimit(1)
                .foregroundStyle(ticked ? Theme.text : Theme.text3)
            Spacer(minLength: 8)
            Text(thread.lastFrom).foregroundStyle(Theme.text3).lineLimit(1).frame(maxWidth: 140, alignment: .trailing)
            Text(AskRow.day(thread.lastDate)).foregroundStyle(Theme.text3).frame(width: 74, alignment: .trailing)
            HStack(spacing: 3) {
                StatusIcon(key: thread.status, color: statusColor)
                Text(statusName).lineLimit(1)
            }
            .foregroundStyle(Theme.text3).frame(width: 90, alignment: .leading)
        }
        .font(.system(size: 12))
        .padding(.horizontal, 6).frame(height: 26)
        .background(highlighted ? Theme.accent.opacity(0.11) : .clear)
        .contentShape(Rectangle())
    }

    private static let format: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "d MMM yyyy"
        return f
    }()

    static func day(_ ms: Int64?) -> String {
        guard let ms else { return "" }
        return format.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }
}
