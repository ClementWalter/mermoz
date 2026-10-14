/// The Feed layout: loading cards for the board scope, moving focus, running a card's
/// actions (drafts open in the composer, never sent), Later, and the cards' AI words and
/// suggestion chips, asked only for the cards on screen and the next few.
import AppKit
import Foundation
import MermozKit
import SwiftUI
import os

private let log = Logger(subsystem: "ai.mermoz.app", category: "feed")

extension AppModel {
    // MARK: loading

    /// The Feed layout is for mail views; People, Docs and the newsletters browser keep theirs.
    var showsFeed: Bool {
        guard layout == .feed, !isReview else { return false }
        switch selection {
        case .people, .docs: return false
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
            // The focused card keeps its place (an Unread card being read) until focus moves on.
            var next = FeedHold.keep(feedFocus, from: feed, in: f)
            for c in leaving where next.card(c.id) == nil {
                let was = feed?.sections.first { $0.key == c.section }?.cards.firstIndex { $0.id == c.id } ?? 0
                if let s = next.sections.firstIndex(where: { $0.key == c.section }) {
                    next.sections[s].cards.insert(c, at: min(was, next.sections[s].cards.count))
                }
            }
            if next != feed { feed = next }
            var nav = FeedNavigator(focus: feedFocus, pane: selectedThreadId)
            nav.refocus(in: next)
            if nav.focus != feedFocus { feedFocus = nav.focus }
            totalThreads = f.open
            feedSuggester.sync(with: next)
            requestVisibleTexts()
            requestSuggestions()
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
        requestSuggestions()
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

    // MARK: suggestion chips

    /// Asks for the suggestions of the cards on screen and the few after the focus, a
    /// moment after scrolling or j/k stops.
    func requestSuggestions() {
        feedSuggestTask?.cancel()
        feedSuggestTask = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled, let client, let feed else { return }
            do {
                try await feedSuggester.refresh(client: client, feed: feed, visible: feedVisible, focus: feedFocus)
            } catch { log.error("suggestions: \(error.localizedDescription)") }
        }
    }

    /// y or a click on the chip: files the thread where suggested; focus and the reading
    /// pane move on to the next card at once.
    func acceptChip(_ card: FeedCard) {
        guard let client, let feed else { return }
        var nav = FeedNavigator(focus: feedFocus, pane: selectedThreadId)
        guard let s = feedSuggester.accept(card, nav: &nav, in: feed) else { return }
        let label = "✦ " + SuggestionText.chip(s.suggestion, statusName: statusName)
        // A closing status takes the card away; otherwise it stays, without its chip.
        if let st = s.suggestion.status, ["done", "archive", "deleted", "backlog"].contains(st) {
            leave(card, label: label, nav: nav)
        } else {
            apply(nav)
        }
        answer("yes", s, of: card, label: label, client: client)
    }

    /// n: the chip goes and is not offered again for this sender; the card stays.
    func dismissChip(_ card: FeedCard) {
        guard let client, let s = feedSuggester.dismiss(card) else { return }
        answer("no", s, of: card, label: "Dismiss suggestion", client: client)
    }

    private func answer(_ verdict: String, _ s: ThreadSuggestion, of card: FeedCard, label: String, client: DaemonClient) {
        Task {
            do {
                let r = try await feedSuggester.send(verdict, s, of: card, client: client)
                undoStack.append(UndoEntry(label: label, before: [], answerUndo: r.undo, answered: s,
                                           answeredDate: card.thread.lastDate))
                if undoStack.count > 100 { undoStack.removeFirst() }
                if let learned = r.learned { toast = learned }
                refreshCounts()
            } catch {
                report(error)
                await loadFeed()
            }
        }
    }

    func statusName(_ key: String) -> String { statuses.first { $0.key == key }?.name ?? key }

    /// What the answers taught, for Settings › Developer.
    func loadLearned() async {
        guard let client else { return }
        do {
            let l = try await client.learned()
            if l != learned { learned = l }
        } catch { report(error) }
    }

    /// Forgets one rule or refusal: that sender's mail is asked about afresh.
    func forget(_ item: LearnedItem) async {
        guard let client else { return }
        learned.removeAll { $0.id == item.id }
        do { try await client.forgetLearned(item) } catch { report(error) }
        await loadLearned()
    }

    // MARK: focus

    /// j/k and the arrows: the next card, and its mail in the reading pane in the same
    /// turn (from memory when fetched before; the neighbours are fetched ahead).
    func feedMove(_ delta: Int) {
        guard let feed else { return }
        var nav = FeedNavigator(focus: feedFocus, pane: selectedThreadId)
        nav.move(by: delta, in: feed)
        apply(nav)
        requestSuggestions()
    }

    /// Puts focus and pane where the navigator says, together.
    func apply(_ nav: FeedNavigator) {
        if feedFocus != nav.focus { feedFocus = nav.focus }
        guard let id = nav.pane, let feed else { return }
        open(id, record: false)
        // Not fetched yet: the card's header and subject until its messages arrive.
        if detail?.thread.id != id, let card = feed.cards.first(where: { FeedSelection.thread(of: $0) == id }) {
            detail = .placeholder(card.members.first ?? card.thread)
        }
        prefetch(nav.neighbours(in: feed))
    }

    /// Fetches the cards around the focus ahead, off the main thread's way.
    func prefetch(_ ids: [String]) {
        prefetchTask?.cancel()
        let wanted = ids.filter { detailCache.get($0) == nil }
        guard !wanted.isEmpty, let client else { return }
        let window = detailWindow
        let full = shownInFull
        prefetchTask = Task {
            for id in wanted {
                guard !Task.isCancelled, let d = try? await client.thread(id, latest: window) else { continue }
                let messages = d.messages
                await Task.detached(priority: .utility) {
                    let bubbles = messages.map { (id: $0.id, content: BubbleContent.compact($0, showAll: full.contains($0.id))) }
                    BubbleTextCache.shared.prepare(bubbles)
                }.value
                detailCache.put(d, for: id)
            }
        }
    }

    /// Undo brought these threads back: their card is focused and in the pane again.
    func feedRestore(_ threadIds: [String]) {
        guard let feed, !threadIds.isEmpty else { return }
        var nav = FeedNavigator(focus: feedFocus, pane: selectedThreadId)
        nav.restore(threadIds, in: feed)
        apply(nav)
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
        case .later: openLater(for: card, typing: false)
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
                let d = try await client.thread(card.thread.id, latest: detailWindow)
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

    /// One undoable change for every conversation of a card (mermozd moves each one's
    /// open threads together), then the card folds away. Undo puts back every thread
    /// mermozd reports it changed.
    private func setStatus(of card: FeedCard, ids: [String], to status: String) {
        guard let client, !ids.isEmpty else { return }
        let before = ids.compactMap(feedThread)
        let name = statuses.first { $0.key == status }?.name ?? status
        let entry = UndoEntry(label: "Move to \(name)", before: before)
        undoStack.append(entry)
        leave(card, label: ids.count > 1 ? "\(name) · \(ids.count) mails" : name)
        Task {
            do {
                let reached = try await client.updateThreads(ids: ids, status: status)
                if !reached.isEmpty, let i = undoStack.firstIndex(where: { $0.token == entry.token }) {
                    undoStack[i].restore = reached
                }
                refreshCounts()
            } catch {
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

    /// Swaps the card for a "Done · Undo" strip, then folds it away and moves focus on
    /// (to `nav` when the caller already moved it).
    func leave(_ card: FeedCard, label: String, nav moved: FeedNavigator? = nil) {
        // The next card and its mail, in the same turn as the action.
        if let feed {
            var nav = FeedNavigator(focus: feedFocus, pane: selectedThreadId)
            if let moved { nav = moved } else { nav.leave(card.id, in: feed) }
            if nav != FeedNavigator(focus: feedFocus, pane: selectedThreadId) { apply(nav) }
        }
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
