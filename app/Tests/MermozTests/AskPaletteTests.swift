/// The ⌘K "Ask Mermoz to do" flow in-process, against a stub daemon: the row for an
/// instruction, the preview, Enter to run (one Undo entry), Esc to drop the plan, and
/// the typed count for a delete.
import Foundation
import MermozKit
import Testing
@testable import Mermoz

/// Stands in for the daemon: answers with a fixed preview and records what ran.
private final class StubBackend: AskBackend, @unchecked Sendable {
    let preview: DoPreview
    private let lock = NSLock()
    private var _runs: [(String, Int?)] = []

    init(_ preview: DoPreview) { self.preview = preview }

    var runs: [(String, Int?)] { lock.withLock { _runs } }

    func doStatus() async throws -> DoAvailability { DoAvailability(available: true, providerName: "Work AI") }
    func doAsk(_ text: String) async throws -> DoPreview { preview }
    func doRun(_ id: String, confirm: Int?) async throws -> DoRunResult {
        lock.withLock { _runs.append((id, confirm)) }
        return DoRunResult(id: id, done: preview.total ?? 0, total: preview.total ?? 0, undoLabel: "Archive")
    }
    func doGet(_ id: String) async throws -> DoOperation {
        DoOperation(id: id, state: "running", done: 0, total: preview.total ?? 0)
    }
    func doUndo(_ id: String) async throws {}
}

private let sentence = "archive all messages past 5 months not in done"

private func plan(total: Int = 2, confirm: String = "enter") -> DoPreview {
    DoPreview(kind: "plan", id: "bop_1", summary: "Archive \(total) threads older than 5 months that aren't Done",
              interpretation: "status ≠ Done AND older than 5 months",
              steps: [DoStep(summary: "Archive \(total) threads older than 5 months that aren't Done",
                             interpretation: "status ≠ Done AND older than 5 months", action: "Archive", count: total,
                             byStatus: [DoStatusCount(status: "todo", name: "To do", count: total)],
                             sample: [DoSample(id: "t1", number: "M-1", status: "todo", subject: "Ferry tickets", lastFrom: "Nora Lind")])],
              total: total, deletes: confirm == "type_count", confirm: confirm)
}

/// The model with the stub's plan previewed, as after Enter on the Ask row.
@MainActor
private func previewed(_ p: DoPreview = plan()) async -> (AppModel, StubBackend) {
    let model = AppModel()
    let stub = StubBackend(p)
    await model.askFlow.start(sentence, backend: stub).value
    return (model, stub)
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
    await model.confirmAsk(backend: stub)?.value
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
    _ = model.confirmAsk(backend: stub)
    #expect(stub.runs.isEmpty)
}

@MainActor @Test func aQuestionIsShownInsteadOfAPlan() async {
    let (model, _) = await previewed(DoPreview(kind: "question", message: "Mermoz never sends, replies to or forwards mail for you."))
    #expect(model.askFlow.phase == .answer("Mermoz never sends, replies to or forwards mail for you."))
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
