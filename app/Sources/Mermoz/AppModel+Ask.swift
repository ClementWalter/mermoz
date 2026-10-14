/// "Ask Mermoz to do": the palette's plan flow wired to the daemon, the toast after a
/// run, and the single Undo entry that reverts the whole run.
import MermozKit

extension AppModel {
    /// Enter on the "Ask Mermoz to do" row: asks for a plan, or says an AI is needed.
    func startAsk(_ text: String, backend: AskBackend? = nil) {
        guard let b = backend ?? client else { return }
        askFlow.start(text, backend: b)
    }

    /// Enter while a plan or a question shows: runs, answers, picks or starts over.
    @discardableResult
    func enterAsk(backend: AskBackend? = nil) -> Task<Void, Never>? {
        guard let b = backend ?? client else { return nil }
        return askFlow.enter(backend: b) { [weak self] r in self?.didRunAsk(r) }
    }

    /// A choice clicked (or picked with ↩): re-plans with exactly that filter.
    @discardableResult
    func pickAsk(_ c: DoChoice, backend: AskBackend? = nil) -> Task<Void, Never>? {
        guard let b = backend ?? client else { return nil }
        return askFlow.pick(c, backend: b)
    }

    /// Enter on a previewed plan: runs it, then offers one Undo for all of it.
    @discardableResult
    func confirmAsk(backend: AskBackend? = nil) -> Task<Void, Never>? {
        guard let b = backend ?? client else { return nil }
        return askFlow.confirm(backend: b) { [weak self] r in self?.didRunAsk(r) }
    }

    func didRunAsk(_ r: DoRunResult?) {
        guard let r else { return }
        undoStack.append(UndoEntry(label: r.undoLabel, before: [], doOperation: r.id))
        if undoStack.count > 100 { undoStack.removeFirst() }
        let n = AskIntent.count(r.done)
        toast = "\(r.undoLabel): \(n) thread\(r.done == 1 ? "" : "s") · ⌘Z to undo"
        paletteMode = nil
        askFlow.query = ""
        refreshCounts()
        Task { await loadList() }
    }
}
