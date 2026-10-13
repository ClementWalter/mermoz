/// Later everywhere (Feed card, List, Board): open the picker, put mail off, undo. The
/// mail leaves its column and comes back Unread at that time (mermozd's Later loop).
import Foundation
import MermozKit
import SwiftUI

/// What the Later picker is for: these threads, from this Feed card (nil: List or
/// Board), opened on the typing field (⇧L) or on the quick choices (l).
struct LaterRequest: Equatable {
    var ids: [String]
    var cardId: String?
    var typing: Bool
}

extension AppModel {
    /// l / ⇧L on a Feed card.
    func openLater(for card: FeedCard, typing: Bool) {
        feedFocus = card.id
        later = LaterRequest(ids: card.threadIds, cardId: card.id, typing: typing)
    }

    /// l / ⇧L, or ⌘K "Later…", on the selected threads in the List or Board.
    func openLaterForSelection(typing: Bool) {
        let ids = targetIds
        guard !ids.isEmpty else { toast = "Select a conversation first"; return }
        later = LaterRequest(ids: ids, cardId: nil, typing: typing)
    }

    func laterOptions() async -> [LaterChoice] {
        (try? await client?.laterOptions()) ?? []
    }

    func laterParse(_ text: String) async -> LaterTime? {
        guard !text.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return try? await client?.laterParse(text) ?? nil
    }

    /// Puts the request's threads off until `until` (Unix ms). `when` is shown now
    /// ("Later · Fri 18:00"); `label` comes back with the mail ("You set this for …").
    func putOff(_ r: LaterRequest, until: Int64, when: String, label: String) {
        guard let client else { return }
        later = nil
        undoStack.append(UndoEntry(label: "Later", before: [], unsnooze: r.ids))
        if let id = r.cardId, let card = feed?.card(id) {
            leave(card, label: LaterKeys.strip(when))
        } else {
            // Like a status change: the threads leave the column, the next one opens.
            let order = layout == .list ? displayOrder : threads
            let idx = order.firstIndex { r.ids.contains($0.id) }
            threads.removeAll { r.ids.contains($0.id) }
            if let idx, let sel = selectedThreadId, r.ids.contains(sel) {
                let rest = order.filter { !r.ids.contains($0.id) }
                open(rest.isEmpty ? nil : rest[min(idx, rest.count - 1)].id, record: false)
            }
            toast = "\(LaterKeys.strip(when)) · ⌘Z to undo"
        }
        Task {
            do { try await client.snooze(r.ids, untilMs: until, label: LaterKeys.hint(label)) } catch {
                report(error)
                await loadList()
            }
        }
    }
}
