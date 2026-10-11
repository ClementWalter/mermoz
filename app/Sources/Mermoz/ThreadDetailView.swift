/// Reading pane: issue chips, summary, messages (older collapsed), attachments, reply bar.
import MermozKit
import SwiftUI

struct ThreadDetailView: View {
    @Environment(AppModel.self) private var model
    let detail: ThreadDetail
    /// Messages shown in full; the latest starts open and every one folds on header click.
    @State private var expanded: Set<String> = []

    var body: some View {
        VStack(spacing: 0) {
            IssueToolbar(thread: detail.thread)
            Divider().overlay(Theme.line)
            if let sg = model.appliedSuggestion[detail.thread.id] { AppliedSuggestionBar(threadId: detail.thread.id, suggestion: sg) }
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(spacing: 6) {
                        Text(detail.thread.number)
                        Text("·")
                        Text(model.accounts.first { $0.id == detail.thread.accountId }?.email ?? "")
                        Text("·")
                        Text("\(detail.messages.count) message\(detail.messages.count == 1 ? "" : "s")")
                    }
                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.text3)
                    Text(detail.thread.subject.isEmpty ? "(no subject)" : detail.thread.subject)
                        .font(.system(size: 19, weight: .semibold))
                        .textSelection(.enabled)
                    if let s = detail.thread.summary, !s.isEmpty { SummaryBox(summary: s, next: detail.thread.nextAction) }
                    ForEach(Array(detail.messages.enumerated()), id: \.element.id) { i, m in
                        if expanded.contains(m.id) {
                            MessageCard(message: m) { expanded.remove(m.id) }
                        } else {
                            CollapsedMessage(message: m).onTapGesture { expanded.insert(m.id) }
                        }
                    }
                }
                .padding(.horizontal, 26).padding(.vertical, 18)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            ReplyBar()
        }
        .foregroundStyle(Theme.text)
        .onAppear { expanded = Set(detail.messages.suffix(1).map(\.id)) }
        .onChange(of: detail.thread.id) { expanded = Set(detail.messages.suffix(1).map(\.id)) }
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
            // ✦ re-cleans this thread with AI now (signatures, quotes, clutter; fills People).
            Button { model.aiClean(thread.id) } label: {
                if model.aiBusy.contains("clean:\(thread.id)") { ProgressView().controlSize(.mini) } else { Image(systemName: "sparkles") }
            }
            .buttonStyle(.plain).foregroundStyle(Theme.accent)
            .disabled(model.aiBusy.contains("clean:\(thread.id)"))
            .help("Rewrite the optimized version with AI (asks your AI again)").accessibilityLabel("Rewrite with AI")
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

/// How a message is shown: as sent (rendered HTML), or the AI's clean version.
enum ReadingMode: String {
    case original, optimized
}

struct MessageCard: View {
    @Environment(AppModel.self) private var model
    let message: Message
    var onFold: () -> Void = {}
    /// One choice for every message, remembered across launches.
    @AppStorage("reading.mode") private var mode: ReadingMode = .original
    @State private var showQuoted = false
    @State private var html: String?
    @State private var height: CGFloat = 200

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Avatar(name: message.from.display, me: message.isFromMe)
                VStack(alignment: .leading, spacing: 1) {
                    HStack(spacing: 6) {
                        Text(message.isFromMe ? "You" : message.from.display).font(.system(size: 13, weight: .semibold))
                        if !message.isFromMe, !message.from.name.isEmpty {
                            Text(message.from.email).font(.system(size: 12)).foregroundStyle(Theme.text3).textSelection(.enabled).lineLimit(1)
                        }
                    }
                    Text(recipientsLine).font(.system(size: 12)).foregroundStyle(Theme.text3).lineLimit(1)
                }
                Spacer()
                Picker("", selection: $mode) {
                    Text("Original").tag(ReadingMode.original)
                    Label("Optimized", systemImage: "sparkle").tag(ReadingMode.optimized)
                }
                .pickerStyle(.segmented).labelsHidden().controlSize(.small).fixedSize()
                .help("Original: the email as sent · Optimized: a clean version written by AI, kept once made")
                Text(Format.long(message.date)).font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            .contentShape(Rectangle())
            .onTapGesture { onFold() }
            if mode == .optimized {
                optimized
            } else {
                original
            }
            if !message.attachments.isEmpty {
                FlowRow(spacing: 8) {
                    ForEach(message.attachments) { a in
                        Button { model.openAttachment(a) } label: { AttachmentChip(attachment: a) }.buttonStyle(.plain)
                    }
                }
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 14)
        .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(hex: "#E3EDF6")))
        .task(id: "\(mode)-\(showQuoted)") {
            if mode == .original, message.hasHtml { html = await model.html(for: message.id, full: showQuoted) }
        }
    }

    /// The email as people expect it: rendered HTML, or its text for plain-text mail.
    @ViewBuilder private var original: some View {
        if message.hasHtml, let html {
            HTMLMessageView(html: html, height: $height).frame(height: height)
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
        } else if let threadId = model.detail?.thread.id {
            HStack(spacing: 8) {
                if model.aiBusy.contains("optimize:\(threadId)") {
                    ProgressView().controlSize(.small)
                    Text("Writing the optimized version…")
                } else {
                    Image(systemName: "sparkle")
                    Text("No optimized version yet")
                    Button("Make it") { model.optimize(threadId) }.buttonStyle(.link)
                }
            }
            .font(.system(size: 12.5)).foregroundStyle(Theme.text2)
            .frame(maxWidth: .infinity, alignment: .leading)
            .task(id: threadId) { model.optimize(threadId) }
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
        return s
    }
}

struct AttachmentChip: View {
    let attachment: Attachment

    var body: some View {
        HStack(spacing: 9) {
            Text(ext).font(.system(size: 9, weight: .heavy)).foregroundStyle(.white)
                .frame(width: 26, height: 30).background(color, in: RoundedRectangle(cornerRadius: 5))
            VStack(alignment: .leading, spacing: 1) {
                Text(attachment.filename).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                Text(Format.size(attachment.size)).font(.system(size: 11.5)).foregroundStyle(Theme.text3)
            }
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 10).padding(.vertical, 7)
        .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.line))
    }

    private var ext: String {
        let e = (attachment.filename as NSString).pathExtension.uppercased()
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

    var body: some View {
        Button { model.startReply() } label: {
            HStack(spacing: 8) {
                Text("Reply…").foregroundStyle(Theme.text3)
                KeyHint("R")
                Spacer()
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
