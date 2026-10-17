/// Contextual conversation AI streams proposals; a proposed reply still opens the composer.
import SwiftUI

struct ChatView: View {
    @Bindable var model: MailboxModel
    let thread: MailThread
    @Environment(\.dismiss) private var dismiss
    @State private var messages: [AIChatMessage] = []
    @State private var question = ""
    @State private var error: String?
    @State private var draftBody: String?
    @State private var showingReply = false
    @State private var showingSettings = false

    private var working: Bool { model.ai?.running == true && model.ai?.threadId == thread.id }

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        Text(thread.subject).font(.headline).foregroundStyle(Theme.text2)
                        if messages.isEmpty {
                            VStack(alignment: .leading, spacing: 10) {
                                Image(systemName: "sparkles").font(.title).foregroundStyle(Theme.accent)
                                Text("Think it through with Mermoz").font(.title2.weight(.semibold))
                                Text("Ask about the conversation, weigh a decision or prepare a reply. Your assistant has the mail in context.")
                                    .foregroundStyle(Theme.text2)
                                Button("Help me reply") { Task { await send("Help me reply to this conversation.") } }
                                    .buttonStyle(.bordered).disabled(working)
                            }.padding(18).paperPanel()
                        }
                        ForEach(messages) { message in
                            VStack(alignment: .leading, spacing: 10) {
                                Label(message.role == "user" ? "You" : "Mermoz", systemImage: message.role == "user" ? "person" : "sparkles")
                                    .font(.caption.weight(.semibold)).foregroundStyle(Theme.accent)
                                Text(.init(message.text)).textSelection(.enabled)
                                ForEach(Array(message.actions.enumerated()), id: \.offset) { _, action in
                                    if action.kind == "draft_reply", let body = action.body {
                                        Button(action.label.isEmpty ? "Review reply" : action.label) {
                                            draftBody = body; showingReply = true
                                        }.buttonStyle(.bordered)
                                    } else if action.kind == "status", let status = action.status {
                                        Button(action.label) { Task { await applyStatus(status) } }.buttonStyle(.bordered)
                                    }
                                }
                            }.padding(16).frame(maxWidth: .infinity, alignment: .leading).paperPanel()
                        }
                        if working {
                            if let text = model.ai?.text, !text.isEmpty {
                                Text(.init(text)).padding(16).frame(maxWidth: .infinity, alignment: .leading).paperPanel()
                            }
                            ProgressView("Mermoz is thinking…").font(.caption)
                        }
                        if let error = error ?? (model.ai?.threadId == thread.id ? model.ai?.error : nil) {
                            Text(error).font(.caption).foregroundStyle(.red)
                        }
                        if model.ai?.enabled != true {
                            Button("Connect your AI") { showingSettings = true }.buttonStyle(.borderedProminent)
                        }
                        Color.clear.frame(height: 1).id("bottom")
                    }.padding(18)
                }.onChange(of: messages.count) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .safeAreaInset(edge: .bottom) {
                HStack(alignment: .bottom, spacing: 12) {
                    TextField("Ask about this conversation…", text: $question, axis: .vertical)
                        .lineLimit(1...5).accessibilityIdentifier("chat-question")
                    Button { let text = question; question = ""; Task { await send(text) } } label: {
                        Image(systemName: "arrow.up.circle.fill").font(.title2)
                    }.disabled(working || question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityLabel("Ask Mermoz")
                }.padding().background(.ultraThinMaterial)
            }
            .background(Theme.ground).foregroundStyle(Theme.text)
            .navigationTitle("Chat about this").navigationBarTitleDisplayMode(.inline)
            .toolbar { Button("Done") { dismiss() } }
            .sheet(isPresented: $showingReply) { ReplyView(model: model, thread: thread, initialBody: draftBody ?? "") }
            .sheet(isPresented: $showingSettings) { AISettingsView(model: model) }
            .task {
                while !Task.isCancelled {
                    await reload()
                    do { try await Task.sleep(for: .milliseconds(500)) } catch { break }
                }
            }
        }.tint(Theme.accent)
    }

    private func reload() async {
        do {
            messages = try await model.mailbox.call("chat.list", params: ["id": thread.id])
            model.ai = try await model.mailbox.call("ai.status")
        } catch { self.error = error.localizedDescription }
    }

    private func send(_ text: String) async {
        do {
            let _: EmptyResult = try await model.mailbox.call("chat.send", params: ["id": thread.id, "question": text])
            error = nil; await reload()
        } catch { self.error = error.localizedDescription }
    }

    private func applyStatus(_ status: String) async {
        do {
            let _: MailThread = try await model.mailbox.call("threads.update", params: ["id": thread.id, "status": status])
            model.revision += 1
        } catch { self.error = error.localizedDescription }
    }
}
