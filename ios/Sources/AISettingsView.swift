/// Per-mailbox consent makes provider access visible before any email leaves the phone.
import SwiftUI

struct AISettingsView: View {
    @Bindable var model: MailboxModel
    @Environment(\.dismiss) private var dismiss
    @State private var kind = "anthropic"
    @State private var modelName = ""
    @State private var endpoint = ""
    @State private var key = ""
    @State private var sorter = "assistant"
    @State private var sorterKey = ""
    @State private var accounts: Set<String> = []
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Label("Your AI, inside Mermoz", systemImage: "sparkles").font(.headline)
                    Text("Your assistant turns mail into an action feed, prepares replies and helps you think through a conversation.")
                        .foregroundStyle(Theme.text2)
                }
                Section("Assistant") {
                    Picker("Provider", selection: $kind) {
                        Text("Anthropic").tag("anthropic")
                        Text("OpenAI compatible").tag("openai")
                    }
                    TextField("Model ID from your provider", text: $modelName)
                        .textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityIdentifier("ai-model")
                    SecureField("API key", text: $key).textInputAutocapitalization(.never)
                        .autocorrectionDisabled().accessibilityIdentifier("ai-key")
                    if kind == "openai" {
                        TextField("API base URL (optional)", text: $endpoint).keyboardType(.URL)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                    }
                    Text("Use an API key for now. Desktop subscription sign-ins are not connected on iPhone.")
                        .font(.caption).foregroundStyle(Theme.text3)
                }
                Section("Instant sorting") {
                    Picker("Judge conversations with", selection: $sorter) {
                        Text("My assistant").tag("assistant")
                        Text("TypeSafe (Jev)").tag("typesafe")
                        Text("SubQ").tag("subq")
                    }
                    if sorter != "assistant" {
                        SecureField("Sorting API key", text: $sorterKey).textInputAutocapitalization(.never).autocorrectionDisabled()
                    }
                    Text("Mermoz finds dates and amounts locally. AI judges what needs attention; the shared Feed rules order the cards.")
                        .font(.caption).foregroundStyle(Theme.text3)
                }
                Section("Mailboxes this AI may read") {
                    ForEach(model.accounts.filter { !$0.hidden && $0.provider != "demo" }) { account in
                        Toggle(account.email, isOn: Binding(get: { accounts.contains(account.id) }, set: { allowed in
                            if allowed { accounts.insert(account.id) } else { accounts.remove(account.id) }
                        }))
                    }
                    Text("Selected mail, recent conversation history and your questions are sent directly to your chosen provider. TypeSafe or SubQ also receives conversation text when selected. Keys stay in this iPhone’s Keychain. AI never sends a reply without your confirmation.")
                        .font(.caption).foregroundStyle(Theme.text2)
                    Link("How Mermoz handles your data", destination: URL(string: "https://mermoz.xyz/privacy/")!)
                }
                if model.isDemo { Section { Text("Connect Gmail in Accounts to use AI with your own mail. The demo contains fictional examples of the Feed and prepared replies.") } }
                if let error { Section { Text(error).foregroundStyle(.red) } }
                Section {
                    Button("Connect AI and allow selected mailboxes") { Task { await save() } }
                        .disabled(busy || key.isEmpty || modelName.isEmpty || accounts.isEmpty)
                        .accessibilityIdentifier("save-ai")
                    if model.ai?.enabled == true {
                        Button("Disconnect AI", role: .destructive) { Task {
                            do { try await model.configureAI(nil); dismiss() }
                            catch { self.error = error.localizedDescription }
                        } }.disabled(busy)
                    }
                }
            }
            .mermozSurface()
            .navigationTitle("AI")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { Button("Done") { dismiss() }.disabled(busy) }
            .task {
                do {
                    if let saved = try AIKeychain().load() {
                        kind = saved.provider.kind; modelName = saved.provider.model
                        endpoint = saved.provider.baseUrl ?? ""; key = saved.key
                        accounts = Set(saved.accounts); sorter = saved.sorter ?? "assistant"; sorterKey = saved.sorterKey
                    }
                } catch { self.error = error.localizedDescription }
            }
        }.tint(Theme.accent)
    }

    private func save() async {
        busy = true
        defer { busy = false }
        let provider = AIProvider(name: kind == "anthropic" ? "Anthropic" : "OpenAI compatible", kind: kind,
                                  baseUrl: kind == "openai" ? (endpoint.isEmpty ? "https://api.openai.com/v1" : endpoint) : nil,
                                  model: modelName.trimmingCharacters(in: .whitespacesAndNewlines))
        do {
            try await model.configureAI(AIConfiguration(provider: provider, key: key.trimmingCharacters(in: .whitespacesAndNewlines),
                                                        accounts: accounts.sorted(), sorter: sorter == "assistant" ? nil : sorter, sorterKey: sorterKey))
            dismiss()
        } catch { self.error = error.localizedDescription }
    }
}
