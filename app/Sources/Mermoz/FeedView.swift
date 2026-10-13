/// Feed layout: everything open as one ordered stream of cards, one thing to do each,
/// with the next step one key away. Order and sections come from mermozd; the AI's
/// words fill in as cards come into view.
import MermozKit
import SwiftUI

struct FeedView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                Text(title).font(.system(size: 15, weight: .semibold))
                if let f = model.feed { Text(f.open.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3) }
                Spacer()
                Picker("", selection: $model.layout) {
                    ForEach(LayoutMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden().frame(width: 170)
                .onChange(of: model.layout) { Task { await model.loadList() } }
                Button { model.startNew() } label: { Image(systemName: "square.and.pencil") }
                    .buttonStyle(.plain).foregroundStyle(Theme.text2)
                    .help("New message (c)").accessibilityLabel("New message")
            }
            .padding(.horizontal, 16)
            .frame(height: 50)
            .glassPanel()

            ScrollViewReader { proxy in
                ScrollView {
                    FeedCards(feed: model.feed, strips: model.feedStrips)
                        .equatable()
                        .frame(maxWidth: 680)
                        .padding(.horizontal, 18).padding(.vertical, 16)
                        .frame(maxWidth: .infinity)
                }
                .scrollIndicators(.automatic)
                .onScrollTargetVisibilityChange(idType: String.self, threshold: 0.99) { model.feedFullyVisible = Set($0) }
                .background { FeedScroller(proxy: proxy) }
            }
            .listPanel()
            .layoutBoundary()
        }
    }

    private var title: String {
        switch model.selection {
        case let .project(id): model.project(id)?.name ?? "Project"
        case let .account(id): model.accounts.first { $0.id == id }?.email ?? "Account"
        default: "All open"
        }
    }
}

/// The Feed's cards. Plain-value inputs, so a redraw around them that leaves the feed as
/// it was skips them; focus reaches each card through its own flag.
struct FeedCards: View, Equatable {
    @Environment(AppModel.self) private var model
    let feed: Feed?
    let strips: [String: String]

    nonisolated static func == (a: FeedCards, b: FeedCards) -> Bool { a.feed == b.feed && a.strips == b.strips }

    var body: some View {
        LazyVStack(alignment: .leading, spacing: 10) {
            if let f = feed {
                FeedBrief(feed: f)
                ForEach(f.sections) { s in
                    FeedSectionHeader(section: s)
                    ForEach(s.cards) { c in
                        // Equatable: only the cards that changed render again on j/k.
                        FeedCardView(card: c, focus: model.feedFocusCells.flag(c.id), strip: strips[c.id])
                            .equatable()
                            .id(c.id)
                            .onAppear { model.cardAppeared(c.id) }
                            .onDisappear { model.cardDisappeared(c.id) }
                    }
                    if s.more > 0 {
                        Text("\(s.more) more here · the List shows them all")
                            .font(.system(size: 12)).foregroundStyle(Theme.text3).padding(.horizontal, 4)
                    }
                }
                if f.cards.isEmpty { AllClear() }
            } else {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity).padding(40)
            }
        }
        // Reports which rows show in full, so the selection scrolls only to reveal one.
        .scrollTargetLayout()
    }
}

/// Keeps the focused card in view. It alone observes the focus, so j/k does not redraw
/// the Feed around it; it scrolls only to reveal a card not fully shown.
struct FeedScroller: View {
    @Environment(AppModel.self) private var model
    let proxy: ScrollViewProxy

    var body: some View {
        Color.clear
            .onChange(of: model.feedFocus) { _, id in
                if let id, !model.feedFullyVisible.contains(id) { proxy.scrollTo(id) }
            }
    }
}

/// Today's date and what is left, in one sentence.
struct FeedBrief: View {
    let feed: Feed

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(Date().formatted(.dateTime.weekday(.wide).day().month(.wide)))
                .font(.system(size: 21, weight: .semibold, design: .serif))
            Text(FeedHeader.line(feed)).font(.system(size: 13.5)).foregroundStyle(Theme.text2)
            if feed.sorting && feed.paused == nil {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("Sorting your mail · \(feed.judged) of \(feed.open)")
                }
                .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
            }
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 2).padding(.bottom, 4)
    }
}

struct FeedSectionHeader: View {
    let section: FeedSection

    var body: some View {
        HStack(spacing: 8) {
            Text(section.title).font(.system(size: 11, weight: .semibold)).kerning(0.7).textCase(.uppercase)
            Text("\(section.cards.count)").font(.system(size: 11)).monospacedDigit()
            Rectangle().fill(Theme.line).frame(height: 1)
        }
        .foregroundStyle(Theme.text3)
        .padding(.top, 10).padding(.horizontal, 2)
    }
}

struct AllClear: View {
    var body: some View {
        VStack(spacing: 6) {
            Image(systemName: "checkmark.circle").font(.system(size: 26, weight: .light))
            Text("All clear. New mail lands here as it arrives.").font(.system(size: 13))
        }
        .foregroundStyle(Theme.text3)
        .frame(maxWidth: .infinity).padding(40)
    }
}

// MARK: card shell

struct FeedCardView: View, Equatable {
    @Environment(AppModel.self) private var model
    let card: FeedCard
    /// The card's own focus flag: j/k redraws only the cards it leaves and reaches.
    let focus: SelectionFlag
    /// "Done · Undo" shown in place of the card for a moment after it was handled.
    let strip: String?

    nonisolated static func == (a: FeedCardView, b: FeedCardView) -> Bool {
        a.focus === b.focus && a.strip == b.strip && a.card == b.card
    }

    private var focused: Bool { focus.on }

    var body: some View {
        if let label = strip {
            DoneStripView(label: label) { model.undoStrip(card.id) }
                .transition(.asymmetric(insertion: .opacity, removal: .opacity.combined(with: .scale(scale: 0.98, anchor: .top))))
        } else {
            shell
                .transition(.asymmetric(insertion: .opacity, removal: .move(edge: .trailing).combined(with: .opacity)))
        }
    }

    private var shell: some View {
        VStack(alignment: .leading, spacing: 8) {
            CardHead(card: card)
            if card.kind != .waiting {
                Text(card.title).font(.system(size: 15, weight: .semibold)).lineLimit(2)
            }
            if let why = card.why ?? card.texts?.why, !why.isEmpty, card.kind != .waiting {
                Label { Text(why) } icon: { Image(systemName: "sparkle").foregroundStyle(Theme.accent) }
                    .font(.system(size: 13)).foregroundStyle(Theme.text2)
            }
            CardBody(card: card)
            CardActions(card: card, focused: focused)
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 14).padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.92), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(focused ? Theme.accent : Theme.line, lineWidth: focused ? 2 : 1))
        .shadow(color: Theme.accent.opacity(focused ? 0.10 : 0), radius: 12, y: 6)
        .contentShape(Rectangle())
        // Like a row in the List: a click shows the mail in the reading pane.
        .onTapGesture { model.openCard(card) }
    }
}

/// Kind label, who it is from, project, due date and age.
struct CardHead: View {
    @Environment(AppModel.self) private var model
    let card: FeedCard

    var body: some View {
        HStack(spacing: 7) {
            Label(card.kind.label, systemImage: card.kind.symbol)
                .font(.system(size: 10.5, weight: .semibold)).kerning(0.5).textCase(.uppercase)
                .foregroundStyle(Color(hex: card.kind.color))
                .labelStyle(.titleAndIcon)
            if !card.isBatch {
                Avatar(name: card.person, size: 20)
                Text(card.person).font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.text2).lineLimit(1)
                ProjectChips(names: card.thread.projects, limit: 1)
            }
            if card.fromBacklog {
                Chip(small: true) { Image(systemName: "arrow.uturn.up"); Text("Back from Backlog") }
                    .foregroundStyle(Theme.accent)
            }
            Spacer(minLength: 4)
            if let due = card.due {
                DueChip(text: "Due \(FeedWords.due(due))", late: (FeedWords.day(due) ?? .distantFuture) < Calendar.current.startOfDay(for: Date()))
            }
            Text(Format.short(card.thread.lastDate)).font(.system(size: 11)).foregroundStyle(Theme.text3)
        }
        .lineLimit(1)
    }
}

struct DueChip: View {
    let text: String
    let late: Bool

    var body: some View {
        let c = late ? Theme.danger : Color(hex: "#B8741C")
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(c)
            .padding(.horizontal, 6).frame(height: 18)
            .background(c.opacity(0.10), in: RoundedRectangle(cornerRadius: 5))
            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(c.opacity(0.3)))
    }
}

// MARK: bodies (the recommended variant of each card type)

struct CardBody: View {
    @Environment(AppModel.self) private var model
    let card: FeedCard

    /// The mail's gist under the card, unless it repeats the "why" line just above.
    private func gist(_ text: String) -> some View {
        Quote(text: FeedWords.nearDuplicate(card.why ?? card.texts?.why, text) ? "" : text)
    }

    var body: some View {
        switch card.kind {
        case .reply, .promise, .intro:
            if card.kind == .intro { PersonLine(card: card) }
            if let d = card.texts?.draft, !d.isEmpty { DraftPeek(label: "Draft reply", text: d) } else { gist(card.thread.snippet) }
        case .decision:
            if let options = card.texts?.options, !options.isEmpty { OptionTiles(card: card, options: options) } else { gist(card.thread.snippet) }
        case .sign:
            Points(points: card.texts?.points ?? [])
            if card.thread.hasAttachments { FileLine(text: "Attachment to sign in the mail") }
        case .event:
            CalendarTile(card: card)
        case .payment:
            AmountTicket(card: card)
        case .waiting:
            WaitingLine(card: card)
        case .cleanup, .reading:
            // A batch lists its mails; a single one (sender not learned yet) shows its gist.
            if !card.isBatch {
                gist(card.texts?.summary.isEmpty == false ? card.texts!.summary : card.thread.snippet)
            } else if card.kind == .cleanup {
                CleanupGroups(card: card)
            } else {
                ReadingLines(card: card)
            }
        case .other:
            gist(card.texts?.summary.isEmpty == false ? card.texts!.summary : card.thread.snippet)
        }
    }
}

struct Quote: View {
    let text: String

    var body: some View {
        if !text.isEmpty {
            Text(text).font(.system(size: 13)).foregroundStyle(Theme.text2).lineLimit(3)
                .padding(.leading, 10)
                .overlay(alignment: .leading) { Rectangle().fill(Theme.line).frame(width: 2) }
        }
    }
}

/// Two lines of the draft the AI prepared; ↩ opens it in the composer.
struct DraftPeek: View {
    let label: String
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Label(label, systemImage: "sparkle")
                .font(.system(size: 10.5, weight: .semibold)).kerning(0.5).textCase(.uppercase)
                .foregroundStyle(Theme.accent)
            // Paragraph breaks would waste the two lines on a greeting.
            Text(text.split(whereSeparator: \.isNewline).joined(separator: " ")).font(.system(size: 13)).lineLimit(2)
        }
        .padding(.horizontal, 10).padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.accent.opacity(0.35), style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
    }
}

/// The options of a decision; the AI's pick is outlined with its reason. 1–3 pick one.
struct OptionTiles: View {
    @Environment(AppModel.self) private var model
    let card: FeedCard
    let options: [FeedChoice]

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            ForEach(Array(options.prefix(3).enumerated()), id: \.offset) { i, o in
                Button { model.runOption(i + 1, on: card) } label: {
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 6) { KeyHint("\(i + 1)"); Text(o.label).font(.system(size: 13, weight: .semibold)) }
                        if !o.detail.isEmpty { Text(o.detail).font(.system(size: 11.5)).foregroundStyle(Theme.text3) }
                        if o.recommended, !o.reason.isEmpty {
                            Label(o.reason, systemImage: "sparkle").font(.system(size: 11.5)).foregroundStyle(Theme.accent)
                        }
                    }
                    .padding(.horizontal, 10).padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
                    .background(o.recommended ? Theme.accentSoft.opacity(0.6) : Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(o.recommended ? Theme.accent : Theme.line))
                }
                .buttonStyle(.plain)
                .help("Opens a reply choosing \(o.label) for you to check")
            }
        }
    }
}

struct Points: View {
    let points: [String]

    var body: some View {
        if !points.isEmpty {
            VStack(alignment: .leading, spacing: 3) {
                ForEach(points, id: \.self) { p in
                    HStack(alignment: .firstTextBaseline, spacing: 6) { Text("•"); Text(p) }
                }
            }
            .font(.system(size: 13))
        }
    }
}

struct FileLine: View {
    let text: String

    var body: some View {
        Label(text, systemImage: "paperclip")
            .font(.system(size: 12.5)).foregroundStyle(Theme.text2)
            .padding(.horizontal, 9).padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.line))
    }
}

/// Day and time of an event, as a calendar page.
struct CalendarTile: View {
    let card: FeedCard

    var body: some View {
        HStack(spacing: 12) {
            if let due = card.due, let d = FeedWords.day(due) {
                VStack(spacing: 0) {
                    Text(d.formatted(.dateTime.month(.abbreviated)).uppercased())
                        .font(.system(size: 10, weight: .bold)).kerning(0.8).foregroundStyle(.white)
                        .frame(maxWidth: .infinity).padding(.vertical, 2).background(Theme.danger)
                    Text(d.formatted(.dateTime.day())).font(.system(size: 22, weight: .semibold, design: .serif))
                    Text(d.formatted(.dateTime.weekday(.abbreviated))).font(.system(size: 10)).foregroundStyle(Theme.text3).padding(.bottom, 3)
                }
                .frame(width: 54)
                .background(Color.white, in: RoundedRectangle(cornerRadius: 10))
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.line))
            }
            VStack(alignment: .leading, spacing: 3) {
                if let t = card.dueTime { Text(t).font(.system(size: 13, weight: .semibold)) }
                Points(points: card.texts?.points ?? [])
                if card.texts?.points.isEmpty ?? true { Text(card.thread.snippet).font(.system(size: 13)).foregroundStyle(Theme.text2).lineLimit(2) }
            }
        }
    }
}

/// The amount first, as written in the mail.
struct AmountTicket: View {
    let card: FeedCard

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 16) {
            if let a = card.amount { Text(a).font(.system(size: 26, weight: .semibold, design: .serif)).monospacedDigit() }
            VStack(alignment: .leading, spacing: 2) {
                Points(points: card.texts?.points ?? [])
                if card.texts?.points.isEmpty ?? true { Text(card.thread.snippet).font(.system(size: 12.5)).foregroundStyle(Theme.text2).lineLimit(2) }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.line))
    }
}

struct PersonLine: View {
    let card: FeedCard

    var body: some View {
        HStack(spacing: 10) {
            Avatar(name: card.person, size: 36)
            VStack(alignment: .leading, spacing: 1) {
                Text(card.person).font(.system(size: 14, weight: .semibold))
                if let s = card.texts?.summary, !s.isEmpty { Text(s).font(.system(size: 12.5)).foregroundStyle(Theme.text2).lineLimit(2) }
            }
        }
    }
}

/// Waiting stays quiet: one line on how long, and how long they usually take.
struct WaitingLine: View {
    let card: FeedCard

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(card.title).font(.system(size: 14, weight: .semibold)).lineLimit(1)
            Text(card.reasons.first ?? "").font(.system(size: 12.5)).foregroundStyle(Theme.text3)
        }
    }
}

/// Automated mail grouped by sender; untick a sender to keep its mail.
struct CleanupGroups: View {
    @Environment(AppModel.self) private var model
    let card: FeedCard

    var body: some View {
        let groups = Dictionary(grouping: card.members, by: \.lastFrom).sorted { $0.value.count > $1.value.count }
        VStack(spacing: 0) {
            ForEach(groups, id: \.key) { sender, mails in
                let kept = mails.allSatisfy { model.feedKept.contains($0.id) }
                HStack(spacing: 9) {
                    Toggle("", isOn: Binding(get: { !kept }, set: { _ in for m in mails { model.toggleKept(m.id) } }))
                        .toggleStyle(.checkbox).labelsHidden()
                    Text("\(mails.count)").font(.system(size: 13, weight: .semibold)).monospacedDigit().frame(width: 18, alignment: .trailing)
                    Text(sender).font(.system(size: 13)).lineLimit(1)
                    Text(mails.map(\.subject).joined(separator: " · ")).font(.system(size: 12)).foregroundStyle(Theme.text3).lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 10).padding(.vertical, 6)
                .opacity(kept ? 0.5 : 1)
                if sender != groups.last?.key { Divider().overlay(Theme.line) }
            }
        }
        .background(Color.white, in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.line))
    }
}

/// One line per newsletter: the AI's gist when written, else the subject.
struct ReadingLines: View {
    @Environment(AppModel.self) private var model
    let card: FeedCard

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(card.members.enumerated()), id: \.element.id) { i, t in
                let line = card.texts?.lines?[safe: i].flatMap { $0.isEmpty ? nil : $0 } ?? t.subject
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text(t.lastFrom).font(.system(size: 12, weight: .semibold)).frame(width: 120, alignment: .leading).lineLimit(1)
                    Text(line).font(.system(size: 13)).foregroundStyle(Theme.text2).lineLimit(2)
                    Spacer(minLength: 0)
                    Button { model.open(t.id) } label: { Text("Read") }.buttonStyle(.link).font(.system(size: 12))
                }
                .padding(.horizontal, 10).padding(.vertical, 6)
                if i < card.members.count - 1 { Divider().overlay(Theme.line) }
            }
        }
        .background(Color.white, in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.line))
    }
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}

// MARK: actions

struct CardActions: View {
    @Environment(AppModel.self) private var model
    let card: FeedCard
    let focused: Bool
    @State private var why = false

    var body: some View {
        HStack(spacing: 6) {
            // Key hints keep their room on every card and only show on the focused one: focus
            // moving then changes no button's size, so no card is laid out again.
            Button { model.runPrimary(card) } label: {
                HStack(spacing: 6) { Text(FeedActions.primaryLabel(card)); KeyHint("↩").opacity(focused ? 1 : 0) }
            }
            .buttonStyle(.glassProminent).tint(Theme.accent).controlSize(.small)
            if card.kind == .payment {
                secondary("Paid", key: "e") { model.done(card) }
            } else if !card.isBatch {
                secondary("Done", key: "e") { model.done(card) }
            }
            if !card.isBatch, card.kind != .cleanup { secondary("Archive", key: "a") { model.archive(card) } }
            // Promotions and newsletters are often just binned: Delete is in plain sight there.
            if card.kind == .cleanup || card.kind == .reading { secondary("Delete", key: "d") { model.delete(card) } }
            Spacer(minLength: 4)
            quiet("Later", key: "l") { model.openLater(for: card, typing: false) }
                .popover(isPresented: Binding(get: { model.later?.cardId == card.id },
                                              set: { if !$0, model.later?.cardId == card.id { model.later = nil } }),
                         arrowEdge: .bottom) {
                    if let r = model.later { LaterPicker(request: r) }
                }
            if card.kind != .cleanup, card.kind != .reading { quiet("Delete", key: "d") { model.delete(card) } }
            if !card.isBatch { quiet("Ask", key: "t") { model.askAbout(card) } }
            Button("Why?") { why.toggle() }
                .buttonStyle(.plain).font(.system(size: 12)).foregroundStyle(Theme.text3)
                .popover(isPresented: $why, arrowEdge: .bottom) { WhyPopover(card: card) }
                .help("Why this card is here")
        }
        .padding(.top, 2)
    }

    private func secondary(_ title: String, key: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 6) { Text(title); KeyHint(key).opacity(focused ? 1 : 0) }
        }
        .buttonStyle(.glass).controlSize(.small)
    }

    private func quiet(_ title: String, key: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 5) { Text(title); KeyHint(key).opacity(focused ? 1 : 0) }
        }
        .buttonStyle(.plain).font(.system(size: 12)).foregroundStyle(Theme.text3)
    }
}

/// "Why this card?" in plain words.
struct WhyPopover: View {
    let card: FeedCard

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Why this card is here").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.text3)
            if let why = card.why ?? card.texts?.why, !why.isEmpty {
                Label(why, systemImage: "sparkle").foregroundStyle(Theme.accent)
            }
            ForEach(card.reasons, id: \.self) { r in
                HStack(alignment: .firstTextBaseline, spacing: 6) { Text("•"); Text(r) }
            }
        }
        .font(.system(size: 13))
        .foregroundStyle(Theme.text)
        .padding(14)
        .frame(width: 300, alignment: .leading)
    }
}

/// What a handled card leaves for a moment: what happened, and Undo.
struct DoneStripView: View {
    let label: String
    let undo: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "checkmark")
            Text(label)
            Spacer()
            Button("Undo", action: undo).buttonStyle(.plain).foregroundStyle(Theme.accent).fontWeight(.semibold)
        }
        .font(.system(size: 12.5))
        .foregroundStyle(Theme.ok)
        .padding(.horizontal, 12).frame(height: 34)
        .background(Theme.ok.opacity(0.10), in: RoundedRectangle(cornerRadius: 10))
    }
}

/// Feed keys, tried before the app-wide ones while the Feed is on screen (see FeedKeyMap).
enum FeedKeys {
    static let characters = FeedKeyMap.characters

    @MainActor
    static func handle(_ c: String, shift: Bool, model: AppModel) -> Bool {
        guard model.showsFeed, model.later == nil, let action = FeedKeyMap.action(c, shift: shift) else { return false }
        switch action {
        case .next: model.feedMove(1); return true
        case .previous: model.feedMove(-1); return true
        default: break
        }
        guard let card = model.focusedCard, model.feedStrips[card.id] == nil else { return false }
        switch action {
        case .primary: model.runPrimary(card)
        case let .option(n): model.runOption(n, on: card)
        case .later: model.openLater(for: card, typing: false)
        case .laterPickDay: model.openLater(for: card, typing: true)
        case .open: model.openCard(card)
        case .done: model.done(card)
        case .archive: model.archive(card)
        case .delete: model.delete(card)
        case .ask: model.askAbout(card)
        case .reply: model.draftReply(to: card, body: card.texts?.draft ?? "")
        case .accept:
            guard card.thread.suggestion != nil else { return false }
            model.acceptSuggestions([card.thread.id])
        case .next, .previous: return false
        }
        return true
    }
}
