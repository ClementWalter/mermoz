/// The Sort queue: one email at a time with the AI's suggestion. Yes files it, No
/// leaves it (and is never suggested again for that sender), Change files it elsewhere,
/// Skip sets it aside. Cards are prepared a few ahead, never in bulk.
import Foundation
import MermozKit
import os

private let log = Logger(subsystem: "ai.mermoz.app", category: "sort")

extension AppModel {
    /// Opens the queue, for every open column or only `columns`.
    func openSort(_ columns: [String] = []) {
        sortColumns = columns
        sortQueue.reset()
        select(.sort)
    }

    /// Counts per column and what was learned, for the sidebar and column buttons.
    func refreshSortInfo() async {
        guard let client else { return }
        do {
            let info = try await client.sortState()
            if info != sortInfo { sortInfo = info }
        } catch { log.error("sort state: \(error.localizedDescription)") }
    }

    /// Fills the hand up to a few cards and shows the first.
    func loadSortQueue() async {
        await refreshSortInfo()
        await fillSortQueue()
        if selectedThreadId != sortQueue.current?.id { showCurrentCard() }
    }

    /// Asks for the next cards while fewer than a few are in hand.
    func fillSortQueue() async {
        guard let client, !sortLoading, sortQueue.needsMore else { return }
        sortLoading = true
        defer { sortLoading = false }
        do {
            let batch = try await client.sortNext(statuses: sortColumns, exclude: sortQueue.exclude)
            sortQueue.add(batch.cards)
            sortInfo.paused = batch.paused
        } catch { report(error) }
    }

    /// The card on screen opens in the reading pane.
    func showCurrentCard() {
        open(sortQueue.current?.id, record: false)
    }

    func sortYes() { answerSort("yes", label: "Yes") }

    func sortNo() { answerSort("no", label: "No") }

    /// Another destination: counts as a No for the suggestion and a Yes for this one.
    func sortChange(status: String? = nil, project: String? = nil) {
        guard status != nil || project != nil else { return }
        answerSort("change", status: status, projects: project.map { [$0] } ?? [], label: "Change")
    }

    /// Sets the card aside for this session; it comes back another time.
    func sortSkip() {
        guard sortQueue.current != nil else { return }
        sortQueue.skip()
        showCurrentCard()
        Task { await fillSortQueue(); if selectedThreadId == nil { showCurrentCard() } }
    }

    private func answerSort(_ answer: String, status: String? = nil, projects: [String] = [], label: String) {
        guard let client, let card = sortQueue.current else { return }
        sortQueue.answer()
        showCurrentCard()
        sortInfo.toSort = max(0, sortInfo.toSort - 1)
        Task {
            do {
                let r = try await client.sortAnswer(card.id, answer: answer, status: status, projects: projects)
                undoStack.append(UndoEntry(label: label, before: [], sortUndo: r.undo, sortCard: card))
                if undoStack.count > 100 { undoStack.removeFirst() }
                if let learned = r.learned { toast = learned }
            } catch {
                // The answer did not land: the card comes back on screen.
                sortQueue.putBack(card)
                showCurrentCard()
                report(error)
            }
            await fillSortQueue()
            if selectedThreadId == nil { showCurrentCard() }
            await refreshSortInfo()
        }
    }
}
