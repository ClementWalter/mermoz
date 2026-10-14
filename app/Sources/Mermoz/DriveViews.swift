/// Google Drive pieces of the interface: preview cards under messages that link to
/// Drive, "Save to Drive" beside attachments, the Docs tab hint, the Settings card,
/// and "Add from Google Drive" in the composer.
import AppKit
import MermozKit
import SwiftUI

// MARK: linked document cards

/// Cards for the Drive files a message links to.
struct DriveLinkCards: View {
    @Environment(AppModel.self) private var model
    let messageId: String

    var body: some View {
        if let d = model.detail, let files = model.drive.threadFiles[d.thread.id] {
            let links = files.links(in: messageId)
            if !links.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(links) { l in DriveLinkCard(link: l, threadId: d.thread.id, accountId: files.accountId) }
                }
            }
        }
    }
}

struct DriveLinkCard: View {
    @Environment(AppModel.self) private var model
    let link: DriveLinkItem
    let threadId: String
    let accountId: String
    @State private var thumbnail: NSImage?

    var body: some View {
        HStack(spacing: 11) {
            icon
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 12.5, weight: .semibold)).lineLimit(1)
                Text(subtitle).font(.system(size: 11.5)).foregroundStyle(Theme.text3).lineLimit(1)
            }
            Spacer(minLength: 8)
            action
        }
        .padding(9)
        .frame(maxWidth: 460, alignment: .leading)
        .background(Color.white, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color(hex: "#DCE8F3")))
        .contentShape(Rectangle())
        .onTapGesture { open() }
        .help("Open in Google Drive")
        .task(id: link.file?.id) {
            if let f = link.file, f.hasThumbnail { thumbnail = await model.driveThumbnail(accountId: accountId, fileId: f.id) }
        }
    }

    private var state: DriveCardState { DriveCardState(link) }

    private var title: String {
        if case let .preview(f) = state { return f.name }
        return "Google Drive file"
    }

    private var subtitle: String {
        switch state {
        case let .preview(f):
            var parts = [f.kind]
            if let o = f.owner { parts.append(o.name.isEmpty ? o.email : o.name) }
            if f.modified > 0 { parts.append("edited \(Format.short(f.modified))") }
            return parts.joined(separator: " · ")
        case .connect: return "Connect Google Drive to see what it is"
        case .allow: return "Mermoz needs your OK in Google to open it"
        case .unavailable: return "Google Drive didn't answer. Click to open it in your browser."
        }
    }

    @ViewBuilder private var icon: some View {
        if let thumbnail {
            Image(nsImage: thumbnail).resizable().scaledToFill()
                .frame(width: 40, height: 40).clipShape(RoundedRectangle(cornerRadius: 6))
        } else {
            Image(systemName: symbol).font(.system(size: 17)).foregroundStyle(Theme.accent)
                .frame(width: 40, height: 40).background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 6))
        }
    }

    private var symbol: String {
        guard case let .preview(f) = state else { return "doc" }
        switch f.kind {
        case "Google Sheet", "Spreadsheet": return "tablecells"
        case "Google Slides", "Presentation": return "rectangle.on.rectangle"
        case "Folder": return "folder"
        case "Image": return "photo"
        case "PDF": return "doc.richtext"
        default: return "doc.text"
        }
    }

    @ViewBuilder private var action: some View {
        switch state {
        case .connect:
            Button("Connect") {
                model.withDrive(accountId, purpose: "preview documents linked in your mail") {
                    Task { await model.loadDrive(threadId: threadId) }
                }
            }
            .buttonStyle(.glass).controlSize(.small)
        case .allow:
            if model.drive.allowing.contains(link.fileId) {
                ProgressView().controlSize(.small).help("Waiting for your OK in the browser")
            } else {
                Button("Allow…") { model.allowDriveFile(link.fileId, threadId: threadId, accountId: accountId) }
                    .buttonStyle(.glass).controlSize(.small)
                    .help("Google shows this one file so you can let Mermoz open it")
            }
        case .preview, .unavailable:
            EmptyView()
        }
    }

    private func open() {
        let url = (link.file?.openURL) ?? URL(string: link.url)
        if let url { NSWorkspace.shared.open(url) }
    }
}

// MARK: save to Drive

/// One click saves the attachment into Mermoz/<project> in Drive; once saved it opens there.
struct SaveToDriveButton: View {
    @Environment(AppModel.self) private var model
    let attachment: Attachment

    var body: some View {
        if let d = model.detail {
            let saved = model.drive.threadFiles[d.thread.id]?.saved(attachment.id)
            Group {
                if model.drive.saving.contains(attachment.id) {
                    ProgressView().controlSize(.mini).help("Saving to Google Drive…")
                } else if let saved {
                    Button { if let u = saved.openURL { NSWorkspace.shared.open(u) } } label: {
                        Label("Open in Drive", systemImage: "checkmark.icloud")
                    }
                    .help(saved.savedLine)
                } else {
                    Button { model.saveToDrive(attachment, threadId: d.thread.id, accountId: d.thread.accountId) } label: {
                        Label("Save to Drive", systemImage: "icloud.and.arrow.up")
                    }
                    .help("Save to your Google Drive, in a folder named after this conversation's project")
                }
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.plain)
            .font(.system(size: 13))
            .foregroundStyle(saved == nil ? Theme.text3 : Theme.ok)
            .frame(width: 22, height: 30)
        }
    }
}

// MARK: Docs tab

/// Above the Docs tab: what Drive can add for this conversation, when it isn't yet.
struct DriveDocsHint: View {
    @Environment(AppModel.self) private var model
    let threadId: String
    let accountId: String

    var body: some View {
        if let text {
            HStack(spacing: 10) {
                Image(systemName: "externaldrive.badge.icloud").foregroundStyle(Theme.accent)
                Text(text).font(.system(size: 12)).foregroundStyle(Theme.text2).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                Button("Connect Google Drive") {
                    model.withDrive(accountId, purpose: "show the Google Drive files of your conversations") {
                        Task { await model.loadDrive(threadId: threadId) }
                    }
                }
                .buttonStyle(.glass).controlSize(.small)
            }
            .padding(.horizontal, 16).padding(.vertical, 10)
            .background(Theme.accentSoft.opacity(0.5))
        }
    }

    private var text: String? {
        guard let s = model.driveStatus(accountId) else { return nil }
        if !s.connected { return "Also show Google Drive documents linked here or shared with these people." }
        return nil
    }
}

// MARK: Settings

struct DriveSettingsCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        SettingsCard(title: "Google Drive",
                     subtitle: "Optional, per account. Mermoz asks Google separately, so your mail works the same whether you connect Drive or not.") {
            ForEach(model.accounts.filter { !$0.isDemo }) { a in
                let s = model.drive.statuses[a.id]
                HStack(spacing: 10) {
                    Image(systemName: s?.connected == true ? "checkmark.icloud" : "icloud")
                        .foregroundStyle(s?.connected == true ? Theme.ok : Theme.text3)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(a.email).font(.system(size: 13, weight: .semibold))
                        Text(s?.summary ?? "").font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer()
                    if s?.isWaiting == true {
                        ProgressView().controlSize(.small)
                    }
                    if s?.connected == true {
                        if s?.canReadAll == false || s?.canSave == false {
                            Button("Connect again") { model.connectDrive(a.id) }.buttonStyle(.glass).controlSize(.small)
                        }
                        Button("Disconnect") { model.disconnectDrive(a.id) }.buttonStyle(.glass).controlSize(.small)
                    } else {
                        Button("Connect Google Drive") { model.connectDrive(a.id) }.buttonStyle(.glass).controlSize(.small)
                    }
                }
                .padding(10)
                .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 10))
            }
            Text("Disconnecting removes Drive from Mermoz on this Mac. To remove it from your Google account as well, open your Google account's security settings.")
                .font(.system(size: 11.5)).foregroundStyle(Theme.text3).fixedSize(horizontal: false, vertical: true)
        }
        .task { await model.refreshDriveStatus() }
    }
}

// MARK: compose

/// "Add from Google Drive": recent files and search, or Google's own file browser.
/// Each file goes in as a link (sharing is asked when sending) or as a regular file,
/// downloaded; files come first when every recipient is outside the user's Google world.
struct DriveAttachButton: View {
    @Environment(AppModel.self) private var model
    @Binding var attachments: AttachmentList
    let accountId: String?
    /// Everyone the message goes to, for the link-or-file default.
    var recipients: [String] = []
    @State private var open = false
    @State private var mode: DriveAttachMode = .link

    var body: some View {
        Button {
            guard let accountId else { return }
            model.withDrive(accountId, purpose: "attach files from your Google Drive") {
                mode = DriveAttachMode.preferred(recipients: recipients, me: me, memory: DriveAttachMemory())
                open = true
            }
        } label: {
            Image(systemName: "externaldrive.badge.icloud").font(.system(size: 14, weight: .medium))
        }
        .buttonStyle(.plain).foregroundStyle(Theme.text2)
        .help("Add a file from Google Drive")
        .accessibilityLabel("Add from Google Drive")
        .popover(isPresented: $open, arrowEdge: .top) {
            if let accountId {
                VStack(alignment: .leading, spacing: 0) {
                    Picker("Attach", selection: Binding(get: { mode }, set: { choose($0) })) {
                        Text("As a file").tag(DriveAttachMode.file)
                        Text("As a link").tag(DriveAttachMode.link)
                    }
                    .pickerStyle(.segmented).labelsHidden()
                    .padding([.horizontal, .top], 12)
                    .help(mode == .file ? "The file itself goes with the email (Google Docs as PDF)."
                        : "Recipients open it in Google Drive; you'll be asked whether to share it.")
                    DrivePicker(accountId: accountId) { files in add(files, accountId: accountId) }
                }
            }
        }
    }

    private var me: String? { model.accounts.first { $0.id == accountId }?.email }

    /// The segmented choice is remembered for this kind of audience.
    private func choose(_ m: DriveAttachMode) {
        mode = m
        DriveAttachMemory().remember(m, outside: DriveAttachMode.allOutside(recipients: recipients, me: me))
    }

    private func add(_ files: [DriveFile], accountId: String) {
        for f in files {
            guard let u = f.openURL else { continue }
            let item = ComposeAttachment(drive: DriveOrigin(fileId: f.id, name: f.name, mime: f.mime, link: u))
            if attachments.add(item), mode == .file {
                model.attachDriveAsFile(item.id, accountId: accountId, in: $attachments)
            }
        }
        open = false
    }
}

/// A composer chip for a Drive file: link or file, with a menu to switch, the
/// download in progress, and a plain-words error with "Keep as link" when it failed.
struct DriveAttachmentChip: View {
    @Environment(AppModel.self) private var model
    let item: ComposeAttachment
    @Binding var attachments: AttachmentList
    let accountId: String?
    var recipients: [String] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 2) {
                AttachmentChip(filename: item.name, size: item.size, isLink: item.isLink,
                               detail: item.fetch == .downloading ? "Downloading…" : nil) {
                    attachments.remove(id: item.id)
                }
                Menu {
                    menuItems
                } label: {
                    Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold))
                }
                .menuStyle(.button).menuIndicator(.hidden).buttonStyle(.plain)
                .foregroundStyle(Theme.text3).fixedSize()
                .disabled(item.fetch == .downloading)
                .accessibilityLabel("Change how “\(item.name)” is sent")
            }
            .contextMenu { menuItems }
            if case let .failed(why) = item.fetch {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(why).font(.system(size: 11.5)).foregroundStyle(Theme.danger)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Keep as link") { attachments.keepAsLink(id: item.id) }
                        .buttonStyle(.link).font(.system(size: 11.5))
                }
                .frame(maxWidth: 280, alignment: .leading)
                .accessibilityElement(children: .combine)
            }
        }
    }

    @ViewBuilder private var menuItems: some View {
        if item.isLink {
            let formats = DriveAttachMode.formats(mime: item.drive?.mime ?? "")
            if formats.count > 1 {
                ForEach(formats, id: \.self) { f in
                    Button("Attach as \(DriveAttachMode.formatLabel(f))") { asFile(format: f) }
                }
            } else {
                Button("Attach as a file") { asFile(format: nil) }
            }
        } else {
            Button("Send as a link instead") {
                attachments.keepAsLink(id: item.id)
                remember(.link)
            }
        }
        if let link = item.drive?.link {
            Button("Open in Google Drive") { NSWorkspace.shared.open(link) }
        }
    }

    private func asFile(format: String?) {
        guard let accountId else { return }
        remember(.file)
        model.attachDriveAsFile(item.id, format: format, accountId: accountId, in: $attachments)
    }

    private func remember(_ m: DriveAttachMode) {
        let me = model.accounts.first { $0.id == accountId }?.email
        DriveAttachMemory().remember(m, outside: DriveAttachMode.allOutside(recipients: recipients, me: me))
    }
}

struct DrivePicker: View {
    @Environment(AppModel.self) private var model
    let accountId: String
    let onPick: ([DriveFile]) -> Void
    @State private var query = ""
    @State private var files: [DriveFile] = []
    @State private var message: String?
    @State private var browsing = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if model.driveStatus(accountId)?.canReadAll == true {
                TextField("Search your Drive", text: $query)
                    .textFieldStyle(.plain)
                    .padding(.horizontal, 10).frame(height: 30)
                    .background(Theme.accent.opacity(0.07), in: RoundedRectangle(cornerRadius: 9))
                    .task(id: query) {
                        try? await Task.sleep(for: .milliseconds(200))
                        await search()
                    }
                Text(query.isEmpty ? "Recently opened" : "Results").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.text3)
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(files) { f in
                            Button { onPick([f]) } label: {
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(f.name).font(.system(size: 13)).lineLimit(1)
                                    Text([f.kind, f.modified > 0 ? Format.short(f.modified) : nil].compactMap { $0 }.joined(separator: " · "))
                                        .font(.system(size: 11)).foregroundStyle(Theme.text3)
                                }
                                .padding(.vertical, 5).padding(.horizontal, 6)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .frame(height: 220)
                if let message { Text(message).font(.system(size: 11.5)).foregroundStyle(Theme.text3) }
                Divider()
            }
            Button {
                browsing = true
                Task {
                    let picked = await model.pickDriveFiles(accountId: accountId)
                    browsing = false
                    if !picked.isEmpty { onPick(picked) }
                }
            } label: {
                HStack(spacing: 6) {
                    if browsing { ProgressView().controlSize(.mini) }
                    Text(browsing ? "Choose in your browser…" : "Browse Google Drive…")
                }
            }
            .buttonStyle(.glass)
            .disabled(browsing)
            .help("Opens Google Drive's own file chooser in your browser")
        }
        .padding(12).frame(width: 340)
    }

    private func search() async {
        do {
            files = try await model.searchDrive(accountId: accountId, query: query)
            message = files.isEmpty ? (query.isEmpty ? "No recent files." : "Nothing found.") : nil
        } catch {
            message = (error as? RPCError)?.message ?? "Google Drive didn't answer."
        }
    }
}
