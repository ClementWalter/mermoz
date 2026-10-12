/// Reading pane: one conversation as a chat. Its Gmail threads read as one stream of
/// messages, newest at the bottom, with tabs for its people, docs, links and photos.
import AppKit
import MermozKit
import SwiftUI

struct ThreadDetailView: View {
    @Environment(AppModel.self) private var model
    let detail: ThreadDetail
    /// Messages shown in full; the latest starts open and every one folds on header click.
    @State private var expanded: Set<String> = []

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
                chat
                ReplyBar(target: model.replyTarget(detail).subject, threads: conversation.threads.count)
            case .people:
                ConversationPeopleTab(people: conversation.people)
            case .docs:
                ConversationDocsTab(entries: DocEntry.merged(attachments: conversation.docs, linked: model.linkedDocs(for: detail)))
            case .links:
                ConversationLinksTab(links: conversation.links)
            case .photos:
                ConversationPhotosTab(photos: conversation.photos)
            }
        }
        .foregroundStyle(Theme.text)
        .onAppear { expanded = Set(detail.messages.suffix(1).map(\.id)) }
        .onChange(of: detail.thread.id) { expanded = Set(detail.messages.suffix(1).map(\.id)) }
    }

    private var chat: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    if let s = detail.thread.summary, !s.isEmpty { SummaryBox(summary: s, next: detail.thread.nextAction) }
                    ForEach(Timeline.items(detail.messages, threads: conversation.threads)) { item in
                        switch item {
                        case let .marker(threadId, subject, date, joined):
                            ThreadMarker(threadId: threadId, subject: subject, date: date, joined: joined)
                        case let .message(m):
                            MessageBubble(message: m, expanded: expanded.contains(m.id)) {
                                if expanded.contains(m.id) { expanded.remove(m.id) } else { expanded.insert(m.id) }
                            }
                            .id(m.id)
                        }
                    }
                }
                .padding(.horizontal, 22).padding(.vertical, 16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            // A chat opens on its latest message.
            .onAppear { if let last = detail.messages.last { proxy.scrollTo(last.id, anchor: .bottom) } }
            .onChange(of: detail.messages.count) { if let last = detail.messages.last { proxy.scrollTo(last.id, anchor: .bottom) } }
        }
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

/// One message as a chat bubble: the user's on the right. Folded bubbles show a few
/// lines; an open one is the full message with its Original / Optimized choice.
struct MessageBubble: View {
    let message: Message
    let expanded: Bool
    let toggle: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            if message.isFromMe { Spacer(minLength: 56) }
            Group {
                if expanded {
                    MessageCard(message: message, tint: message.isFromMe ? Theme.accentSoft : .white, onFold: toggle)
                } else {
                    FoldedBubble(message: message).onTapGesture(perform: toggle)
                }
            }
            if !message.isFromMe { Spacer(minLength: 56) }
        }
    }
}

struct FoldedBubble: View {
    let message: Message

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            if !message.isFromMe { Avatar(name: message.from.display) }
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(message.isFromMe ? "You" : message.from.display).font(.system(size: 12, weight: .semibold))
                    Text(Format.short(message.date)).font(.system(size: 11)).foregroundStyle(Theme.text3)
                    if !message.attachments.isEmpty {
                        Image(systemName: "paperclip").font(.system(size: 10)).foregroundStyle(Theme.text3)
                    }
                }
                Text((message.bodyClean ?? message.displayBody).trimmingCharacters(in: .whitespacesAndNewlines))
                    .font(.system(size: 13)).foregroundStyle(Theme.text.opacity(0.88))
                    .lineLimit(4)
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .background(message.isFromMe ? Theme.accentSoft : Color.white, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Color(hex: "#E3EDF6")))
        .contentShape(Rectangle())
        .help("Show the whole message")
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

    var body: some View {
        let initials = name.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined().uppercased()
        Circle()
            .fill(me ? Theme.text.opacity(0.8) : Color(hex: Theme.accountPalette[stableHash % Theme.accountPalette.count]))
            .overlay(Text(me ? "You" : (initials.isEmpty ? "?" : initials)).font(.system(size: me ? 9 : 11, weight: .bold)).foregroundStyle(.white))
            .frame(width: 28, height: 28)
    }

    /// String.hashValue is seeded per launch; colors must stay the same across launches.
    private var stableHash: Int { name.unicodeScalars.reduce(0) { $0 &+ Int($1.value) } }
}

struct CollapsedMessage: View {
    let message: Message

    var body: some View {
        HStack(spacing: 10) {
            Avatar(name: message.from.display, me: message.isFromMe)
            Text(message.isFromMe ? "You" : message.from.display).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.text)
            if !message.isFromMe, !message.from.name.isEmpty {
                // Who it is when the display name is generic ("complaints", "support").
                Text(message.from.domain).font(.system(size: 11.5)).foregroundStyle(Theme.text3).lineLimit(1)
            }
            Text(message.displayBody.replacingOccurrences(of: "\n", with: " ")).lineLimit(1).foregroundStyle(Theme.text2)
            Spacer(minLength: 6)
            Text(Format.short(message.date)).foregroundStyle(Theme.text3)
        }
        .font(.system(size: 12.5))
        .padding(.horizontal, 12).padding(.vertical, 9)
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color(hex: "#E3EDF6")))
        .contentShape(Rectangle())
    }
}

struct MessageCard: View {
    @Environment(AppModel.self) private var model
    let message: Message
    /// The bubble's color: the user's own messages are tinted.
    var tint: Color = .white
    var onFold: () -> Void = {}
    /// The Gmail thread the message belongs to, for asking the AI about it.
    private var threadId: String? { message.threadId ?? model.detail?.thread.id }
    /// This message's own Original / Optimized choice; other messages keep theirs.
    private var mode: ReadingMode { model.readingModes[message: message.id] }
    @State private var showQuoted = false
    @State private var html: String?
    @State private var height: CGFloat = 200
    /// Full From / To / Cc / Bcc / date, opened by clicking the recipients line.
    @State private var showDetails = false
    /// Remote images were asked for ("Show images"); verified senders get them anyway.
    @State private var imagesRequested = false
    @State private var hasRemoteImages = false

    /// Images load for senders Gmail verified, or when the reader asks.
    private var loadImages: Bool { imagesRequested || message.auth == "pass" }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Avatar(name: message.from.display, me: message.isFromMe)
                VStack(alignment: .leading, spacing: 1) {
                    HStack(spacing: 6) {
                        Text(message.isFromMe ? "You" : message.from.display).font(.system(size: 13, weight: .semibold))
                        SenderSeal(message: message)
                        if !message.isFromMe, !message.from.name.isEmpty {
                            Text(message.from.email).font(.system(size: 12)).foregroundStyle(Theme.text3).textSelection(.enabled).lineLimit(1)
                        }
                    }
                    Button { withAnimation(.easeOut(duration: 0.15)) { showDetails.toggle() } } label: {
                        HStack(spacing: 4) {
                            Text(recipientsLine).lineLimit(1)
                            Image(systemName: showDetails ? "chevron.up" : "chevron.down").font(.system(size: 8, weight: .bold))
                        }
                        .font(.system(size: 12)).foregroundStyle(Theme.text3)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help(showDetails ? "Hide details" : "Show everyone on this email")
                }
                Spacer()
                if mode == .optimized, message.bodyClean != nil, let threadId {
                    // ✦ asks the AI again for this message only.
                    Button { model.aiClean(threadId, message: message.id) } label: {
                        if model.aiBusy.contains("clean:\(message.id)") { ProgressView().controlSize(.mini) } else { Image(systemName: "sparkles") }
                    }
                    .buttonStyle(.plain).foregroundStyle(Theme.accent)
                    .disabled(model.aiBusy.contains("clean:\(message.id)"))
                    .help("Rewrite this optimized version with AI (asks your AI again)").accessibilityLabel("Rewrite with AI")
                }
                Picker("", selection: Binding(get: { mode }, set: { model.readingModes[message: message.id] = $0 })) {
                    Text("Original").tag(ReadingMode.original)
                    Label("Optimized", systemImage: "sparkle").tag(ReadingMode.optimized)
                }
                .pickerStyle(.segmented).labelsHidden().controlSize(.small).fixedSize()
                .help("Original: the email as sent · Optimized: a clean version written by AI, kept once made")
                Text(Format.long(message.date)).font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            .contentShape(Rectangle())
            .onTapGesture { onFold() }
            if showDetails { MessageDetails(message: message) }
            if mode == .optimized {
                optimized
            } else {
                original
            }
            if !message.attachments.isEmpty {
                FlowRow(spacing: 8) {
                    ForEach(message.attachments) { a in
                        Button { model.openAttachment(a) } label: { AttachmentChip(attachment: a) }.buttonStyle(.plain)
                            .onDrag { model.dragProvider(for: a) }
                    }
                }
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 14)
        .background(tint, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(hex: "#E3EDF6")))
        .task(id: "\(mode)-\(showQuoted)-\(loadImages)") {
            if mode == .original, message.hasHtml, let body = await model.html(for: message.id, full: showQuoted, images: loadImages) {
                html = body.html
                hasRemoteImages = body.remoteImages ?? false
            }
        }
    }

    /// The email as people expect it: rendered HTML, or its text for plain-text mail.
    @ViewBuilder private var original: some View {
        if message.hasHtml, hasRemoteImages, !loadImages {
            HStack(spacing: 8) {
                Image(systemName: "photo").foregroundStyle(Theme.text3)
                Text("Images are hidden so the sender can't see when you read this.").foregroundStyle(Theme.text2)
                Spacer()
                Button("Show images") { imagesRequested = true }.buttonStyle(.link)
            }
            .font(.system(size: 12))
            .padding(.horizontal, 12).frame(height: 32)
            .background(Theme.accentSoft.opacity(0.45), in: RoundedRectangle(cornerRadius: 8))
        }
        if message.hasHtml, let html {
            HTMLMessageView(html: html, images: loadImages, height: $height).frame(height: height)
        } else if message.hasHtml {
            ProgressView().controlSize(.small).frame(maxWidth: .infinity, minHeight: 40)
        } else {
            bodyText(Text((showQuoted ? message.bodyText : message.displayBody).nonEmpty ?? "(empty message)"))
        }
        if message.hasQuoted == true {
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
    }

    /// The AI's clean version, made once per message and stored.
    @ViewBuilder private var optimized: some View {
        if let clean = message.bodyClean {
            bodyText(Text(clean.isEmpty ? AttributedString("(empty message)") : CleanText.attributed(clean)))
        } else if let threadId {
            HStack(spacing: 8) {
                if model.aiBusy.contains("optimize:\(message.id)") {
                    ProgressView().controlSize(.small)
                    Text("Writing the optimized version…")
                } else {
                    Image(systemName: "sparkle")
                    Text("No optimized version yet")
                    Button("Make it") { model.optimize(threadId, message: message.id) }.buttonStyle(.link)
                }
            }
            .font(.system(size: 12.5)).foregroundStyle(Theme.text2)
            .frame(maxWidth: .infinity, alignment: .leading)
            .task(id: message.id) { model.optimize(threadId, message: message.id) }
        }
    }

    private func bodyText(_ text: Text) -> some View {
        text.font(.system(size: 13.5)).lineSpacing(3)
            .foregroundStyle(Theme.text.opacity(0.9))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var recipientsLine: String {
        var s = "to " + message.to.map(\.display).joined(separator: ", ")
        if !message.cc.isEmpty { s += ", cc " + message.cc.map(\.display).joined(separator: ", ") }
        if let bcc = message.bcc, !bcc.isEmpty { s += ", bcc " + bcc.map(\.display).joined(separator: ", ") }
        return s
    }
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
