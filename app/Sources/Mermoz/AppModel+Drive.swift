/// Google Drive in the app: connecting it per account, previews of linked documents,
/// the Drive files of a conversation, saving attachments, and attaching and sharing
/// Drive files in a reply. Sharing only ever happens after the user says yes.
import AppKit
import MermozKit
import os
import SwiftUI

private let log = Logger(subsystem: "ai.mermoz.app", category: "drive")

/// A Drive feature was used before Drive was connected: ask once, in plain words.
struct DriveConnectPrompt: Identifiable, Equatable {
    var id: String { accountId }
    let accountId: String
    /// What the user was trying to do, e.g. "save “report.pdf” to your Google Drive".
    let purpose: String
}

@MainActor
@Observable
final class DriveModel {
    /// Per account id.
    var statuses: [String: DriveStatus] = [:]
    /// Per thread id: the conversation's linked, shared and saved Drive files.
    var threadFiles: [String: DriveThreadFiles] = [:]
    /// Attachments being saved right now.
    var saving: Set<String> = []
    var thumbnails: [String: NSImage] = [:]
    var connectPrompt: DriveConnectPrompt?
    /// Files being opened up for Mermoz in Google's picker ("Allow").
    var allowing: Set<String> = []
    /// What to do once the account's Drive is connected (the action that asked).
    @ObservationIgnored var afterConnect: [String: () -> Void] = [:]
}

extension AppModel {
    func driveStatus(_ accountId: String?) -> DriveStatus? { accountId.flatMap { drive.statuses[$0] } }

    func refreshDriveStatus() async {
        guard let client else { return }
        do {
            let all = try await client.driveStatus()
            drive.statuses = Dictionary(uniqueKeysWithValues: all.map { ($0.accountId, $0) })
        } catch { report(error) }
    }

    /// Opens Google's consent for Drive in the browser.
    func connectDrive(_ accountId: String) {
        guard let client else { return }
        Task {
            do {
                let start = try await client.driveConnect(accountId: accountId)
                if let u = URL(string: start.authUrl) { NSWorkspace.shared.open(u) }
                await refreshDriveStatus()
            } catch { report(error) }
        }
    }

    func disconnectDrive(_ accountId: String) {
        guard let client else { return }
        Task {
            do {
                try await client.driveDisconnect(accountId: accountId)
                drive.threadFiles = drive.threadFiles.filter { $0.value.accountId != accountId }
                await refreshDriveStatus()
            } catch { report(error) }
        }
    }

    /// `drive_changed`: refresh what is shown, and resume the action that asked to connect.
    func driveChanged(_ accountId: String) {
        Task {
            await refreshDriveStatus()
            if drive.statuses[accountId]?.connected == true, let resume = drive.afterConnect.removeValue(forKey: accountId) {
                resume()
            }
            for (tid, f) in drive.threadFiles where f.accountId == accountId { await loadDrive(threadId: tid) }
        }
    }

    /// Asks to connect Drive before `action`; runs it at once when Drive is there.
    func withDrive(_ accountId: String, purpose: String, _ action: @escaping () -> Void) {
        if !DriveStatus.needsConnect(drive.statuses[accountId]) {
            action()
        } else {
            drive.afterConnect[accountId] = action
            drive.connectPrompt = DriveConnectPrompt(accountId: accountId, purpose: purpose)
        }
    }

    /// The conversation's Drive files, for link cards, the Docs tab and saved attachments.
    func loadDrive(threadId: String) async {
        guard let client else { return }
        do {
            let f = try await client.driveFiles(threadId: threadId)
            if drive.threadFiles[threadId] != f { drive.threadFiles[threadId] = f }
        } catch { log.error("drive files: \(error.localizedDescription)") }
    }

    func driveThumbnail(accountId: String, fileId: String) async -> NSImage? {
        if let i = drive.thumbnails[fileId] { return i }
        guard let client, let t = try? await client.driveThumbnail(accountId: accountId, fileId: fileId),
              let data = Data(base64Encoded: t.data), let image = NSImage(data: data) else { return nil }
        drive.thumbnails[fileId] = image
        return image
    }

    /// Saves an attachment into Mermoz/<project> in Drive; asks to connect Drive first if needed.
    func saveToDrive(_ a: Attachment, threadId: String, accountId: String) {
        withDrive(accountId, purpose: "save “\(a.filename)” to your Google Drive") { [weak self] in
            self?.performSave(a, threadId: threadId, accountId: accountId)
        }
    }

    private func performSave(_ a: Attachment, threadId: String, accountId: String) {
        guard let client, !drive.saving.contains(a.id) else { return }
        drive.saving.insert(a.id)
        Task {
            defer { drive.saving.remove(a.id) }
            do {
                let saved = try await client.driveSave(attachmentId: a.id)
                toast = saved.savedLine
                await loadDrive(threadId: threadId)
            } catch let e as RPCError {
                handleDrive(e, accountId: accountId, purpose: "save “\(a.filename)” to your Google Drive") {
                    self.performSave(a, threadId: threadId, accountId: accountId)
                }
            } catch { report(error) }
        }
    }

    /// Turns a Drive error into the one thing the user can do about it.
    private func handleDrive(_ e: RPCError, accountId: String, purpose: String, retry: @escaping () -> Void) {
        switch DriveNeed(e) {
        case .connect?:
            drive.afterConnect[accountId] = retry
            drive.connectPrompt = DriveConnectPrompt(accountId: accountId, purpose: purpose)
        case .save?, .readAll?:
            toast = e.message
        case .pick?, nil:
            report(e)
        }
    }

    /// Opens Google's file picker and waits for the user's choice (empty when cancelled).
    func pickDriveFiles(accountId: String, only fileIds: [String] = []) async -> [DriveFile] {
        guard let client else { return [] }
        do {
            let start = try await client.drivePick(accountId: accountId, fileIds: fileIds)
            if let u = URL(string: start.authUrl) { NSWorkspace.shared.open(u) }
            // The user is in the browser; Google's picker gives up after 10 minutes too.
            for _ in 0..<600 {
                try await Task.sleep(for: .seconds(1))
                let r = try await client.drivePickResult(start.pickId)
                switch r.state {
                case "waiting": continue
                case "done": return r.files
                case "error": toast = r.error ?? "Google Drive didn't answer. Try again."; return []
                default: return []
                }
            }
        } catch is CancellationError {
        } catch { report(error) }
        return []
    }

    /// "Allow" on a linked document Mermoz may not open yet: the user confirms that
    /// one file in Google's picker, then the card fills in.
    func allowDriveFile(_ fileId: String, threadId: String, accountId: String) {
        guard !drive.allowing.contains(fileId) else { return }
        drive.allowing.insert(fileId)
        Task {
            defer { drive.allowing.remove(fileId) }
            _ = await pickDriveFiles(accountId: accountId, only: [fileId])
            await loadDrive(threadId: threadId)
        }
    }

    func searchDrive(accountId: String, query: String) async throws -> [DriveFile] {
        guard let client else { return [] }
        return try await client.driveSearch(accountId: accountId, query: query)
    }

    /// Gives the recipients access to the reply's Drive files. A file Mermoz may not
    /// share yet goes through Google's picker once (the user confirms it there).
    /// Returns false when nothing could be shared, so the send can stop.
    func shareDriveFiles(_ plan: SharePlan, accountId: String, role: String) async -> Bool {
        guard let client else { return false }
        var failed: [String] = []
        for f in plan.files {
            do {
                let r = try await client.driveShare(accountId: accountId, fileId: f.id, emails: plan.emails, role: role)
                failed += r.failed.map(\.email)
            } catch let e as RPCError where DriveNeed(e) == .pick {
                let picked = await pickDriveFiles(accountId: accountId, only: [f.id])
                guard picked.contains(where: { $0.id == f.id }) else {
                    toast = "“\(f.name)” wasn't shared: it wasn't confirmed in Google Drive."
                    return false
                }
                do {
                    let r = try await client.driveShare(accountId: accountId, fileId: f.id, emails: plan.emails, role: role)
                    failed += r.failed.map(\.email)
                } catch { report(error); return false }
            } catch { report(error); return false }
        }
        if !failed.isEmpty { toast = "Couldn't give access to \(failed.joined(separator: ", ")). They may need a Google account." }
        return true
    }

    /// Turns a Drive item of the composer into a regular file: the daemon downloads it
    /// (exports Google files to `format`, PDF by default) and the chip shows its size,
    /// or says in plain words why it stays a link.
    func attachDriveAsFile(_ id: UUID, format: String? = nil, accountId: String, in list: Binding<AttachmentList>) {
        guard let client, let item = list.wrappedValue.items.first(where: { $0.id == id }),
              let origin = item.drive, item.fetch != .downloading else { return }
        list.wrappedValue.startDownload(id: id)
        Task {
            do {
                let d = try await client.driveDownload(accountId: accountId, fileId: origin.fileId, format: format)
                list.wrappedValue.downloaded(id: id, d)
            } catch let e as RPCError {
                list.wrappedValue.downloadFailed(id: id, e.message)
            } catch {
                log.error("drive download: \(error.localizedDescription)")
                list.wrappedValue.downloadFailed(id: id, "Couldn't download “\(origin.name)” from Google Drive. Try again, or keep it as a link.")
            }
        }
    }

    /// The Docs tab's Drive entries for the open conversation.
    func driveDocs(for d: ThreadDetail) -> [DocEntry] { drive.threadFiles[d.thread.id]?.docEntries ?? [] }
}

/// "Connect Google Drive?" asked the first time a Drive feature is used.
struct DriveConnectSheet: View {
    @Environment(AppModel.self) private var model
    let prompt: DriveConnectPrompt

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Connect Google Drive?", systemImage: "externaldrive.badge.icloud")
                .font(.system(size: 15, weight: .semibold))
            Text("To \(prompt.purpose), Mermoz needs your permission in Google. Your mail keeps working whatever you choose.")
                .font(.system(size: 13)).fixedSize(horizontal: false, vertical: true)
            if let email = model.accounts.first(where: { $0.id == prompt.accountId })?.email {
                Text("Account: \(email)").font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            HStack {
                Spacer()
                Button("Not now") {
                    model.drive.afterConnect[prompt.accountId] = nil
                    model.drive.connectPrompt = nil
                }
                .keyboardShortcut(.cancelAction)
                Button("Connect Google Drive") {
                    model.connectDrive(prompt.accountId)
                    model.drive.connectPrompt = nil
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.glassProminent).tint(Theme.accent)
            }
        }
        .padding(20).frame(width: 420)
    }
}
