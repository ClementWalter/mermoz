/// Sort: the email on the left, the AI's suggestion on the right, answered with
/// Yes (y), No (n), Change… (c) or Skip (s); ⌘Z takes an answer back.
import MermozKit
import SwiftUI

struct SortView: View {
    var body: some View {
        HStack(spacing: 0) {
            DetailPane().frame(maxWidth: .infinity, maxHeight: .infinity)
            SortPanel().frame(width: 340)
        }
    }
}

struct SortPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if let offer = model.sortInfo.offer {
                ProviderOfferBanner(offer: offer)
            } else if let paused = SortText.paused(model.sortInfo.paused, provider: model.sortInfo.providerName) {
                Label(paused, systemImage: "pause.circle")
                    .font(.system(size: 12.5)).foregroundStyle(Theme.text2)
                    .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.accentSoft.opacity(0.6), in: RoundedRectangle(cornerRadius: 8))
            }
            if let card = model.sortQueue.current {
                SortCardView(card: card)
            } else if model.sortLoading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Looking at the next email…").font(.system(size: 13)).foregroundStyle(Theme.text2)
                }
            } else {
                empty
            }
            Spacer(minLength: 0)
            learned
        }
        .padding(16)
        .frame(maxHeight: .infinity, alignment: .top)
        .glassPanel()
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "sparkles").foregroundStyle(Theme.accent)
            Text("Sort").font(.system(size: 15, weight: .semibold))
            Text(SortText.toSort(model.sortColumns.isEmpty ? model.sortInfo.toSort
                                 : model.sortColumns.map(model.sortInfo.count).reduce(0, +)))
                .font(.system(size: 12)).foregroundStyle(Theme.text3)
            Spacer()
            if let col = model.sortColumns.first {
                Button { model.openSort() } label: {
                    HStack(spacing: 4) {
                        Text(model.statuses.first { $0.key == col }?.name ?? col)
                        Image(systemName: "xmark").font(.system(size: 8, weight: .bold))
                    }
                    .font(.system(size: 11.5))
                }
                .buttonStyle(.plain).foregroundStyle(Theme.text2)
                .help("Sort every open column")
            }
        }
    }

    @ViewBuilder private var empty: some View {
        if !model.sortInfo.configured && !model.router.configured {
            Text("Add a key for instant sorting in Settings › Developer to get suggestions.")
                .font(.system(size: 13)).foregroundStyle(Theme.text2)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                Image(systemName: "checkmark.circle").font(.system(size: 22, weight: .light)).foregroundStyle(Theme.ok)
                Text("Nothing to sort right now").font(.system(size: 13.5, weight: .medium))
            }
        }
    }

    /// What the answers taught, in plain words.
    @ViewBuilder private var learned: some View {
        let items = Array(model.sortInfo.learned.prefix(5))
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 5) {
                Text("LEARNED").font(.system(size: 10.5, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.text3)
                ForEach(items) { item in
                    Label(item.sentence(statusName: statusName), systemImage: item.kind == "rule" ? "checkmark" : "nosign")
                        .font(.system(size: 11.5)).foregroundStyle(Theme.text2).lineLimit(2)
                }
            }
        }
    }

    private func statusName(_ key: String) -> String { model.statuses.first { $0.key == key }?.name ?? key }
}

struct SortCardView: View {
    @Environment(AppModel.self) private var model
    let card: SortCard

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(card.thread.lastFrom).font(.system(size: 12.5, weight: .medium)).lineLimit(1)
                Text(card.thread.subject.isEmpty ? "(no subject)" : card.thread.subject)
                    .font(.system(size: 12.5)).foregroundStyle(Theme.text2).lineLimit(2)
                Text("Now in \(statusName(card.thread.status))")
                    .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
            }
            if let sg = card.thread.suggestion {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Suggested").font(.system(size: 10.5, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.text3)
                    Text("→ " + SortText.target(status: sg.status, projects: sg.projects, statusName: statusName))
                        .font(.system(size: 17, weight: .semibold)).foregroundStyle(Theme.accent)
                    if let why = card.why {
                        Text(why).font(.system(size: 11.5)).foregroundStyle(Theme.text2)
                    }
                }
                .padding(12).frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.accentSoft.opacity(0.55), in: RoundedRectangle(cornerRadius: 10))
            }
            HStack(spacing: 8) {
                Button { model.sortYes() } label: { Text("Yes").frame(maxWidth: .infinity) }
                    .buttonStyle(.glassProminent).tint(Theme.accent).controlSize(.large)
                    .help("Move it there (y)")
                Button { model.sortNo() } label: { Text("No").frame(maxWidth: .infinity) }
                    .buttonStyle(.glass).controlSize(.large)
                    .help("Leave it where it is; never suggested again for this sender (n)")
            }
            HStack(spacing: 8) {
                changeMenu
                Button { model.sortSkip() } label: { Text("Skip").frame(maxWidth: .infinity) }
                    .buttonStyle(.glass)
                    .help("Not now (s)")
            }
            Text("y yes · n no · c change · s skip · ⌘Z undo")
                .font(.system(size: 11)).foregroundStyle(Theme.text3)
        }
    }

    /// Pick where it really goes: a status or a project.
    private var changeMenu: some View {
        Menu {
            Section("Status") {
                ForEach(model.statuses.filter { routable.contains($0.key) && $0.key != card.thread.status }) { s in
                    Button(s.name) { model.sortChange(status: s.key) }
                }
            }
            Section("Project") {
                ForEach(model.projects.filter { !card.thread.projects.contains($0.name) }) { p in
                    Button(p.name) { model.sortChange(project: p.name) }
                }
            }
        } label: {
            Text("Change…").frame(maxWidth: .infinity)
        }
        .menuStyle(.button).buttonStyle(.glass)
        .help("Send it somewhere else (c)")
    }

    private var routable: Set<String> { ["todo", "waiting", "in_progress", "backlog", "done", "archive"] }

    private func statusName(_ key: String) -> String { model.statuses.first { $0.key == key }?.name ?? key }
}

/// "✦ 312 to sort · Start" on an open column: opens the Sort queue on that column.
struct SortStartButton: View {
    @Environment(AppModel.self) private var model
    let status: String

    var body: some View {
        let n = model.sortInfo.count(status)
        if n > 0, model.router.configured || model.sortInfo.configured {
            Button { model.openSort([status]) } label: {
                HStack(spacing: 5) {
                    Image(systemName: "sparkle").font(.system(size: 9, weight: .semibold))
                    Text(SortText.toSort(n))
                    Text("· Start").fontWeight(.semibold)
                }
                .font(.system(size: 11.5))
                .foregroundStyle(Theme.accent)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Go through this column one email at a time")
        }
    }
}

/// "TypeSafe credits ran out. Use SubQ instead?" with a button: the switch happens only
/// when the user presses it.
struct ProviderOfferBanner: View {
    @Environment(AppModel.self) private var model
    let offer: ProviderOffer

    var body: some View {
        HStack(spacing: 8) {
            Label(offer.message, systemImage: "pause.circle")
                .font(.system(size: 12.5)).foregroundStyle(Theme.text2)
            Spacer(minLength: 4)
            Button("Use \(offer.name)") { Task { await model.setRouterProvider(offer.provider) } }
                .buttonStyle(.glassProminent).controlSize(.small)
        }
        .padding(10).frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.accentSoft.opacity(0.6), in: RoundedRectangle(cornerRadius: 8))
    }
}
