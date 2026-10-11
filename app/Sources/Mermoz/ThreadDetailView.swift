/// Reading pane: issue chips, summary, messages (older collapsed), attachments, reply bar.
import MermozKit
import SwiftUI

struct ThreadDetailView: View {
    @Environment(AppModel.self) private var model
    let detail: ThreadDetail
    @State private var expanded: Set<String> = []

    var body: some View {
        VStack(spacing: 0) {
            IssueToolbar(thread: detail.thread)
            Divider().overlay(Theme.line)
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
                        let isLast = i == detail.messages.count - 1
                        if isLast || expanded.contains(m.id) {
                            MessageCard(message: m)
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
        .onChange(of: detail.thread.id) { expanded = [] }
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

            Menu {
                ForEach(model.projects) { p in
                    Button(p.name) { model.update([thread.id], project: .set(p.id)) }
                }
                if thread.projectId != nil {
                    Divider()
                    Button("Remove from project") { model.update([thread.id], project: .clear) }
                }
            } label: {
                Chip {
                    let p = model.project(thread.projectId)
                    RoundedRectangle(cornerRadius: 3).fill(Color(hex: p?.color ?? "#C6D7E6")).frame(width: 9, height: 9)
                    Text(p?.name ?? "Project")
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
            Button { model.markDoneSelected() } label: { Image(systemName: "archivebox") }
                .buttonStyle(.plain).foregroundStyle(Theme.text2)
                .help("Done (e)").accessibilityLabel("Mark done")
        }
        .padding(.horizontal, 12)
        .frame(height: 50)
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
            Text(message.bodyText.replacingOccurrences(of: "\n", with: " ")).lineLimit(1).foregroundStyle(Theme.text2)
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
    @State private var formatted = false
    @State private var html: String?
    @State private var height: CGFloat = 200

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Avatar(name: message.from.display, me: message.isFromMe)
                VStack(alignment: .leading, spacing: 1) {
                    Text(message.isFromMe ? "You" : message.from.display).font(.system(size: 13, weight: .semibold))
                    Text(recipientsLine).font(.system(size: 12)).foregroundStyle(Theme.text3).lineLimit(1)
                }
                Spacer()
                if message.hasHtml {
                    Toggle("Show formatted", isOn: $formatted).toggleStyle(.button).controlSize(.small)
                }
                Text(Format.long(message.date)).font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            if formatted, let html {
                HTMLMessageView(html: html, height: $height).frame(height: height)
            } else {
                Text(message.bodyText.isEmpty ? "(empty message)" : message.bodyText)
                    .font(.system(size: 13.5)).lineSpacing(3)
                    .foregroundStyle(Theme.text.opacity(0.9))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
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
        .task(id: formatted) {
            if formatted, html == nil { html = await model.html(for: message.id) }
        }
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
