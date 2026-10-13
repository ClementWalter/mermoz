/// The Feed layout: loading cards for the board scope, moving focus, running a card's
/// actions (drafts open in the composer, never sent), Later, and the cards' AI words,
/// asked only for the cards on screen.
import AppKit
import Foundation
import MermozKit
import SwiftUI
import os

private let log = Logger(subsystem: "ai.mermoz.app", category: "feed")

extension AppModel {
    // MARK: loading

    /// The Feed layout is for mail views; People, Docs, the newsletters browser and Sort keep theirs.
    var showsFeed: Bool {
        guard layout == .feed, !isReview else { return false }
        switch selection {
        case .people, .docs, .feeds, .sort: return false
        default: return true
        }
    }

    /// The feed follows the sidebar: a project's or an account's work, else everything open.
    private var feedScope: (project: String?, accountId: String?) {
        switch selection {
        case let .project(id): (id, nil)
        case let .account(id): (nil, id)
        default: (nil, nil)
        }
    }

    func loadFeed() async {
        guard let client else { return }
        let started = ContinuousClock.now
        // The first load is timed to its first card on screen (see `cardAppeared`).
        if feed == nil, feedOpening == nil { feedOpening = started }
        do {
            let scope = feedScope
            let f = try await client.feed(project: scope.project, accountId: scope.accountId)
            log.info("feed fetched: \(f.cards.count) cards in \((ContinuousClock.now - started).ms) ms")
            // Cards folding away stay until their strip is gone, so nothing jumps.
            let leaving = feed?.cards.filter { feedStrips[$0.id] != nil } ?? []
            var next = f
            for c in leaving where next.card(c.id) == nil {
                let was = feed?.sections.first { $0.key == c.section }?.cards.firstIndex { $0.id == c.id } ?? 0
                if let s = next.sections.firstIndex(where: { $0.key == c.section }) {
                    next.sections[s].cards.insert(c, at: min(was, next.sections[s].cards.count))
                }
            }
            if next != feed { feed = next }
            let ids = next.cards.map(\.id)
            if feedFocus == nil || !ids.contains(feedFocus!) { feedFocus = ids.first }
            totalThreads = f.open
            requestVisibleTexts()
        } catch { report(error) }
    }

    /// Coalesces bursts of `feed_changed` (a judging round, words arriving) into one reload.
    func scheduleFeedRefresh() {
        feedRefreshTask?.cancel()
        feedRefreshTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            await loadFeed()
        }
    }

    /// Every thread the feed shows, for undo snapshots.
    func feedThread(_ id: String) -> ThreadSummary? {
        feed?.cards.lazy.flatMap { [$0.thread] + $0.members }.first { $0.id == id }
    }

    var focusedCard: FeedCard? { feed?.card(feedFocus) }

    // MARK: card words

    func cardAppeared(_ id: String) {
        if let opening = feedOpening {
            log.info("feed on screen after \((ContinuousClock.now - opening).ms) ms")
            feedOpening = nil
        }
        feedVisible.insert(id)
        requestVisibleTexts()
    }

    func cardDisappeared(_ id: String) { feedVisible.remove(id) }

    /// Asks the AI for the words of the cards on screen, a moment after scrolling stops.
    func requestVisibleTexts() {
        feedTextsTask?.cancel()
        feedTextsTask = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled, let client, let feed else { return }
            let ids = FeedTextsRequest.ids(visible: feed.cards.filter { feedVisible.contains($0.id) }, asked: feedAsked)
            guard !ids.isEmpty else { return }
            feedAsked.formUnion(ids)
            do { try await client.feedTexts(ids) } catch {
                feedAsked.subtract(ids)
                report(error)
            }
        }
    }

    // MARK: focus

    /// j/k: the next card at once, and its mail in the reading pane as soon as the keys
    /// pause, so holding j glides through the cards instead of loading every mail.
    func feedMove(_ delta: Int) {
        guard let feed else { return }
        feedFocus = FeedFocus.step(feed.cards.map(\.id), from: feedFocus, by: delta)
        guard let card = feed.card(feedFocus) else { return }
        feedPaneTask?.cancel()
        feedPaneTask = Task {
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            open(FeedSelection.thread(of: card), record: false)
        }
    }

    func focus(_ card: FeedCard) { feedFocus = card.id }

    // MARK: actions

    /// ↩ on the focused card.
    func runPrimary(_ card: FeedCard) {
        run(FeedActions.primary(card), on: card)
    }

    /// 1–3 on a decision card.
    func runOption(_ n: Int, on card: FeedCard) {
        if let a = FeedActions.option(n, of: card) { run(a, on: card) }
    }

    func run(_ action: FeedAction, on card: FeedCard) {
        switch action {
        case let .draft(body): draftReply(to: card, body: body)
        case .open: openCard(card)
        case .addToCalendar: addToCalendar(card)
        case let .archive(ids):
            setStatus(of: card, ids: ids.filter { !feedKept.contains($0) }, to: "archive")
            teach(card)
        case .done: setStatus(of: card, ids: card.threadIds, to: "done")
        case .later: later(card, until: FeedLater.tomorrowMorning(after: Date()))
        }
    }

    /// Opens the mail on the right, like a click on a row.
    func openCard(_ card: FeedCard) {
        feedFocus = card.id
        open(FeedSelection.thread(of: card))
    }

    /// Opens the composer on the card's thread with the AI's words in it; the user sends.
    func draftReply(to card: FeedCard, body: String) {
        guard let client else { return }
        feedFocus = card.id
        Task {
            do {
                let d = try await client.thread(card.thread.id)
                selectedThreadId = d.thread.id
                detail = d
                startReply()
                compose?.body = body
            } catch { report(error) }
        }
    }

    /// "Ask about this" (t): the "Chat about this" panel on the card's thread.
    func askAbout(_ card: FeedCard) {
        feedFocus = card.id
        open(card.thread.id)
        openChat()
    }

    /// e on a card: Done (a bill: paid). a: Archive. A batch applies to its ticked mails.
    func done(_ card: FeedCard) {
        setStatus(of: card, ids: card.threadIds.filter { !feedKept.contains($0) }, to: card.isBatch ? "archive" : "done")
    }

    /// d / ⌫: to the Trash, like ⌫ elsewhere; the strip says "Deleted · Undo".
    func delete(_ card: FeedCard) {
        setStatus(of: card, ids: card.threadIds.filter { !feedKept.contains($0) }, to: "deleted")
    }

    func archive(_ card: FeedCard) {
        setStatus(of: card, ids: card.threadIds.filter { !feedKept.contains($0) }, to: "archive")
        teach(card)
    }

    /// Archiving a single automated mail is a yes to "Archive" for its sender; three
    /// make a rule, and that sender's mail is then cleared in the batch.
    private func teach(_ card: FeedCard) {
        guard !card.isBatch, card.kind == .cleanup || card.kind == .reading, let client else { return }
        Task { try? await client.learnArchive([card.thread.id]) }
    }

    /// One undoable change for every thread of a card, then the card folds away.
    private func setStatus(of card: FeedCard, ids: [String], to status: String) {
        guard let client, !ids.isEmpty else { return }
        let before = ids.compactMap(feedThread)
        let name = statuses.first { $0.key == status }?.name ?? status
        undoStack.append(UndoEntry(label: "Move to \(name)", before: before))
        leave(card, label: ids.count > 1 ? "\(name) · \(ids.count) mails" : name)
        Task {
            do {
                try await client.updateThreads(ids: ids, status: status)
                refreshCounts()
            } catch {
                report(error)
                await loadFeed()
            }
        }
    }

    /// Later (l, or ⇧L with a day): out of the feed until then or until new mail.
    func later(_ card: FeedCard, until: Date) {
        guard let client else { return }
        let ids = card.threadIds
        undoStack.append(UndoEntry(label: "Later", before: [], unsnooze: ids))
        let words = Calendar.current.isDateInTomorrow(until)
            ? "tomorrow" : until.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated))
        leave(card, label: "Later · back \(words)")
        Task {
            do { try await client.snooze(ids, until: until) } catch {
                report(error)
                await loadFeed()
            }
        }
    }

    /// Writes the event as a calendar file and hands it to Calendar, which asks where to add it.
    func addToCalendar(_ card: FeedCard) {
        guard let due = card.due,
              let ics = CalendarFile.ics(title: card.title, date: due, time: card.dueTime, uid: card.thread.id)
        else { return openCard(card) }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("mermoz-calendar", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let file = dir.appendingPathComponent("\(card.thread.number).ics")
            try ics.write(to: file, atomically: true, encoding: .utf8)
            NSWorkspace.shared.open(file)
            toast = "Opening Calendar · press e when it's done"
        } catch { report(error) }
    }

    /// Swaps the card for a "Done · Undo" strip, then folds it away and moves focus on.
    private func leave(_ card: FeedCard, label: String) {
        let ids = feed?.cards.map(\.id) ?? []
        if feedFocus == card.id { feedFocus = FeedFocus.after(removing: card.id, from: ids) }
        withAnimation(.easeInOut(duration: 0.25)) { feedStrips[card.id] = label }
        Task {
            try? await Task.sleep(for: .milliseconds(1300))
            guard feedStrips[card.id] != nil else { return }
            withAnimation(.easeInOut(duration: 0.25)) {
                feedStrips[card.id] = nil
                if let s = feed?.sections.firstIndex(where: { $0.cards.contains { $0.id == card.id } }) {
                    feed?.sections[s].cards.removeAll { $0.id == card.id }
                    feed?.sections.removeAll { $0.cards.isEmpty }
                }
            }
        }
    }

    /// The strip's Undo: the same as ⌘Z, for the card it stands for.
    func undoStrip(_ id: String) {
        feedStrips[id] = nil
        feedFocus = id
        undo()
    }

    // MARK: batches

    func toggleKept(_ id: String) {
        if feedKept.contains(id) { feedKept.remove(id) } else { feedKept.insert(id) }
    }
}

extension Duration {
    /// Whole milliseconds, for timing logs.
    var ms: Int64 { components.seconds * 1000 + components.attoseconds / 1_000_000_000_000_000 }
}
