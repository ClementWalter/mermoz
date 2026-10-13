/// Reading pane: one conversation as a chat. Its Gmail threads read as one stream of
/// messages, newest at the bottom, with tabs for its people, docs, links and photos.
import AppKit
import MermozKit
import SwiftUI

struct ThreadDetailView: View {
    @Environment(AppModel.self) private var model
    let detail: ThreadDetail

    private var conversation: Conversation {
        detail.conversation ?? Conversation(id: detail.thread.id, threads: [detail.thread])
    }

    var body: some View {
        VStack(spacing: 0) {
            IssueToolbar(thread: detail.thread)
            Divider().overlay(Theme.line)
            if let sg = model.appliedSuggestion[detail.thread.id] { AppliedSuggestionBar(threadId: detail.thread.id, suggestion: sg) }
            ConversationHeader(detail: detail, conversation: conversation)
            Divider().overlay(Theme.line)
            switch model.conversationTab {
            case .messages:
                ChatStream(detail: detail, conversation: conversation)
                ReplyBar(target: model.replyTarget(detail).subject, threads: conversation.threads.count)
            case .people:
                ConversationPeopleTab(people: conversation.people)
            case .docs:
                DriveDocsHint(threadId: detail.thread.id, accountId: detail.thread.accountId)
                ConversationDocsTab(entries: DocEntry.merged(attachments: conversation.docs, linked: model.linkedDocs(for: detail)))
            case .links:
                ConversationLinksTab(links: conversation.links)
            case .photos:
                ConversationPhotosTab(photos: conversation.photos)
            }
        }
        .foregroundStyle(Theme.text)
        // Drive previews, the Docs tab's Drive files and saved attachments; again on new mail.
        .task(id: "\(detail.thread.id)-\(detail.messages.count)") { await model.loadDrive(threadId: detail.thread.id) }
    }
}

/// The messages of one conversation as a chat. A long conversation opens on its latest
/// messages; the earlier ones join on request, so opening it lays out a screenful.
struct ChatStream: View {
    @Environment(AppModel.self) private var model
    let detail: ThreadDetail
    let conversation: Conversation
    /// Messages shown when a conversation opens.
    static let opening = 12

    var body: some View {
        let all = Timeline.items(detail.messages, threads: conversation.threads)
        let (items, kept) = model.chatShowsAll ? (all, 0) : Timeline.recent(all, keep: Self.opening)
        let hidden = kept + (model.chatShowsAll ? 0 : detail.earlier ?? 0)
        let runs = ChatRuns.startsRun(items)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 4) {
                    if let s = detail.thread.summary, !s.isEmpty { SummaryBox(summary: s, next: detail.thread.nextAction) }
                    if hidden > 0 {
                        Button("Show \(hidden) earlier message\(hidden == 1 ? "" : "s")") { model.chatShowsAll = true }
                            .buttonStyle(.link).font(.system(size: 12))
                            .frame(maxWidth: .infinity).padding(.vertical, 6)
                    }
                    // Rows are keyed by place, not message: the next conversation reuses
                    // these views and SwiftUI updates them rather than building new ones.
                    ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                        switch item {
                        case let .marker(threadId, subject, date, joined):
                            ThreadMarker(threadId: threadId, subject: subject, date: date, joined: joined)
                        case let .message(m):
                            MessageRow(message: m, firstOfRun: runs.contains(m.id), kind: conversation.kind)
                        }
                    }
                }
                .padding(.horizontal, 22).padding(.vertical, 16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            // ↑/↓ move between messages, Enter opens or folds the one in focus.
            .focusable()
            .focusEffectDisabled()
            .onKeyPress(.upArrow) { moveFocus(-1, proxy); return .handled }
            .onKeyPress(.downArrow) { moveFocus(1, proxy); return .handled }
            .onKeyPress(.return) {
                guard let id = model.focusedMessageId else { return .ignored }
                model.toggleMessage(id)
                return .handled
            }
            // A chat opens on its latest message. The anchor places it there without laying
            // out every bubble above, which scrolling to the last one would.
            .defaultScrollAnchor(.bottom)
            // Another conversation, or new mail in this one, shows its latest message.
            .onChange(of: "\(detail.thread.id)/\(detail.messages.count)") { proxy.scrollTo(items.count - 1, anchor: .bottom) }
        }
    }

    private func moveFocus(_ delta: Int, _ proxy: ScrollViewProxy) {
        let ids = detail.messages.sorted { $0.date < $1.date }.map(\.id)
        guard !ids.isEmpty else { return }
        let at = model.focusedMessageId.flatMap { ids.firstIndex(of: $0) } ?? (delta > 0 ? -1 : ids.count)
        let i = min(max(at + delta, 0), ids.count - 1)
        // Stepping above the latest messages brings the earlier ones in.
        if i < ids.count - Self.opening { model.chatShowsAll = true }
        model.focusedMessageId = ids[i]
        let all = Timeline.items(detail.messages, threads: conversation.threads)
        let shown = model.chatShowsAll ? all : Timeline.recent(all, keep: Self.opening).items
        guard let row = shown.firstIndex(where: { $0.id == ids[i] }) else { return }
        withAnimation(.easeOut(duration: 0.12)) { proxy.scrollTo(row, anchor: .center) }
    }
}

/// Title, who is in it, and the tabs about this conversation.
struct ConversationHeader: View {
    @Environment(AppModel.self) private var model
    let detail: ThreadDetail
    let conversation: Conversation

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 8) {
            Text(detail.thread.subject.isEmpty ? "(no subject)" : detail.thread.subject)
                .font(.system(size: 17, weight: .semibold))
                .textSelection(.enabled)
                .lineLimit(2)
            Text(subtitle).font(.system(size: 12)).foregroundStyle(Theme.text3).lineLimit(1)
            HStack(spacing: 2) {
                ForEach(ConversationTab.allCases, id: \.self) { tab in
                    Button { model.conversationTab = tab } label: {
                        HStack(spacing: 4) {
                            Text(tab.rawValue)
                            if let n = count(tab), n > 0 { Text("\(n)").foregroundStyle(Theme.text3) }
                        }
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(model.conversationTab == tab ? Theme.accent : Theme.text2)
                        .padding(.horizontal, 9).frame(height: 24)
                        .background(model.conversationTab == tab ? Theme.accentSoft : .clear, in: RoundedRectangle(cornerRadius: 7))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .padding(.horizontal, 18).padding(.top, 12).padding(.bottom, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var subtitle: String {
        var parts = [conversation.people.prefix(3).map(\.display).joined(separator: ", ")].filter { !$0.isEmpty }
        let n = conversation.threads.count
        if n > 1 { parts.append("\(n) email threads") }
        parts.append("\(detail.messages.count) message\(detail.messages.count == 1 ? "" : "s")")
        if model.accounts.count > 1, let a = model.accounts.first(where: { $0.id == detail.thread.accountId }) { parts.append(a.email) }
        return parts.joined(separator: " · ")
    }

    private func count(_ tab: ConversationTab) -> Int? {
        switch tab {
        case .messages: nil
        case .people: conversation.people.count
        case .docs: conversation.docs.count
        case .links: conversation.links.count
        case .photos: conversation.photos.count
        }
    }
}

/// Where another Gmail thread starts in the chat. A thread added to the conversation
/// can be taken out with one click.
struct ThreadMarker: View {
    @Environment(AppModel.self) private var model
    let threadId: String
    let subject: String
    let date: Int64
    let joined: Bool

    var body: some View {
        VStack(spacing: 5) {
            if joined {
                HStack(spacing: 8) {
                    Text("Added from another email")
                    Button("Not the same conversation") { model.splitThread(threadId) }
                        .buttonStyle(.link)
                        .help("Show this email on its own again")
                }
                .font(.system(size: 11.5)).foregroundStyle(Theme.text2)
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(Theme.accentSoft.opacity(0.7), in: Capsule())
            }
            Text("\(Format.short(date)) · \(subject.isEmpty ? "(no subject)" : subject)")
                .font(.system(size: 11)).foregroundStyle(Theme.text3).lineLimit(1)
                .padding(.horizontal, 10).padding(.vertical, 3)
                .overlay(Capsule().strokeBorder(Theme.line))
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 6)
    }
}

/// One message as the chat shows it by default: a light bubble with only what the
/// message adds (quoted history and signature folded locally), the user's on the right.
/// The name and time show on the first bubble of a run. A click opens the whole email.
struct CompactBubble: View {
    @Environment(AppModel.self) private var model
    let message: Message
    let firstOfRun: Bool

    private var focused: Bool { model.focusedMessageId == message.id }

    var body: some View {
        let parts = BubbleParts.compact(message)
        let mine = message.isFromMe
        HStack(alignment: .top, spacing: 8) {
            if mine { Spacer(minLength: 72) }
            if !mine {
                if firstOfRun { Avatar(name: message.from.display, size: 26) } else { Color.clear.frame(width: 26, height: 1) }
            }
            VStack(alignment: mine ? .trailing : .leading, spacing: 3) {
                if firstOfRun {
                    HStack(spacing: 6) {
                        Text(mine ? "You" : message.from.display).font(.system(size: 12, weight: .semibold))
                        Text(Format.short(message.date)).font(.system(size: 11)).foregroundStyle(Theme.text3)
                    }
                }
                bubble(parts)
            }
            if !mine { Spacer(minLength: 72) }
        }
        .padding(.top, firstOfRun ? 6 : 0)
    }

    @ViewBuilder private func bubble(_ parts: BubbleParts) -> some View {
        let content = BubbleContent.compact(message, showAll: model.shownInFull.contains(message.id))
        let background = message.isFromMe ? Theme.accentSoft : Color.white
        if !content.clamped, parts.thumbnails.isEmpty, parts.fileCount == 0 {
            // Most bubbles are text alone: one AppKit view draws the text, background and border.
            BubbleTextView(id: message.id, content: content,
                           chrome: BubbleChrome(background: background,
                                                border: focused ? Theme.accent.opacity(0.6) : Color(hex: "#E3EDF6"),
                                                borderWidth: focused ? 1.5 : 1)) { model.toggleMessage(message.id) }
                .help("Open the whole email")
                .accessibilityLabel("Message from \(message.isFromMe ? "you" : message.from.display)")
                .accessibilityValue(content.text)
                .accessibilityHint("Opens the whole email")
                .accessibilityAddTraits(.isButton)
        } else {
            framedBubble(parts, content: content, background: background)
        }
    }

    private func framedBubble(_ parts: BubbleParts, content: BubbleContent, background: Color) -> some View {
        let clamped = content.clamped
        return VStack(alignment: .leading, spacing: 6) {
            // TextKit draws the text from caches keyed by message and width (BubbleTextCache).
            BubbleTextView(id: message.id, content: content) { model.toggleMessage(message.id) }
                .overlay(alignment: .bottom) {
                    // A long message fades out at its eighth line; "Show more" lets it all in.
                    if clamped {
                        LinearGradient(colors: [background.opacity(0), background], startPoint: .top, endPoint: .bottom)
                            .frame(height: 22).allowsHitTesting(false)
                    }
                }
            if clamped {
                Button("Show more") { model.shownInFull.insert(message.id) }.buttonStyle(.link).font(.system(size: 12))
            }
            if !parts.thumbnails.isEmpty {
                HStack(spacing: 6) {
                    ForEach(parts.thumbnails) { a in
                        PhotoThumb(file: SharedFile(id: a.id, filename: a.filename, mime: a.mime, size: a.size, messageId: message.id,
                                                    threadId: message.threadId ?? "", from: "", date: message.date))
                            .frame(width: 96)
                    }
                }
            }
            if parts.fileCount > 0 {
                Label("\(parts.fileCount)", systemImage: "paperclip").font(.system(size: 11)).foregroundStyle(Theme.text3)
                    .help("\(parts.fileCount) file\(parts.fileCount == 1 ? "" : "s") attached: open the email to see them")
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(background, in: RoundedRectangle(cornerRadius: 15))
        .overlay(RoundedRectangle(cornerRadius: 15).strokeBorder(focused ? Theme.accent.opacity(0.6) : Color(hex: "#E3EDF6"),
                                                                    lineWidth: focused ? 1.5 : 1))
        .contentShape(Rectangle())
        .onTapGesture { model.toggleMessage(message.id) }
        .help("Open the whole email")
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Message from \(message.isFromMe ? "you" : message.from.display)")
        .accessibilityHint("Opens the whole email")
        .accessibilityAddTraits(.isButton)
    }
}

/// A message at either level: its compact bubble, or the whole email once opened.
struct MessageRow: View {
    @Environment(AppModel.self) private var model
    let message: Message
    let firstOfRun: Bool
    var kind: String?

    var body: some View {
        if model.expandedMessages.contains(message.id) {
            ExpandedMessage(message: message, kind: kind)
        } else {
            CompactBubble(message: message, firstOfRun: firstOfRun)
        }
    }
}

/// The whole email, opened in place from its bubble: who it went to, the seal, the
/// Original / Optimized choice, View source, the full body and its files. Clicking its
/// header (or Esc) folds it back into the chat.
struct ExpandedMessage: View {
    @Environment(AppModel.self) private var model
    let message: Message
    var kind: String?
    @State private var showQuoted = false
    @State private var html: String?
    @State private var height: CGFloat = 40
    /// Full From / To / Cc / Bcc / date, opened by clicking the recipients line.
    @State private var showDetails = false
    /// Remote images were asked for ("Show images"); verified senders get them anyway.
    @State private var imagesRequested = false
    @State private var hasRemoteImages = false
    /// "Show more" pressed on a very long message.
    @State private var expandedText = false
    @State private var showSource = false

    private var layout: MessageLayout { MessageLayout.of(message, kind: kind) }
    private var compact: Bool { if case let .bubble(_, c) = layout { c } else { false } }
    /// This message's own Original / Optimized choice; other messages keep theirs.
    private var mode: ReadingMode { model.readingModes[message: message.id] }
    /// The Gmail thread the message belongs to, for asking the AI about it.
    private var threadId: String? { message.threadId ?? model.detail?.thread.id }
    /// Images load for senders Gmail verified, or when the reader asks.
    private var loadImages: Bool { imagesRequested || message.auth == "pass" }
    private var shown: BodyShown { MessageBody.shown(message, mode: mode, htmlReady: html != nil, showQuoted: showQuoted) }

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            if message.isFromMe { Spacer(minLength: 56) }
            bubble
            if !message.isFromMe { Spacer(minLength: 56) }
        }
        .sheet(isPresented: $showSource) { MessageSourceView(message: message) }
    }

    private var bubble: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
                .contentShape(Rectangle())
                .onTapGesture { model.toggleMessage(message.id) }
                .help("Fold back into the chat")
            if !compact {
                Button { withAnimation(.easeOut(duration: 0.15)) { showDetails.toggle() } } label: {
                    HStack(spacing: 4) {
                        Text(Recipients.line(to: message.to, cc: message.cc, bcc: message.bcc ?? [])).lineLimit(1)
                        Image(systemName: showDetails ? "chevron.up" : "chevron.down").font(.system(size: 8, weight: .bold))
                    }
                    .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(showDetails ? "Hide details" : "Show everyone on this email")
            }
            if showDetails { MessageDetails(message: message) }
            if mode == .original, message.hasHtml, hasRemoteImages, !loadImages { remoteImagesBar }
            content
            if mode == .optimized, message.bodyClean == nil { optimizedPending }
            if message.hasQuoted == true, mode == .original {
                // Quoted history stays one click away, like Gmail's "…".
                Button { showQuoted.toggle() } label: {
                    Image(systemName: "ellipsis").font(.system(size: 11, weight: .bold))
                        .foregroundStyle(Theme.text3)
                        .frame(width: 30, height: 16)
                        .background(Theme.accentSoft.opacity(0.7), in: Capsule())
                }
                .buttonStyle(.plain)
                .help(showQuoted ? "Hide earlier messages" : "Show earlier messages")
                .accessibilityLabel(showQuoted ? "Hide quoted text" : "Show quoted text")
            }
            DriveLinkCards(messageId: message.id)
            let chips = MessageBody.chips(message)
            if !chips.isEmpty {
                FlowRow(spacing: 8) {
                    ForEach(chips) { a in
                        HStack(spacing: 2) {
                            Button { model.openAttachment(a) } label: { AttachmentChip(attachment: a) }.buttonStyle(.plain)
                                .onDrag { model.dragProvider(for: a) }
                            SaveToDriveButton(attachment: a)
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 14).padding(.vertical, compact ? 9 : 11)
        .background(message.isFromMe ? Theme.accentSoft : Color.white, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Color(hex: "#E3EDF6")))
        .task(id: "\(mode)-\(showQuoted)-\(loadImages)") {
            // The formatted email is local and quick; its text shows meanwhile.
            if mode == .original, message.hasHtml, let body = await model.html(for: message.id, full: showQuoted, images: loadImages) {
                html = body.html
                hasRemoteImages = body.remoteImages ?? false
            }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.text3)
            if !message.isFromMe { Avatar(name: message.from.display) }
            Text(message.isFromMe ? "You" : message.from.display).font(.system(size: 13, weight: .semibold))
            SenderSeal(message: message)
            Text(Format.long(message.date)).font(.system(size: 11.5)).foregroundStyle(Theme.text3)
            Spacer(minLength: 6)
            if mode == .optimized {
                Label("Optimized", systemImage: "sparkle").font(.system(size: 11)).foregroundStyle(Theme.accent)
            }
            Menu {
                Picker("Show", selection: Binding(get: { mode }, set: { choose($0) })) {
                    Text("Original (as sent)").tag(ReadingMode.original)
                    Text("Optimized (clean version by AI)").tag(ReadingMode.optimized)
                }
                .pickerStyle(.inline)
                if mode == .optimized, message.bodyClean != nil, let threadId {
                    Button("Rewrite the optimized version") { model.aiClean(threadId, message: message.id) }
                }
                Divider()
                Button("View source") { showSource = true }
            } label: {
                Image(systemName: "ellipsis.circle").font(.system(size: 13)).foregroundStyle(Theme.text3)
            }
            .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
            .help("Original or optimized, view source")
        }
    }

    /// Picking Optimized is the only thing that asks the AI, and only once per message.
    private func choose(_ m: ReadingMode) {
        model.readingModes[message: message.id] = m
        if m == .optimized, message.bodyClean == nil, let threadId { model.optimize(threadId, message: message.id) }
    }

    @ViewBuilder private var content: some View {
        switch shown {
        case .html:
            if let html { HTMLMessageView(html: html, images: loadImages, height: $height).frame(height: height) }
        case let .clean(c):
            bodyText(Text(c.isEmpty ? AttributedString("(empty message)") : CleanText.attributed(c)))
        case let .text(t):
            let cut = MessageBody.clamp(t.trimmingCharacters(in: .whitespacesAndNewlines), expanded: expandedText)
            bodyText(Text(cut.text.isEmpty ? "(empty message)" : cut.text))
            if cut.clamped {
                Button("Show more") { expandedText = true }.buttonStyle(.link).font(.system(size: 12))
            }
            // The formatted version places pasted images itself; until then they show here.
            let images = MessageBody.inlineImages(message)
            if !images.isEmpty {
                FlowRow(spacing: 8) {
                    ForEach(images) { a in
                        Button { model.openAttachment(a) } label: { PhotoThumb(file: SharedFile(id: a.id, filename: a.filename, mime: a.mime, size: a.size, messageId: message.id, threadId: threadId ?? "", from: "", date: message.date)).frame(width: 160) }
                            .buttonStyle(.plain)
                    }
                }
            }
        }
    }

    /// Optimized chosen but not made yet: the email stays readable above this line.
    private var optimizedPending: some View {
        HStack(spacing: 8) {
            if model.aiBusy.contains("optimize:\(message.id)") {
                ProgressView().controlSize(.mini)
                Text("Writing the optimized version…")
            } else if let threadId {
                Image(systemName: "sparkle")
                Text("No optimized version yet")
                Button("Make it") { model.optimize(threadId, message: message.id) }.buttonStyle(.link)
            }
        }
        .font(.system(size: 12)).foregroundStyle(Theme.text2)
    }

    private var remoteImagesBar: some View {
        HStack(spacing: 8) {
            Image(systemName: "photo").foregroundStyle(Theme.text3)
            Text("Images from the web are hidden so the sender can't see when you read this.").foregroundStyle(Theme.text2)
            Spacer()
            Button("Show images") { imagesRequested = true }.buttonStyle(.link)
        }
        .font(.system(size: 12))
        .padding(.horizontal, 10).frame(minHeight: 28)
        .background(Theme.accentSoft.opacity(0.45), in: RoundedRectangle(cornerRadius: 8))
    }

    private func bodyText(_ text: Text) -> some View {
        text.font(.system(size: 13.5)).lineSpacing(3)
            .foregroundStyle(Theme.text.opacity(0.9))
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The email exactly as received: every header (true From, Reply-To) and its parts.
struct MessageSourceView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let message: Message
    @State private var source: MessageSource?
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Source of this email").font(.system(size: 14, weight: .semibold))
                Spacer()
                if let source {
                    Button("Copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(source.source, forType: .string)
                    }
                }
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            if let source {
                ScrollView {
                    Text(source.source + (source.truncated ? "\n\n… (cut: the rest is attached files)" : ""))
                        .font(.system(size: 11.5, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else if let error {
                Text(error).foregroundStyle(Theme.text2).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .padding(16)
        .frame(minWidth: 640, minHeight: 480)
        .task {
            do { source = try await model.client?.messageSource(message.id) } catch {
                self.error = "The source can't be fetched right now (it comes from Gmail). Try again when online."
            }
        }
    }
}

/// Everyone in the conversation; a click opens them in People.
struct ConversationPeopleTab: View {
    @Environment(AppModel.self) private var model
    let people: [ConversationPerson]

    var body: some View {
        TabList(empty: "No one else in this conversation", isEmpty: people.isEmpty) {
            ForEach(people) { p in
                Button { model.showPerson(p.email) } label: {
                    HStack(spacing: 10) {
                        Avatar(name: p.display)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(p.display).font(.system(size: 13, weight: .semibold))
                            Text(p.role.isEmpty ? p.email : p.role).font(.system(size: 12)).foregroundStyle(Theme.text3).lineLimit(1)
                        }
                        Spacer()
                        if p.messages > 0 {
                            Text("\(p.messages) message\(p.messages == 1 ? "" : "s")").font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                        }
                    }
                    .padding(.horizontal, 16).padding(.vertical, 8)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }
}

/// Attachments and files linked from elsewhere, newest first.
struct ConversationDocsTab: View {
    @Environment(AppModel.self) private var model
    let entries: [DocEntry]

    var body: some View {
        TabList(empty: "No files in this conversation", isEmpty: entries.isEmpty) {
            ForEach(entries) { e in
                switch e {
                case let .attachment(f):
                    Button { model.openAttachment(f.attachment) } label: { FileRow(file: f) }.buttonStyle(.plain)
                case let .linked(_, title, url, from, date):
                    Button { if let u = URL(string: url) { NSWorkspace.shared.open(u) } } label: {
                        HStack(spacing: 10) {
                            Image(systemName: "doc.text").foregroundStyle(Theme.accent).frame(width: 26)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(title).font(.system(size: 13)).lineLimit(1)
                                Text("\(from) · \(Format.short(date))").font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                            }
                            Spacer()
                        }
                        .padding(.horizontal, 14).padding(.vertical, 7)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

struct ConversationLinksTab: View {
    @Environment(AppModel.self) private var model
    let links: [SharedLink]

    var body: some View {
        TabList(empty: "No links in this conversation", isEmpty: links.isEmpty) {
            ForEach(links) { l in
                Button { model.openLink(l) } label: {
                    HStack(spacing: 10) {
                        Image(systemName: "link").foregroundStyle(Theme.accent)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(l.short).font(.system(size: 13)).lineLimit(1)
                            Text("\(l.from) · \(Format.short(l.date))").font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                        }
                        Spacer()
                    }
                    .padding(.horizontal, 16).padding(.vertical, 8)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(l.url)
            }
        }
    }
}

struct ConversationPhotosTab: View {
    @Environment(AppModel.self) private var model
    let photos: [SharedFile]

    var body: some View {
        if photos.isEmpty {
            TabList(empty: "No photos in this conversation", isEmpty: true) { EmptyView() }
        } else {
            ScrollView {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 120), spacing: 8)], spacing: 8) {
                    ForEach(photos) { p in
                        Button { model.openAttachment(p.attachment) } label: { PhotoThumb(file: p) }.buttonStyle(.plain)
                    }
                }
                .padding(16)
            }
        }
    }
}

/// A photo from the mailbox's local copy, loaded when it scrolls into view.
struct PhotoThumb: View {
    @Environment(AppModel.self) private var model
    let file: SharedFile
    @State private var image: NSImage?

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 10).fill(Theme.accentSoft.opacity(0.6))
            if let image {
                Image(nsImage: image).resizable().scaledToFill()
            } else {
                Image(systemName: "photo").foregroundStyle(Theme.text3)
            }
        }
        .frame(height: 120)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .help("\(file.filename) · \(file.from)")
        .task(id: file.id) {
            if let f = try? await model.client?.attachment(file.id) { image = NSImage(contentsOfFile: f.path) }
        }
    }
}

/// A tab's scrolling list, or a quiet line when it has nothing.
struct TabList<Content: View>: View {
    let empty: String
    let isEmpty: Bool
    @ViewBuilder let content: () -> Content

    var body: some View {
        if isEmpty {
            Text(empty).font(.system(size: 12.5)).foregroundStyle(Theme.text3)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView { LazyVStack(alignment: .leading, spacing: 0) { content() }.padding(.vertical, 8) }
        }
    }
}

struct IssueToolbar: View {
    @Environment(AppModel.self) private var model
    let thread: ThreadSummary
    @State private var dueDate = Date()
    @State private var showDue = false

    var body: some View {
        HStack(spacing: 6) {
            Menu {
                ForEach(model.statuses) { s in
                    Button { model.update([thread.id], status: s.key) } label: { Text(s.name) }
                }
            } label: {
                Chip {
                    StatusIcon(key: thread.status, color: Color(hex: model.color(forStatus: thread.status)))
                    Text(model.statuses.first { $0.key == thread.status }?.name ?? thread.status)
                }
            }
            .menuStyle(.button).buttonStyle(.plain).fixedSize()

            Menu {
                ForEach([1, 2, 3, 4, 0], id: \.self) { p in
                    Button(Theme.priorityNames[p] ?? "") { model.update([thread.id], priority: p) }
                }
            } label: {
                Chip { PriorityIcon(priority: thread.priority); Text(Theme.priorityNames[thread.priority] ?? "") }
            }
            .menuStyle(.button).buttonStyle(.plain).fixedSize()

            // A thread can be in several projects: each item toggles one (a Gmail label).
            Menu {
                ForEach(model.projects) { p in
                    Toggle(p.name, isOn: Binding(
                        get: { thread.projects.contains(p.id) },
                        set: { on in
                            if on { model.update([thread.id], addProjects: [p.id]) } else { model.update([thread.id], removeProjects: [p.id]) }
                        }))
                }
            } label: {
                Chip {
                    let first = thread.projects.first.flatMap { model.project($0) }
                    RoundedRectangle(cornerRadius: 3).fill(Color(hex: first?.color ?? "#C6D7E6")).frame(width: 9, height: 9)
                    Text(projectLabel)
                }
            }
            .menuStyle(.button).buttonStyle(.plain).fixedSize()

            Button { showDue = true } label: {
                Chip {
                    Image(systemName: "clock").font(.system(size: 10.5))
                    Text(thread.due.map { "Due \(Format.due($0))" } ?? "Follow up")
                }
            }
            .buttonStyle(.plain)
            .popover(isPresented: $showDue, arrowEdge: .bottom) {
                VStack(alignment: .leading, spacing: 10) {
                    DatePicker("", selection: $dueDate, displayedComponents: .date).datePickerStyle(.graphical).labelsHidden()
                    HStack {
                        if thread.due != nil {
                            Button("Clear") { model.update([thread.id], due: .clear); showDue = false }
                        }
                        Spacer()
                        Button("Set") {
                            let f = DateFormatter()
                            f.dateFormat = "yyyy-MM-dd"
                            model.update([thread.id], due: .set(f.string(from: dueDate)))
                            showDue = false
                        }
                        .keyboardShortcut(.defaultAction)
                    }
                }
                .padding(12).frame(width: 260)
            }

            Spacer()
            Button { model.toggleChat() } label: { Image(systemName: "bubble.left.and.text.bubble.right") }
                .buttonStyle(.plain).foregroundStyle(model.chatOpen ? Theme.accent : Theme.text2)
                .help("Chat about this (t)").accessibilityLabel("Chat about this")
            Button { model.markDoneSelected() } label: { Image(systemName: "checkmark.circle") }
                .buttonStyle(.plain).foregroundStyle(Theme.text2)
                .help("Done (e)").accessibilityLabel("Mark done")
            Button { model.moveSelected(to: "archive") } label: { Image(systemName: "archivebox") }
                .buttonStyle(.plain).foregroundStyle(Theme.text2)
                .help("Archive (a)").accessibilityLabel("Archive")
            Button { model.moveSelected(to: "deleted") } label: { Image(systemName: "trash") }
                .buttonStyle(.plain).foregroundStyle(Theme.text2)
                .help("Delete (⌫)").accessibilityLabel("Delete")
        }
        .padding(.horizontal, 12)
        .frame(height: 50)
    }

    private var projectLabel: String {
        switch thread.projects.count {
        case 0: "Project"
        case 1: thread.projects[0]
        default: "\(thread.projects[0]) +\(thread.projects.count - 1)"
        }
    }
}

/// Shown after opening an Unread thread the AI had sorted: reading applied its
/// suggestion; Undo puts the thread in To do without the added projects.
struct AppliedSuggestionBar: View {
    @Environment(AppModel.self) private var model
    let threadId: String
    let suggestion: Suggestion

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "sparkle").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.accent)
            Text("Sorted by AI:").foregroundStyle(Theme.text2)
            Text(description).fontWeight(.medium)
            Spacer()
            Button("Undo") { model.undoApplied(threadId) }.buttonStyle(.plain).foregroundStyle(Theme.accent)
        }
        .font(.system(size: 12))
        .padding(.horizontal, 14).frame(height: 32)
        .background(Theme.accentSoft.opacity(0.55))
    }

    private var description: String {
        var parts: [String] = []
        if let s = suggestion.status { parts.append(model.statuses.first { $0.key == s }?.name ?? s) }
        parts += suggestion.projects
        return parts.joined(separator: " · ")
    }
}

struct SummaryBox: View {
    let summary: String
    let next: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Summary · next action", systemImage: "sparkle")
                .font(.system(size: 11, weight: .semibold)).kerning(0.5).textCase(.uppercase)
                .foregroundStyle(Theme.accent)
            Text(summary).font(.system(size: 13)).foregroundStyle(Theme.text.opacity(0.9))
            if let next, !next.isEmpty {
                Text("→ \(next)").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Color(hex: "#164F82"))
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.accentSoft.opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(hex: "#D3E5F6")))
    }
}

struct Avatar: View {
    let name: String
    var me = false
    var size: CGFloat = 28

    var body: some View {
        let initials = name.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined().uppercased()
        Circle()
            .fill(me ? Theme.text.opacity(0.8) : Color(hex: Theme.accountPalette[stableHash % Theme.accountPalette.count]))
            .overlay(Text(me ? "You" : (initials.isEmpty ? "?" : initials))
                .font(.system(size: (me ? 9 : 11) * size / 28, weight: .bold)).foregroundStyle(.white))
            .frame(width: size, height: size)
    }

    /// String.hashValue is seeded per launch; colors must stay the same across launches.
    private var stableHash: Int { name.unicodeScalars.reduce(0) { $0 &+ Int($1.value) } }
}

/// File badge, name and size; with `onRemove`, an × to take it out (composer).
struct AttachmentChip: View {
    let filename: String
    let size: Int64
    var isLink = false
    var onRemove: (() -> Void)?

    init(attachment: Attachment) {
        filename = attachment.filename
        size = attachment.size
    }

    init(filename: String, size: Int64, isLink: Bool = false, onRemove: (() -> Void)? = nil) {
        self.filename = filename
        self.size = size
        self.isLink = isLink
        self.onRemove = onRemove
    }

    var body: some View {
        HStack(spacing: 9) {
            Group {
                if isLink {
                    Image(systemName: "link").font(.system(size: 11, weight: .bold))
                } else {
                    Text(ext).font(.system(size: 9, weight: .heavy))
                }
            }
            .foregroundStyle(.white)
            .frame(width: 26, height: 30).background(color, in: RoundedRectangle(cornerRadius: 5))
            VStack(alignment: .leading, spacing: 1) {
                Text(filename).font(.system(size: 12, weight: .semibold)).lineLimit(1).truncationMode(.middle)
                Text(isLink ? "Link" : Format.size(size)).font(.system(size: 11.5)).foregroundStyle(Theme.text3)
            }
            .frame(maxWidth: 200, alignment: .leading)
            if let onRemove {
                Button(action: onRemove) { Image(systemName: "xmark").font(.system(size: 9, weight: .bold)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.text3)
                    .accessibilityLabel("Remove \(filename)")
            }
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 10).padding(.vertical, 7)
        .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.line))
    }

    private var ext: String {
        let e = (filename as NSString).pathExtension.uppercased()
        return e.isEmpty ? "FILE" : String(e.prefix(4))
    }

    private var color: Color {
        switch ext {
        case "PDF": Color(hex: "#D9534F")
        case "XLS", "XLSX", "CSV": Color(hex: "#2E9A6B")
        case "DOC", "DOCX": Color(hex: "#1F6FB5")
        case "PNG", "JPG", "JPEG", "GIF", "HEIC": Color(hex: "#8A6FD1")
        default: Color(hex: "#5A7386")
        }
    }
}

struct ReplyBar: View {
    @Environment(AppModel.self) private var model
    /// Subject of the Gmail thread the reply goes to, named when there are several.
    var target = ""
    var threads = 1

    var body: some View {
        Button { model.startReply() } label: {
            HStack(spacing: 8) {
                Text("Reply…").foregroundStyle(Theme.text3)
                KeyHint("R")
                Spacer()
                if threads > 1, !target.isEmpty {
                    Text("Replies to “\(target)”").font(.system(size: 11.5)).foregroundStyle(Theme.text3).lineLimit(1)
                }
            }
            .padding(.horizontal, 14)
            .frame(height: 42)
            .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.line))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 26).padding(.bottom, 18).padding(.top, 6)
    }
}

/// Wrapping row for chips (attachments, recipients).
struct FlowRow: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxW = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, rowH: CGFloat = 0, widest: CGFloat = 0
        for v in subviews {
            let s = v.sizeThatFits(.unspecified)
            if x > 0 && x + s.width > maxW { y += rowH + spacing; x = 0; rowH = 0 }
            x += s.width + spacing
            rowH = max(rowH, s.height)
            widest = max(widest, x - spacing)
        }
        return CGSize(width: min(widest, maxW), height: y + rowH)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, rowH: CGFloat = 0
        for v in subviews {
            let s = v.sizeThatFits(.unspecified)
            if x > bounds.minX && x + s.width > bounds.maxX { y += rowH + spacing; x = bounds.minX; rowH = 0 }
            v.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(s))
            x += s.width + spacing
            rowH = max(rowH, s.height)
        }
    }
}

private extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}

/// Green seal when Gmail verified the sender's domain sent this email; an orange
/// warning when the domain says its genuine mail always passes and this one did not
/// (a likely impersonation), or when the domain is a platform anyone can send from.
/// Nothing when there is no proof either way, e.g. a copy relayed by a forwarder.
struct SenderSeal: View {
    let message: Message

    var body: some View {
        switch message.auth {
        case "pass":
            Image(systemName: "checkmark.seal.fill").font(.system(size: 13)).foregroundStyle(Theme.ok)
                .help("Verified sender: \(message.from.domain) confirmed this email really comes from them")
                .accessibilityLabel("Verified sender")
        case "shared":
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 12)).foregroundStyle(Color(hex: "#E0782F"))
                .help("Sent through \(message.from.domain), a platform anyone can sign up to. The name “\(message.from.display)” is not verified: be careful with links and attachments.")
                .accessibilityLabel("Sender not verified")
        case "fail":
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 12)).foregroundStyle(Color(hex: "#E0782F"))
                .help("This email may not really be from \(message.from.email). \(message.from.domain) says its genuine emails can always be confirmed, and this one could not. Be careful with links and attachments.")
                .accessibilityLabel("May not really be from this sender")
        default:
            EmptyView()
        }
    }
}

/// Everyone on a message, in full, like the header details of other mail clients.
struct MessageDetails: View {
    let message: Message

    var body: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 14, verticalSpacing: 6) {
            row("From", [message.from])
            row("To", message.to)
            row("Cc", message.cc)
            row("Bcc", message.bcc ?? [])
            GridRow {
                Text("Date").foregroundStyle(Theme.text3).gridColumnAlignment(.trailing)
                Text(Format.long(message.date))
            }
        }
        .font(.system(size: 12.5))
        .foregroundStyle(Theme.text)
        .textSelection(.enabled)
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.accentSoft.opacity(0.35), in: RoundedRectangle(cornerRadius: 10))
    }

    @ViewBuilder private func row(_ label: String, _ people: [Address]) -> some View {
        if !people.isEmpty {
            GridRow {
                Text(label).foregroundStyle(Theme.text3).gridColumnAlignment(.trailing)
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(people, id: \.email) { a in
                        HStack(spacing: 6) {
                            if !a.name.isEmpty { Text(a.name).fontWeight(.medium) }
                            Text(a.name.isEmpty ? a.email : "<\(a.email)>").foregroundStyle(Theme.text2)
                        }
                    }
                }
            }
        }
    }
}
