/// Plain-text composer with recipient pills that carry a trust mark, and
/// attachments added by drop, file picker, ⇧⌘A or paste.
import AppKit
import MermozKit
import SwiftUI
import UniformTypeIdentifiers

struct ComposeView: View {
    @Environment(AppModel.self) private var model
    @State var context: ComposeContext
    @State private var thenWaiting = false
    @State private var sending = false
    @State private var confirmSuspicious = false
    @State private var confirmNoReply = false
    @State private var dropTargeted = false
    @State private var picking = false
    @State private var pasteMonitor: Any?
    /// Drive files to share with the recipients, while the user is asked.
    @State private var sharePlan: SharePlan?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                if let tid = context.threadId, let t = model.detail?.thread, t.id == tid {
                    Text(t.number).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.text3)
                    Text("Replying to \(t.lastFrom)").font(.system(size: 12.5)).foregroundStyle(Theme.text2).lineLimit(1)
                } else {
                    Text(context.draftId != nil ? "Edit draft" : "New message").font(.system(size: 13, weight: .semibold))
                }
                Spacer()
                Button { model.compose = nil } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain).foregroundStyle(Theme.text2)
                    .keyboardShortcut(.cancelAction).accessibilityLabel("Discard")
            }
            .padding(.horizontal, 20).frame(height: 50)
            Divider().overlay(Theme.line)

            VStack(spacing: 0) {
                if context.threadId == nil && context.draftId == nil && model.visibleAccounts.count > 1 {
                    field("From") {
                        Picker("", selection: Binding(get: { context.accountId ?? "" }, set: { context.accountId = $0 })) {
                            ForEach(model.visibleAccounts) { Text($0.email).tag($0.id) }
                        }
                        .labelsHidden().fixedSize()
                    }
                }
                field("To") { RecipientField(addresses: $context.to) }
                if let w = context.noReplyWarning { NoReplyBanner(warning: w) { context.to = w.applied(to: context.to) } }
                field("Cc") { RecipientField(addresses: $context.cc) }
                field("Subject") {
                    TextField("", text: $context.subject).textFieldStyle(.plain).font(.system(size: 13))
                }
            }
            .padding(.horizontal, 22)

            ComposeBodyEditor(text: $context.body, focusOnAppear: !context.to.isEmpty,
                              onFiles: { urls in Task { await attach(urls) } },
                              onImage: { data in Task { await attachPastedImage(data) } })
                .padding(12)
                .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.line))
                .padding(.horizontal, 22).padding(.top, 14)

            if !context.attachments.isEmpty { attachmentTray }

            if let why = context.sendError {
                Label(why, systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 12.5)).foregroundStyle(Theme.danger)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 22).padding(.top, 12)
                    .accessibilityLabel("Not sent. \(why)")
            }

            HStack(spacing: 10) {
                Button {
                    trySend()
                } label: {
                    HStack(spacing: 6) {
                        if sending { ProgressView().controlSize(.mini) }
                        Text(context.sendError != nil ? "Retry" : context.draftId != nil && model.isReview ? "Approve & send" : "Send")
                    }
                    .padding(.horizontal, 6)
                }
                .buttonStyle(.glassProminent)
                .tint(Theme.accent)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(sending || context.to.isEmpty || context.attachments.blocksSend)
                Button { picking = true } label: { Image(systemName: "paperclip").font(.system(size: 14, weight: .medium)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.text2)
                    .keyboardShortcut("a", modifiers: [.command, .shift])
                    .help("Attach files (⇧⌘A). You can also drop or paste them here.")
                    .accessibilityLabel("Attach files")
                DriveAttachButton(attachments: $context.attachments, accountId: accountId, recipients: context.to + context.cc)
                if context.threadId != nil {
                    Toggle("Then set Waiting", isOn: $thenWaiting).toggleStyle(.checkbox).font(.system(size: 12.5))
                }
                Spacer()
                Text("Plain text").font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            .padding(.horizontal, 22).padding(.vertical, 16)
        }
        .foregroundStyle(Theme.text)
        .overlay { if dropTargeted { dropHighlight } }
        .onDrop(of: [.fileURL], isTargeted: $dropTargeted) { providers in
            for p in providers {
                _ = p.loadDataRepresentation(for: .fileURL) { data, _ in
                    guard let data, let url = URL(dataRepresentation: data, relativeTo: nil) else { return }
                    Task { @MainActor in await attach([url]) }
                }
            }
            return !providers.isEmpty
        }
        .fileImporter(isPresented: $picking, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            if case let .success(urls) = result { Task { await attach(urls) } }
        }
        .onAppear { installPasteMonitor() }
        .onDisappear {
            if let m = pasteMonitor { NSEvent.removeMonitor(m) }
            pasteMonitor = nil
        }
        .confirmationDialog(NoReplyWarning.title, isPresented: $confirmNoReply) {
            if let w = context.noReplyWarning, let s = w.suggestion {
                Button("Send to \(s.email)") { context.to = w.applied(to: context.to); trySend() }
            }
            Button("Send anyway", role: .destructive) { checkSuspiciousThenSend() }
            Button("Review recipients", role: .cancel) {}
        } message: {
            Text(context.noReplyWarning.map { [$0.detail, $0.question].compactMap { $0 }.joined(separator: " ") } ?? "")
        }
        .confirmationDialog("Some recipients look suspicious", isPresented: $confirmSuspicious) {
            Button("Send anyway", role: .destructive) { askShareThenSend() }
            Button("Review recipients", role: .cancel) {}
        } message: {
            Text(suspicious.joined(separator: ", "))
        }
        // Drive files in the reply: sharing is asked, never done silently.
        .confirmationDialog(sharePlan?.question ?? "", isPresented: Binding(get: { sharePlan != nil }, set: { if !$0 { sharePlan = nil } })) {
            if let plan = sharePlan, let accountId {
                Button("Let them view") { shareThenSend(plan, accountId: accountId, role: "reader") }
                Button("Let them comment") { shareThenSend(plan, accountId: accountId, role: "commenter") }
                Button("Send without sharing") { doSend() }
                Button("Cancel", role: .cancel) {}
            }
        } message: {
            Text(sharePlan?.detail ?? "")
        }
    }

    /// The account the message goes out from.
    private var accountId: String? {
        context.accountId ?? model.detail.flatMap { d in context.threadId == d.thread.id ? d.thread.accountId : nil }
            ?? model.visibleAccounts.first?.id
    }

    private func askShareThenSend() {
        let me = model.accounts.first { $0.id == accountId }?.email
        let plan = SharePlan(attachments: context.attachments.items, recipients: context.to + context.cc, me: me)
        if plan.isNeeded { sharePlan = plan } else { doSend() }
    }

    private func shareThenSend(_ plan: SharePlan, accountId: String, role: String) {
        sending = true
        Task {
            if await model.shareDriveFiles(plan, accountId: accountId, role: role) {
                doSend()
            } else {
                sending = false
            }
        }
    }

    private var suspicious: [String] {
        (context.to + context.cc).filter {
            if case .suspicious = model.checker.check($0) { return true }
            return false
        }
    }

    /// A no-reply recipient is never sent to without the user saying so.
    private func trySend() {
        if context.noReplyWarning != nil { confirmNoReply = true } else { checkSuspiciousThenSend() }
    }

    private func checkSuspiciousThenSend() {
        if suspicious.isEmpty { askShareThenSend() } else { confirmSuspicious = true }
    }

    private func doSend() {
        sending = true
        context.sendError = nil
        Task {
            if case let .failed(again) = await model.send(context, thenWaiting: thenWaiting) { context = again }
            sending = false
        }
    }

    // MARK: attachments

    private var attachmentTray: some View {
        VStack(alignment: .leading, spacing: 8) {
            FlowRow(spacing: 8) {
                ForEach(context.attachments.items) { a in
                    if a.drive != nil {
                        DriveAttachmentChip(item: a, attachments: $context.attachments, accountId: accountId,
                                            recipients: context.to + context.cc)
                    } else {
                        AttachmentChip(filename: a.name, size: a.size, isLink: a.isLink) {
                            context.attachments.remove(id: a.id)
                        }
                    }
                }
            }
            if let tooBig = context.attachments.limitMessage {
                Text(tooBig).font(.system(size: 12)).foregroundStyle(Theme.danger)
                    .fixedSize(horizontal: false, vertical: true)
            } else if context.attachments.isDownloading {
                Text("Downloading from Google Drive…").font(.system(size: 12)).foregroundStyle(Theme.text3)
            } else {
                let n = context.attachments.items.count
                Text("\(n) \(n == 1 ? "attachment" : "attachments") · \(Format.size(context.attachments.totalSize))")
                    .font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
        }
        .padding(.horizontal, 22).padding(.top, 12)
    }

    private var dropHighlight: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 14).fill(Theme.accent.opacity(0.08))
            RoundedRectangle(cornerRadius: 14).strokeBorder(Theme.accent, style: StrokeStyle(lineWidth: 2, dash: [7, 5]))
            Label("Drop to attach", systemImage: "paperclip")
                .font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.accent)
                .padding(.horizontal, 16).padding(.vertical, 10)
                .background(.regularMaterial, in: Capsule())
        }
        .padding(8)
        .allowsHitTesting(false)
    }

    /// Reads sizes off the main thread so typing never waits on the disk.
    private func attach(_ urls: [URL], inline: Bool = false) async {
        let found = await Task.detached(priority: .userInitiated) { () -> [(URL, Int64, Bool)] in
            urls.map { u in
                let v = try? u.resourceValues(forKeys: [.fileSizeKey, .isDirectoryKey])
                return (u, Int64(v?.fileSize ?? 0), v?.isDirectory ?? false)
            }
        }.value
        for (url, size, isDir) in found {
            if isDir {
                model.toast = "\(url.lastPathComponent) is a folder. Compress it first, then attach the .zip file."
                continue
            }
            context.attachments.add(ComposeAttachment(name: url.lastPathComponent, size: size, source: .file(url), inline: inline))
        }
    }

    /// ⌘V in the address or subject fields with files or an image on the clipboard
    /// attaches them; the body editor handles its own pastes.
    private func installPasteMonitor() {
        guard pasteMonitor == nil else { return }
        pasteMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                  event.charactersIgnoringModifiers == "v",
                  !(event.window?.firstResponder is BodyTextView) else { return event }
            let pb = NSPasteboard.general
            let files = (pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
            if !files.isEmpty {
                Task { @MainActor in await attach(files) }
                return nil
            }
            let hasText = pb.availableType(from: [.string]) != nil
            guard !hasText, let type = pb.availableType(from: [.png, .tiff]), let data = pb.data(forType: type) else { return event }
            Task { @MainActor in await attachPastedImage(data) }
            return nil
        }
    }

    private func attachPastedImage(_ data: Data) async {
        if let url = await Self.savePastedImage(data) { await attach([url], inline: true) }
    }

    /// Writes a pasted image to a PNG in the temporary folder, off the main thread.
    static func savePastedImage(_ data: Data) async -> URL? {
        await Task.detached(priority: .userInitiated) { () -> URL? in
            guard let png = NSBitmapImageRep(data: data)?.representation(using: .png, properties: [:]) else { return nil }
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("Mermoz Pasted", isDirectory: true)
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let stamp = Date().formatted(.iso8601.year().month().day().time(includingFractionalSeconds: false))
                .replacingOccurrences(of: ":", with: ".")
            let url = dir.appendingPathComponent("Pasted image \(stamp).png")
            do { try png.write(to: url) } catch { return nil }
            return url
        }.value
    }

    private func field<C: View>(_ label: String, @ViewBuilder content: () -> C) -> some View {
        HStack(alignment: .center, spacing: 10) {
            Text(label).font(.system(size: 13)).foregroundStyle(Theme.text3).frame(width: 56, alignment: .leading)
            content()
            Spacer(minLength: 0)
        }
        .frame(minHeight: 38)
        .overlay(alignment: .bottom) { Rectangle().fill(Color(hex: "#E3EDF6")).frame(height: 1) }
    }
}

/// Pills plus an inline text field; commits on comma, space, return or blur.
struct RecipientField: View {
    @Environment(AppModel.self) private var model
    @Binding var addresses: [String]
    @State private var draft = ""
    @FocusState private var focused: Bool

    var body: some View {
        FlowRow(spacing: 6) {
            ForEach(addresses, id: \.self) { a in
                RecipientPill(address: a, verdict: model.checker.check(a)) {
                    addresses.removeAll { $0 == a }
                }
            }
            TextField(addresses.isEmpty ? "name@example.com" : "", text: $draft)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .frame(minWidth: 140)
                .focused($focused)
                .onSubmit(commit)
                .onChange(of: draft) { _, v in
                    if v.hasSuffix(",") || v.hasSuffix(" ") || v.hasSuffix(";") { commit() }
                }
                .onChange(of: focused) { _, f in if !f { commit() } }
                .onKeyPress(.delete) {
                    if draft.isEmpty, !addresses.isEmpty { addresses.removeLast(); return .handled }
                    return .ignored
                }
        }
        .padding(.vertical, 6)
    }

    private func commit() {
        let parts = draft.split(whereSeparator: { ",; ".contains($0) }).map { String($0).trimmingCharacters(in: .whitespaces) }
        for p in parts where !p.isEmpty && !addresses.contains(p) { addresses.append(p) }
        draft = ""
    }
}

struct RecipientPill: View {
    let address: String
    let verdict: RecipientVerdict
    let onRemove: () -> Void
    @State private var showReason = false

    var body: some View {
        HStack(spacing: 5) {
            Text(address).lineLimit(1)
            mark
            Button(action: onRemove) { Image(systemName: "xmark").font(.system(size: 8, weight: .bold)) }
                .buttonStyle(.plain).foregroundStyle(Theme.text3).accessibilityLabel("Remove \(address)")
        }
        .font(.system(size: 12.5))
        .foregroundStyle(isBad ? Color(hex: "#9A2B2B") : Theme.text)
        .padding(.leading, 10).padding(.trailing, 8)
        .frame(height: 24)
        .background(isBad ? Color(hex: "#FFF7F7") : Color.white.opacity(0.9), in: Capsule())
        .overlay(Capsule().strokeBorder(isBad ? Color(hex: "#F3D3D3") : Color(hex: "#DCE8F3")))
        .onHover { h in if isBad { showReason = h } }
        .onTapGesture { if isBad { showReason.toggle() } }
        .popover(isPresented: $showReason, arrowEdge: .bottom) {
            if case let .suspicious(reason) = verdict {
                VStack(alignment: .leading, spacing: 3) {
                    Text("This address looks suspicious").font(.system(size: 12, weight: .semibold)).foregroundStyle(Color(hex: "#9A2B2B"))
                    Text(reason).font(.system(size: 12)).foregroundStyle(Color(hex: "#5B2020"))
                }
                .padding(11).frame(width: 260, alignment: .leading)
            }
        }
    }

    private var isBad: Bool {
        if case .suspicious = verdict { return true }
        return false
    }

    @ViewBuilder private var mark: some View {
        switch verdict {
        case .known:
            Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.ok)
                .accessibilityLabel("Address looks good")
        case .unknown:
            EmptyView()
        case .suspicious:
            Image(systemName: "exclamationmark.circle").font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.danger)
                .accessibilityLabel("Suspicious address")
        }
    }
}

/// Under a To line that has an address nobody reads: says so, and offers the person
/// named in the mail when there is one.
struct NoReplyBanner: View {
    let warning: NoReplyWarning
    /// Swaps the no-reply addresses for the suggestion.
    var useSuggestion: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Theme.danger)
            VStack(alignment: .leading, spacing: 2) {
                Text(NoReplyWarning.title).font(.system(size: 12.5, weight: .semibold))
                Text(warning.detail).font(.system(size: 12)).foregroundStyle(Theme.text2)
            }
            Spacer(minLength: 8)
            if let q = warning.question {
                Button(q, action: useSuggestion)
                    .buttonStyle(.link).font(.system(size: 12))
                    .help("Write to this address instead")
            }
        }
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
    }
}
