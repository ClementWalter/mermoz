/// Account consent, sync progress and local data removal remain available offline.
import SwiftUI

struct AccountsView: View {
    @Bindable var model: MailboxModel
    @Environment(\.dismiss) private var dismiss
    @State private var removing: MailAccount?
    @State private var showingAI = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Mailboxes") {
                    ForEach(model.accounts.filter { !$0.hidden }) { account in
                        VStack(alignment: .leading, spacing: 6) {
                            Text(account.email).font(.headline)
                            if account.provider == "demo" { Text("Fictional mail, available offline").foregroundStyle(.secondary) }
                            else {
                                Text(account.sync.state == "syncing" ? "Syncing \(account.sync.synced) of \(account.sync.total) messages" : "Mail is available offline")
                                    .font(.subheadline).foregroundStyle(.secondary)
                                if let error = account.sync.error { Text(error).font(.caption).foregroundStyle(.red) }
                                Button("Reconnect") { Task { await model.connectGoogle() } }.disabled(model.connecting)
                            }
                            Button("Remove from this iPhone", role: .destructive) { removing = account }
                        }
                    }
                    Button(model.connecting ? "Connecting…" : "Connect Gmail") { Task { await model.connectGoogle() } }
                        .disabled(model.connecting).accessibilityIdentifier("connect-gmail")
                }
                if let error = model.accountError { Section { Text(error).foregroundStyle(.red) } }
                Section("Your mail stays with you") {
                    Text("Mermoz stores your mailbox on this iPhone. It syncs directly with Gmail while you use the app. No Mac or Mermoz server is needed.")
                    Button("AI assistant", systemImage: "sparkles") { showingAI = true }
                    Link("Privacy policy", destination: URL(string: "https://mermoz.xyz/privacy/")!)
                }
            }
            .navigationTitle("Accounts")
            .mermozSurface()
            .sheet(isPresented: $showingAI) { AISettingsView(model: model) }
            .toolbar { Button("Done") { dismiss() } }
            .confirmationDialog("Remove this account from Mermoz? Gmail itself won't be deleted.", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible) {
                if let removing { Button("Remove mailbox", role: .destructive) { Task { await model.removeAccount(removing); self.removing = nil } } }
            }
        }
    }
}

struct DraftsView: View {
    @Bindable var model: MailboxModel
    @Environment(\.dismiss) private var dismiss
    @State private var drafts: [MailDraft] = []
    @State private var error: String?

    var body: some View {
        NavigationStack {
            List(drafts) { draft in
                NavigationLink {
                    SavedDraftView(model: model, draft: draft)
                } label: {
                    VStack(alignment: .leading) {
                        Text(draft.subject).font(.headline)
                        Text(draft.body).lineLimit(2).foregroundStyle(.secondary)
                    }
                }
            }
            .overlay { if drafts.isEmpty { ContentUnavailableView("No saved drafts", systemImage: "square.and.pencil", description: error.map(Text.init)) } }
            .navigationTitle("Drafts")
            .toolbar { Button("Done") { dismiss() } }
            .task {
                do { drafts = try await model.mailbox.call("drafts.list") }
                catch { self.error = error.localizedDescription }
            }
        }
    }
}

private struct SavedDraftView: View {
    @Bindable var model: MailboxModel
    let draft: MailDraft
    @State private var detail: MailDetail?
    @State private var error: String?

    var body: some View {
        Group {
            if let detail { ReplyView(model: model, thread: detail.thread, savedDraft: draft) }
            else if let error { Text(error) }
            else { ProgressView("Opening draft…") }
        }
        .task {
            do {
                guard let id = draft.threadId else { throw MailboxError(message: "This draft has no conversation.") }
                detail = try await model.mailbox.call("threads.get", params: ["id": id])
            } catch { self.error = error.localizedDescription }
        }
    }
}
