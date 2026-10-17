/// Action cards put the desktop Feed's priorities and prepared replies first on iPhone.
import SwiftUI

struct FeedView: View {
    @Bindable var model: MailboxModel
    let openAISettings: () -> Void
    @State private var reply: CardReply?
    @State private var chatting: MailThread?

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                VStack(alignment: .leading, spacing: 7) {
                    Text(Date().formatted(.dateTime.weekday(.wide).day().month(.wide)))
                        .font(.system(.title2, design: .serif).weight(.semibold))
                    Text(brief).font(.subheadline).foregroundStyle(Theme.text2)
                    if model.ai?.running == true {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text("Preparing your feed · \(model.ai?.completed ?? 0) of \(model.ai?.total ?? 0)")
                                .font(.caption).foregroundStyle(Theme.text3)
                        }
                    }
                }.padding(.vertical, 6)
                if model.ai?.enabled != true && !model.isDemo {
                    Button(action: openAISettings) {
                        HStack(alignment: .top, spacing: 12) {
                            Image(systemName: "sparkles").font(.title2)
                            VStack(alignment: .leading, spacing: 5) {
                                Text("Give your inbox a next step").font(.headline)
                                Text("Connect your AI to prioritize mail, prepare replies and talk through decisions.")
                                    .font(.subheadline).foregroundStyle(Theme.text2)
                            }
                            Image(systemName: "chevron.right").font(.caption)
                        }.padding(18).frame(maxWidth: .infinity, alignment: .leading).paperPanel()
                    }.buttonStyle(.plain).accessibilityIdentifier("connect-ai")
                }
                if let error = model.ai?.error ?? model.aiError {
                    Label(error, systemImage: "exclamationmark.circle").font(.caption).foregroundStyle(Theme.text2)
                }
                if let feed = model.feed {
                    ForEach(feed.sections) { section in
                        HStack(spacing: 8) {
                            Text(section.title.uppercased()).font(.caption.weight(.semibold)).tracking(0.8)
                            Text("\(section.cards.count)").font(.caption).monospacedDigit()
                            Rectangle().fill(Theme.line).frame(height: 1)
                        }.foregroundStyle(Theme.text3).padding(.top, 10)
                        ForEach(section.cards) { card in
                            FeedCardView(card: card, reply: { text in
                                reply = CardReply(thread: card.thread, body: text)
                            }, chat: { chatting = card.thread }, archive: { Task { await archive(card) } })
                                .task { await model.prepareFeed(ids: [card.thread.id] + card.members.map(\.id)) }
                        }
                        if section.more > 0 {
                            Text("\(section.more) more · find all your mail in List")
                                .font(.caption).foregroundStyle(Theme.text3)
                        }
                    }
                    if feed.cards.isEmpty {
                        ContentUnavailableView("All clear", systemImage: "checkmark.circle",
                                               description: Text("New mail lands here as it arrives."))
                    }
                } else { ProgressView("Opening your feed…").frame(maxWidth: .infinity).padding(40) }
                if model.isDemo {
                    Text("Maya’s demo mailbox · Fictional AI cards, stored on this iPhone")
                        .font(.caption).foregroundStyle(Theme.text3).padding(.vertical, 8)
                }
            }.padding(.horizontal, 18).padding(.bottom, 24)
        }
        .background(Theme.ground)
        .refreshable { await model.resume(); await model.prepareFeed() }
        .task { await model.loadFeed(); await model.prepareFeed() }
        .task(id: model.revision) { await model.loadFeed() }
        .sheet(item: $reply) { reply in ReplyView(model: model, thread: reply.thread, initialBody: reply.body) }
        .sheet(item: $chatting) { ChatView(model: model, thread: $0) }
    }

    private var brief: String {
        guard let feed = model.feed else { return "A little clarity for the day ahead." }
        return feed.open == 0 ? "Nothing left to handle." : "\(feed.open) open conversations. One next step at a time."
    }

    private func archive(_ card: MobileFeedCard) async {
        do {
            for thread in [card.thread] + card.members.filter({ $0.id != card.thread.id }) {
                let _: MailThread = try await model.mailbox.call("threads.update", params: ["id": thread.id, "status": "archive"])
            }
            model.revision += 1
            await model.loadFeed()
        } catch { model.aiError = error.localizedDescription }
    }
}

private struct CardReply: Identifiable {
    let id = UUID()
    let thread: MailThread
    let body: String
}

private struct FeedCardView: View {
    let card: MobileFeedCard
    let reply: (String) -> Void
    let chat: () -> Void
    let archive: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            NavigationLink(value: card.thread) {
                VStack(alignment: .leading, spacing: 9) {
                    HStack(spacing: 7) {
                        Label(card.label.uppercased(), systemImage: card.symbol)
                            .font(.system(size: 10, weight: .semibold)).tracking(0.6).foregroundStyle(Color(hex: card.kind.color))
                        Spacer(minLength: 4)
                        if let due = card.due { Text(due).font(.caption2.weight(.medium)).foregroundStyle(Color(hex: "#B8741C")) }
                    }
                    HStack(spacing: 6) {
                        Circle().fill(Theme.accentSoft).frame(width: 22, height: 22)
                            .overlay(Text(String(card.thread.lastFrom.prefix(1))).font(.caption2.weight(.semibold)).foregroundStyle(Theme.accent))
                        Text(card.thread.lastFrom).font(.caption.weight(.medium)).foregroundStyle(Theme.text2).lineLimit(1)
                        Spacer()
                        Text(Date(timeIntervalSince1970: Double(card.thread.lastDate) / 1000).formatted(.dateTime.month(.abbreviated).day()))
                            .font(.caption2).foregroundStyle(Theme.text3).lineLimit(1)
                    }
                    Text(card.title).font(.system(size: 18, weight: .semibold)).foregroundStyle(Theme.text)
                        .multilineTextAlignment(.leading).frame(maxWidth: .infinity, alignment: .leading)
                    if let why = card.why ?? card.texts?.why, !why.isEmpty {
                        Label(why, systemImage: "sparkle").font(.subheadline).foregroundStyle(Theme.text2)
                            .multilineTextAlignment(.leading)
                    }
                    if let amount = card.amount { Text(amount).font(.title.weight(.semibold)).foregroundStyle(Theme.text) }
                    ForEach(card.texts?.points ?? [], id: \.self) { Text("• \($0)").font(.subheadline).foregroundStyle(Theme.text2) }
                    if card.texts?.draft.isEmpty != false {
                        Text(card.texts?.summary.isEmpty == false ? card.texts!.summary : card.thread.snippet)
                            .font(.subheadline).foregroundStyle(Theme.text2).lineLimit(3).multilineTextAlignment(.leading)
                    }
                }
            }.buttonStyle(.plain).accessibilityIdentifier("thread-\(card.thread.id)")
            if let draft = card.texts?.draft, !draft.isEmpty {
                VStack(alignment: .leading, spacing: 7) {
                    Label("Draft reply", systemImage: "sparkle").font(.caption.weight(.medium)).foregroundStyle(Theme.accent)
                    Text(draft).font(.subheadline).foregroundStyle(Theme.text2).lineLimit(4)
                }.padding(12).frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.accentSoft.opacity(0.55), in: RoundedRectangle(cornerRadius: 10))
            }
            ForEach(Array((card.texts?.options ?? []).enumerated()), id: \.offset) { _, option in
                Button { reply(option.reply) } label: {
                    VStack(alignment: .leading, spacing: 3) {
                        HStack {
                            Text(option.label).font(.subheadline.weight(.semibold))
                            if option.recommended { Image(systemName: "sparkle").font(.caption) }
                        }
                        Text(option.detail).font(.caption).foregroundStyle(Theme.text2)
                    }.padding(12).frame(maxWidth: .infinity, alignment: .leading)
                        .background(Theme.accentSoft.opacity(0.4), in: RoundedRectangle(cornerRadius: 10))
                }.buttonStyle(.plain)
            }
            ForEach(card.members.prefix(3)) { member in
                NavigationLink(value: member) {
                    Text(member.subject).font(.subheadline).foregroundStyle(Theme.text2).lineLimit(1)
                }
            }
            Divider().overlay(Theme.line)
            HStack(spacing: 14) {
                if [.reply, .promise, .intro, .waiting].contains(card.kind) {
                    Button { reply(card.texts?.draft ?? "") } label: {
                        Label(card.texts?.draft.isEmpty == false ? "Review reply" : "Reply", systemImage: "arrowshape.turn.up.left")
                    }.font(.subheadline.weight(.medium)).accessibilityIdentifier("card-reply-\(card.thread.id)")
                } else if [.cleanup, .reading].contains(card.kind) {
                    Button(card.members.count > 1 ? "Archive \(card.members.count)" : "Archive", action: archive)
                        .font(.subheadline.weight(.medium))
                    NavigationLink("Read", value: card.thread).font(.subheadline)
                } else {
                    NavigationLink(card.kind == .sign ? "Open to sign" : "Open conversation", value: card.thread)
                        .font(.subheadline.weight(.medium))
                }
                if card.members.isEmpty {
                    Button(action: chat) { Image(systemName: "sparkles") }.accessibilityLabel("Chat about this")
                }
                Spacer(minLength: 0)
                if ![.cleanup, .reading].contains(card.kind) {
                    Button(action: archive) { Image(systemName: "archivebox") }.accessibilityLabel("Archive")
                }
            }.foregroundStyle(Theme.accent)
        }.padding(16).paperPanel()
    }
}
