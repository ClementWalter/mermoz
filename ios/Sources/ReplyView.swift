/// A reply is saved locally before an explicit send, so failed delivery keeps the draft.
import SwiftUI

struct MailDraft: Decodable, Identifiable, Sendable {
    let id: String
    let accountId: String
    let threadId: String?
    let to: [String]
    let cc: [String]
    let subject: String
    let body: String
    let state: String
}

struct ReplyView: View {
    @Bindable var model: MailboxModel
    let thread: MailThread
    @Environment(\.dismiss) private var dismiss
    @State private var bodyText = ""
    @State private var draft: MailDraft?
    @State private var busy = false
    @State private var error: String?
    @State private var confirmingSend = false
    @State private var confirmingClose = false

    init(model: MailboxModel, thread: MailThread, savedDraft: MailDraft? = nil, initialBody: String = "") {
        self.model = model
        self.thread = thread
        _draft = State(initialValue: savedDraft)
        _bodyText = State(initialValue: savedDraft?.body ?? initialBody)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text(thread.subject).font(.headline)
                    if let draft { Text("To: \(draft.to.joined(separator: ", "))").font(.subheadline) }
                    TextEditor(text: $bodyText).frame(minHeight: 240)
                        .accessibilityIdentifier("reply-body")
                        .disabled(busy || draft != nil)
                }
                if let error { Text(error).foregroundStyle(.red) }
                if let draft {
                    Section {
                        Text("Saved on this iPhone. Sending delivers this reply to the recipients shown above.")
                        if draft.state == "draft" {
                            Button("Send reply") { confirmingSend = true }
                                .disabled(busy).accessibilityIdentifier("send-reply")
                        } else {
                            Text("Delivery couldn't be confirmed. Check Gmail Sent before writing another reply.")
                                .foregroundStyle(.orange)
                        }
                        Button("Keep draft") { dismiss() }.disabled(busy)
                    }
                    .confirmationDialog("Send this reply?", isPresented: $confirmingSend, titleVisibility: .visible) {
                        Button("Send to \(draft.to.joined(separator: ", "))") { Task { await send(draft) } }
                    }
                }
            }
            .navigationTitle("Reply")
            .mermozSurface()
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") {
                        if draft == nil && !bodyText.isEmpty { confirmingClose = true }
                        else { dismiss() }
                    }.disabled(busy)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if draft == nil {
                        Button("Save draft") { Task { await save() } }
                            .disabled(busy || bodyText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
            }
            .interactiveDismissDisabled(busy || (!bodyText.isEmpty && draft == nil))
            .confirmationDialog("Keep this reply?", isPresented: $confirmingClose, titleVisibility: .visible) {
                Button("Save draft") { Task { await save(); if draft != nil { dismiss() } } }
                Button("Discard reply", role: .destructive) { dismiss() }
            }
        }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        do {
            draft = try await model.mailbox.call("drafts.reply", params: ["id": thread.id, "body": bodyText])
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    private func send(_ draft: MailDraft) async {
        busy = true
        defer { busy = false }
        do {
            let _: MailDraft = try await model.mailbox.call("drafts.send", params: ["id": draft.id])
            model.revision += 1
            dismiss()
        } catch {
            self.error = error.localizedDescription
            if let drafts: [MailDraft] = try? await model.mailbox.call("drafts.list") {
                self.draft = drafts.first { $0.id == draft.id } ?? draft
            }
        }
    }
}
