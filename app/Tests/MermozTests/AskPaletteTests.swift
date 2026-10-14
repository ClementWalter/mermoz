/// The ⌘K "Ask Mermoz to do" flow in-process, against a stub daemon: the row for an
/// instruction, the preview, Enter to run (one Undo entry), Esc to drop the plan, the
/// typed count for a delete, answering the AI's question in the field, the choices that
/// re-plan, and what one keystroke costs the main thread.
import AppKit
import Foundation
import MermozKit
import SwiftUI
import Testing
@testable import Mermoz

/// Stands in for the daemon: answers each ask with the next preview (the last repeats)
/// and records what was asked, re-planned and run.
private final class StubBackend: AskBackend, @unchecked Sendable {
    private let answers: [DoPreview]
    private let lock = NSLock()
    private var _runs: [(String, Int?)] = []
    private var _asks: [([DoRound], DoPick?)] = []
    private var _replans: [(String, DoPick)] = []

    init(_ answers: DoPreview...) { self.answers = answers }

    var runs: [(String, Int?)] { lock.withLock { _runs } }
    var asks: [([DoRound], DoPick?)] { lock.withLock { _asks } }
    var replans: [(String, DoPick)] { lock.withLock { _replans } }

    private func next() -> DoPreview {
        let n = lock.withLock { _asks.count + _replans.count }
        return answers[min(n - 1, answers.count - 1)]
    }

    func doStatus() async throws -> DoAvailability { DoAvailability(available: true, providerName: "Work AI") }
    func doAsk(_ text: String, rounds: [DoRound], pick: DoPick?) async throws -> DoPreview {
        lock.withLock { _asks.append((rounds, pick)) }
        return next()
    }
    func doReplan(_ id: String, pick: DoPick) async throws -> DoPreview {
        lock.withLock { _replans.append((id, pick)) }
        return next()
    }
    func doRun(_ id: String, confirm: Int?) async throws -> DoRunResult {
        lock.withLock { _runs.append((id, confirm)) }
        return DoRunResult(id: id, done: 2, total: 2, undoLabel: "Archive")
    }
    func doGet(_ id: String) async throws -> DoOperation { DoOperation(id: id, state: "running", done: 0, total: 2) }
    func doUndo(_ id: String) async throws {}
}

private let sentence = "archive all messages past 5 months not in done"
private let notes = DoChoice(label: "Gemini · gemini@google.example · 40 threads", pick: DoPick(from: "gemini@google.example"), threads: 40)
private let notesSubject = DoChoice(label: "Subject “Notes:…” · 40 threads", pick: DoPick(subjectPrefix: "Notes:"), threads: 40)

private func plan(total: Int = 2, confirm: String = "enter", id: String = "bop_1", choices: [DoChoice] = []) -> DoPreview {
    DoPreview(kind: "plan", id: id, summary: "Archive \(total) threads older than 5 months that aren't Done",
              interpretation: "status ≠ Done AND older than 5 months",
              steps: [DoStep(summary: "Archive \(total) threads older than 5 months that aren't Done",
                             interpretation: "status ≠ Done AND older than 5 months", action: "Archive", count: total,
                             byStatus: [DoStatusCount(status: "todo", name: "To do", count: total)],
                             sample: [DoSample(id: "t1", number: "M-1", status: "todo", subject: "Ferry tickets", lastFrom: "Nora Lind")])],
              total: total, deletes: confirm == "type_count", confirm: confirm, choices: choices)
}

private func asksBack(canAnswer: Bool = true) -> DoPreview {
    DoPreview(kind: "question", message: "Which notes do you mean?", choices: [notes, notesSubject], canAnswer: canAnswer)
}

/// The model after Enter on the Ask row, with the stub's first answer shown.
@MainActor
private func asked(_ stub: StubBackend) async -> AppModel {
    let model = AppModel()
    await model.askFlow.start(sentence, backend: stub).value
    return model
}

@MainActor
private func previewed(_ p: DoPreview = plan()) async -> (AppModel, StubBackend) {
    let stub = StubBackend(p)
    return (await asked(stub), stub)
}

// MARK: the row

@MainActor @Test func anInstructionGetsTheAskRow() {
    let row = CommandPalette.askItem(sentence, otherMatches: true, availability: nil) {}
    #expect(row?.title == "✦ Ask Mermoz to do: “\(sentence)”")
}

@MainActor @Test func aSearchGetsNoAskRow() {
    #expect(CommandPalette.askItem("audit report from marc", otherMatches: true, availability: nil) {} == nil)
}

@MainActor @Test func withoutAnAITheRowSaysWhereToConnectOne() {
    let row = CommandPalette.askItem(sentence, otherMatches: true, availability: DoAvailability(available: false)) {}
    #expect(row?.detail == "Connect an AI first: Settings › Agents")
}

@MainActor @Test func theAskRowKeepsThePaletteOpen() {
    #expect(CommandPalette.askItem(sentence, otherMatches: true, availability: nil) {}?.keepsOpen == true)
}

// MARK: preview, confirm, cancel

@MainActor @Test func askingShowsThePreview() async {
    let (model, _) = await previewed()
    #expect(model.askFlow.preview?.summary == "Archive 2 threads older than 5 months that aren't Done")
}

@MainActor @Test func thePreviewRunsNothing() async {
    let (_, stub) = await previewed()
    #expect(stub.runs.isEmpty)
}

@MainActor @Test func enterRunsThePlan() async {
    let (model, stub) = await previewed()
    await model.enterAsk(backend: stub)?.value
    #expect(stub.runs.map(\.0) == ["bop_1"])
}

@MainActor @Test func aRunLeavesOneUndoForAllOfIt() async {
    let (model, stub) = await previewed()
    await model.confirmAsk(backend: stub)?.value
    #expect(model.undoStack.last?.doOperation == "bop_1")
}

@MainActor @Test func aRunSaysWhatItDid() async {
    let (model, stub) = await previewed()
    await model.confirmAsk(backend: stub)?.value
    #expect(model.toast == "Archive: 2 threads · ⌘Z to undo")
}

@MainActor @Test func aRunClosesThePalette() async {
    let (model, stub) = await previewed()
    model.paletteMode = .search
    await model.confirmAsk(backend: stub)?.value
    #expect(model.paletteMode == nil)
}

@MainActor @Test func escDropsThePlan() async {
    let (model, _) = await previewed()
    model.askFlow.cancel()
    #expect(!model.askFlow.isActive)
}

@MainActor @Test func escRunsNothing() async {
    let (model, stub) = await previewed()
    model.askFlow.cancel()
    _ = model.enterAsk(backend: stub)
    #expect(stub.runs.isEmpty)
}

@MainActor @Test func editingTheSentenceDropsThePlan() async {
    let (model, _) = await previewed()
    model.askFlow.query = "archive all messages past 6 months"
    model.askFlow.fieldEdited()
    #expect(!model.askFlow.isActive)
}

@MainActor @Test func nothingMatchingCannotBeConfirmed() async {
    let (model, _) = await previewed(plan(total: 0))
    #expect(!model.askFlow.canConfirm)
}

// MARK: delete

@MainActor @Test func aDeleteWaitsForTheCount() async {
    let (model, _) = await previewed(plan(total: 1842, confirm: "type_count"))
    #expect(!model.askFlow.canConfirm)
}

@MainActor @Test func aDeleteRunsOnceTheCountIsTyped() async {
    let (model, stub) = await previewed(plan(total: 1842, confirm: "type_count"))
    model.askFlow.typed = "1,842"
    await model.confirmAsk(backend: stub)?.value
    #expect(stub.runs.first?.1 == 1842)
}

@MainActor @Test func aWrongCountRunsNothing() async {
    let (model, stub) = await previewed(plan(total: 1842, confirm: "type_count"))
    model.askFlow.typed = "184"
    _ = model.confirmAsk(backend: stub)
    #expect(stub.runs.isEmpty)
}

// MARK: questions, answers and choices

@MainActor @Test func aQuestionShowsItsChoices() async {
    let model = await asked(StubBackend(asksBack()))
    #expect(model.askFlow.choices.map(\.label) == [notes.label, notesSubject.label])
}

@MainActor @Test func aQuestionEmptiesTheFieldForTheAnswer() async {
    let model = await asked(StubBackend(asksBack()))
    #expect(model.askFlow.query.isEmpty && model.askFlow.takesAnswer)
}

@MainActor @Test func typingTheAnswerKeepsTheQuestion() async {
    let model = await asked(StubBackend(asksBack()))
    model.askFlow.query = "the meeting notes"
    model.askFlow.fieldEdited()
    #expect(model.askFlow.question != nil)
}

@MainActor @Test func enterSendsTheAnswerWithTheQuestion() async {
    let stub = StubBackend(asksBack(), plan())
    let model = await asked(stub)
    model.askFlow.query = "the meeting notes"
    await model.enterAsk(backend: stub)?.value
    #expect(stub.asks.last?.0 == [DoRound(question: "Which notes do you mean?", answer: "the meeting notes")])
}

@MainActor @Test func anAnswerCanEndInAPlan() async {
    let stub = StubBackend(asksBack(), plan())
    let model = await asked(stub)
    model.askFlow.query = "the meeting notes"
    await model.enterAsk(backend: stub)?.value
    #expect(model.askFlow.preview?.id == "bop_1")
}

@MainActor @Test func afterTwoAnswersOnlyTheChoicesRemain() async {
    let stub = StubBackend(asksBack())
    let model = await asked(stub)
    for answer in ["the notes", "from the meetings"] {
        model.askFlow.query = answer
        await model.enterAsk(backend: stub)?.value
    }
    #expect(!model.askFlow.takesAnswer && !model.askFlow.choices.isEmpty)
}

@MainActor @Test func clickingAChoiceAsksWithThatFilter() async {
    let stub = StubBackend(asksBack(), plan())
    let model = await asked(stub)
    await model.pickAsk(notes, backend: stub)?.value
    #expect(stub.asks.last?.1 == DoPick(from: "gemini@google.example"))
}

@MainActor @Test func enterOnAnEmptyFieldPicksTheHighlightedChoice() async {
    let stub = StubBackend(asksBack(), plan())
    let model = await asked(stub)
    model.askFlow.moveChoice(1)
    await model.enterAsk(backend: stub)?.value
    #expect(stub.asks.last?.1 == DoPick(subjectPrefix: "Notes:"))
}

@MainActor @Test func anEmptyPreviewOffersItsSuggestions() async {
    let (model, _) = await previewed(plan(total: 0, choices: [notes]))
    #expect(model.askFlow.choices == [notes])
}

@MainActor @Test func aSuggestionReplansThePreview() async {
    let stub = StubBackend(plan(total: 0, choices: [notes]), plan(total: 40, id: "bop_2"))
    let model = await asked(stub)
    await model.pickAsk(notes, backend: stub)?.value
    #expect(stub.replans.map(\.0) == ["bop_1"] && model.askFlow.preview?.total == 40)
}

@MainActor @Test func aPickSendsTheProtocolsKeys() throws {
    let data = Params().with("pick", DoPick(subjectPrefix: "Notes:")).data()
    let json = try JSONSerialization.jsonObject(with: data) as? [String: [String: String]]
    #expect(json?["pick"] == ["subject_prefix": "Notes:"])
}

// MARK: keystroke cost

/// One keystroke in a palette over a big mailbox (300 projects, 9 statuses): the field
/// changes, then the palette renders and lays out, all on the main thread.
@MainActor @Test func aKeystrokeCostsUnderFourMilliseconds() throws {
    let model = AppModel()
    model.paletteMode = .search
    let projects = (0..<300).map { "{\"id\":\"Project \($0)\",\"name\":\"Project \($0)\",\"color\":\"#5B8DEF\"}" }
    model.projects = try JSONDecoder().decode([Project].self, from: Data("[\(projects.joined(separator: ","))]".utf8))
    model.statuses = ["unread", "todo", "waiting", "in_progress", "backlog", "review", "done", "archive", "deleted"]
        .enumerated().map { Status(key: $1, name: $1, order: $0, color: "#5A7386") }
    let host = NSHostingView(rootView: AnyView(CommandPalette().environment(model)))
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 900, height: 700), styleMask: .borderless,
                          backing: .buffered, defer: false)
    window.contentView = host
    host.layoutSubtreeIfNeeded()
    let typed = "delete all message from gemeni notes from google meet"
    var total = Duration.zero
    for c in typed {
        let t0 = ContinuousClock.now
        model.askFlow.query.append(c)
        // Layout applies the change: the palette renders and lays out. No run-loop turn,
        // so no other main-thread work is counted.
        host.layoutSubtreeIfNeeded()
        total += t0.duration(to: .now)
    }
    let perKey = total / typed.count
    #expect(perKey < .milliseconds(4), "a keystroke took \(perKey)")
}
