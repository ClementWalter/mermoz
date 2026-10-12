/// Settings window: General, Accounts, Agents, and a separate Developer section.
import AppKit
import MermozKit
import SwiftUI

enum SettingsPane: String, CaseIterable, Identifiable {
    case general = "General"
    case accounts = "Accounts"
    case agents = "Agents"
    case developer = "Developer"
    var id: String { rawValue }

    var icon: String {
        switch self {
        case .general: "gearshape"
        case .accounts: "envelope"
        case .agents: "sparkle"
        case .developer: "chevron.left.forwardslash.chevron.right"
        }
    }
}

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var pane: SettingsPane = .agents

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                ForEach([SettingsPane.general, .accounts, .agents]) { navButton($0) }
                Spacer()
                // Developer lives apart, at the bottom, so everyday users never stumble on it.
                navButton(.developer).foregroundStyle(Theme.text3)
            }
            .padding(.horizontal, 10).padding(.vertical, 12)
            .frame(width: 210)
            .frame(maxHeight: .infinity)
            .glassPanel()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    Text(pane.rawValue).font(.system(size: 20, weight: .semibold))
                    switch pane {
                    case .general: GeneralPane()
                    case .accounts: AccountsPane()
                    case .agents: AgentsPane()
                    case .developer: DeveloperPane()
                    }
                }
                .padding(.horizontal, 26).padding(.vertical, 22)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .paperPanel()
        }
        // Same chrome as the main window: traffic lights in their own strip, panels below.
        .padding(.horizontal, 10).padding(.bottom, 10).padding(.top, 36)
        .ignoresSafeArea(edges: .top)
        .foregroundStyle(Theme.text)
        .frame(minWidth: 900, minHeight: 620)
        .background(Theme.ground)
    }

    private func navButton(_ p: SettingsPane) -> some View {
        Button { pane = p } label: {
            HStack(spacing: 9) {
                // Fixed icon column so labels line up whatever the symbol's width.
                Image(systemName: p.icon).font(.system(size: 13)).frame(width: 18, alignment: .center)
                Text(p.rawValue)
                Spacer(minLength: 0)
            }
            .font(.system(size: 13, weight: pane == p ? .semibold : .regular))
            .padding(.horizontal, 8).frame(height: 30)
            .background(pane == p ? Theme.accent.opacity(0.13) : .clear, in: RoundedRectangle(cornerRadius: 8))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

struct SettingsCard<Content: View>: View {
    let title: String
    var subtitle: String? = nil
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title).font(.system(size: 13.5, weight: .semibold))
            if let subtitle { Text(subtitle).font(.system(size: 12)).foregroundStyle(Theme.text3) }
            content()
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Color(hex: "#E3EDF6")))
    }
}

struct GeneralPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        SettingsCard(title: "Storage", subtitle: "All your mail and attachments are kept on this Mac so Mermoz works offline.") {
            Button("Show in Finder") { NSWorkspace.shared.open(DaemonPaths.home) }.buttonStyle(.glass)
        }
    }
}

struct AccountsPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        SettingsCard(title: "Mail accounts") {
            ForEach(model.accounts) { a in
                HStack(spacing: 10) {
                    Circle().fill(Theme.accountColor(a.id, among: model.accounts)).frame(width: 9, height: 9)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(a.email).font(.system(size: 13, weight: .semibold))
                        Text(syncText(a)).font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                    }
                    Spacer()
                    Button("Remove", role: .destructive) { model.removeAccount(a.id) }.buttonStyle(.glass).controlSize(.small)
                }
                .padding(10)
                .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 10))
            }
            Button { model.addGmail() } label: { Label("Add Gmail account", systemImage: "plus") }
                .buttonStyle(.glass)
            Text("Mermoz opens your browser so you can sign in with Google.")
                .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
        }
    }

    private func syncText(_ a: Account) -> String {
        switch a.sync.state {
        case "error": return a.sync.error ?? "Can't sync right now"
        case "initial", "syncing":
            guard a.sync.total > 0 else { return "Getting ready…" }
            return "Downloading \(a.sync.synced.formatted()) of \(a.sync.total.formatted())"
        default: return "Up to date · available offline"
        }
    }
}

struct AgentsPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ProvidersCard()
        if !model.providers.isEmpty {
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 14), GridItem(.flexible(), spacing: 14)], alignment: .leading, spacing: 14) {
                ForEach(model.accounts) { a in AccountAgentCard(account: a) }
            }
        }
        SettingsCard(title: "Sending") {
            Toggle(isOn: Binding(get: { model.agentsMaySend }, set: { model.setAgentsMaySend($0) })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Agents may send without review").font(.system(size: 13, weight: .semibold))
                    Text("Off: every email an agent wants to send waits in Review for you.")
                        .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                }
                // Label takes the row so the switch sits on the right edge.
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .toggleStyle(.switch)
            Text("Only you can turn this on, here in the app. Agents can never approve their own emails.")
                .font(.system(size: 12)).foregroundStyle(Color(hex: "#5E2343"))
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(hex: "#FBEFF5"), in: RoundedRectangle(cornerRadius: 9))
        }
    }
}

/// Models offered for Anthropic providers; any other id can be typed.
private let claudeModels: [(id: String, label: String, short: String)] = [
    ("claude-opus-5-5", "Claude Opus 5.5 — best quality", "Opus 5.5"),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5 — balanced", "Sonnet 5.5"),
    ("claude-haiku-5-5", "Claude Haiku 5.5 — fastest, about 40× cheaper", "Haiku 5.5"),
]

struct ProvidersCard: View {
    @Environment(AppModel.self) private var model
    @State private var editing: Provider?
    @State private var advanced = false
    @State private var testResult: [String: String] = [:]

    var body: some View {
        SettingsCard(title: "AI", subtitle: "Mermoz uses AI to clean your messages and build your directory. Your mail only goes to the AI you allow for each account.") {
            ForEach(model.providers) { p in providerRow(p) }
            if let e = model.login.error {
                Text(e).font(.system(size: 11.5)).foregroundStyle(Color(hex: "#9B2C2C")).lineLimit(2)
            }
            if !model.providers.contains(where: { $0.kind == "chatgpt" }) {
                signInRow(title: "Connect ChatGPT", subtitle: "Uses your ChatGPT Plus or Pro plan. No key, no extra bill.",
                          button: "Continue with ChatGPT", tile: AnyView(ChatGPTTile())) { model.signInWithChatGPT() }
            }
            if !model.providers.contains(where: { $0.kind == "anthropic" && $0.auth == "oauth" }) {
                signInRow(title: "Connect Claude", subtitle: "Sign in with your Anthropic account (API credits).",
                          button: "Sign in with Anthropic", tile: AnyView(LogoTile())) { model.signInWithAnthropic() }
            }
            if model.login.running {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Finish signing in in your browser…").font(.system(size: 12)).foregroundStyle(Theme.text2)
                }
            }
            if let editing {
                ProviderForm(existing: editing) { self.editing = nil }
            } else if advanced {
                ProviderForm(existing: nil) { advanced = false }
            } else {
                Button("Use an API key or another provider instead") { advanced = true }
                    .buttonStyle(.plain).font(.system(size: 12)).foregroundStyle(Theme.accent)
            }
        }
        .task { await model.loadAgent() }
    }

    private func signInRow(title: String, subtitle: String, button: String, tile: AnyView, action: @escaping () -> Void) -> some View {
        HStack(spacing: 14) {
            tile
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 14, weight: .semibold))
                Text(subtitle).font(.system(size: 12)).foregroundStyle(Theme.text2)
            }
            Spacer()
            Button(button, action: action)
                // Stays clickable: a new click restarts a browser flow that got stuck.
                .buttonStyle(.glassProminent).tint(button.contains("ChatGPT") ? Color.black : Theme.accent).controlSize(.large)
        }
        .padding(14)
        .background(Theme.accentSoft.opacity(0.55), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(hex: "#D3E5F6")))
    }

    private func providerRow(_ p: Provider) -> some View {
        HStack(spacing: 12) {
            if p.kind == "chatgpt" { ChatGPTTile(small: true) } else { LogoTile(small: true) }
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text(p.name).font(.system(size: 13, weight: .semibold))
                    Pill(text: p.hasKey == true ? "Connected" : "Needs a key", ok: p.hasKey == true)
                }
                if let r = testResult[p.id] { Text(r).font(.system(size: 11.5)).foregroundStyle(Theme.text2) }
            }
            Spacer()
            if p.kind == "chatgpt" {
                Menu {
                    ForEach(model.providerModels[p.id] ?? []) { m in
                        Button(m.name.isEmpty ? m.id : m.name) {
                            Task { _ = await model.saveProvider(id: p.id, name: p.name, kind: p.kind, baseUrl: nil, model: m.id, apiKey: nil) }
                        }
                    }
                } label: {
                    Text(model.providerModels[p.id]?.first { $0.id == p.model }?.name ?? p.model).font(.system(size: 12))
                }
                .menuStyle(.button).fixedSize()
                .task { await model.loadModels(for: p) }
            } else if p.kind == "anthropic" {
                Menu {
                    ForEach(claudeModels, id: \.id) { m in
                        Button(m.label) {
                            Task { _ = await model.saveProvider(id: p.id, name: p.name, kind: p.kind, baseUrl: p.baseUrl, model: m.id, apiKey: nil) }
                        }
                    }
                } label: {
                    Text(claudeModels.first { $0.id == p.model }?.short ?? p.model).font(.system(size: 12))
                }
                .menuStyle(.button).fixedSize()
            }
            Menu {
                Button("Test connection") {
                    testResult[p.id] = "Testing…"
                    Task {
                        let t = await model.testProvider(p.id)
                        testResult[p.id] = t.map { $0.ok ? "Works ✓" : ($0.error ?? "Failed") } ?? "Failed"
                    }
                }
                if p.auth != "oauth" { Button("Edit…") { editing = p } }
                Divider()
                if p.kind == "chatgpt" {
                    Button("Usage & limits in ChatGPT…") { NSWorkspace.shared.open(URL(string: "https://chatgpt.com/#settings/Usage")!) }
                }
                Button(p.auth == "oauth" ? "Sign out" : "Remove", role: .destructive) { model.deleteProvider(p.id) }
            } label: {
                Image(systemName: "ellipsis")
            }
            .menuStyle(.button).menuIndicator(.hidden).fixedSize()
            .accessibilityLabel("More")
        }
        .padding(12)
        .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.line))
    }
}

struct LogoTile: View {
    var small = false

    var body: some View {
        RoundedRectangle(cornerRadius: small ? 8 : 10)
            .fill(LinearGradient(colors: [Color(hex: "#D97757"), Color(hex: "#C2603F")], startPoint: .top, endPoint: .bottom))
            .overlay(Image(systemName: "sparkle").font(.system(size: small ? 13 : 17, weight: .semibold)).foregroundStyle(.white))
            .frame(width: small ? 30 : 40, height: small ? 30 : 40)
            .accessibilityHidden(true)
    }
}

struct ChatGPTTile: View {
    var small = false

    var body: some View {
        RoundedRectangle(cornerRadius: small ? 8 : 10)
            .fill(Color.black)
            .overlay(Image(systemName: "bubble.left.and.text.bubble.right.fill").font(.system(size: small ? 12 : 15)).foregroundStyle(.white))
            .frame(width: small ? 30 : 40, height: small ? 30 : 40)
            .accessibilityHidden(true)
    }
}

struct Pill: View {
    let text: String
    let ok: Bool

    var body: some View {
        Text(text)
            .font(.system(size: 10.5, weight: .semibold))
            .padding(.horizontal, 7).padding(.vertical, 2)
            .foregroundStyle(ok ? Color(hex: "#1E6B49") : Color(hex: "#9B2C2C"))
            .background(ok ? Color(hex: "#E3F4EC") : Color(hex: "#FDEEEE"), in: Capsule())
    }
}

struct ProviderForm: View {
    @Environment(AppModel.self) private var model
    let existing: Provider?
    let done: () -> Void
    @State private var name = ""
    @State private var kind = "anthropic"
    @State private var modelId = "claude-opus-5-5"
    @State private var baseUrl = ""
    @State private var apiKey = ""
    @State private var saving = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            TextField("Name (e.g. Personal AI, Work AI)", text: $name)
            Picker("Type", selection: $kind) {
                Text("Anthropic (Claude)").tag("anthropic")
                Text("OpenAI-compatible").tag("openai")
            }
            .pickerStyle(.segmented)
            if kind == "anthropic" {
                Picker("Model", selection: $modelId) {
                    ForEach(claudeModels, id: \.id) { Text($0.label).tag($0.id) }
                }
            } else {
                TextField("Model", text: $modelId)
                TextField("Base URL (e.g. https://api.example.com/v1)", text: $baseUrl)
            }
            SecureField(existing?.hasKey == true ? "API key (leave empty to keep the current one)" : "API key", text: $apiKey)
            HStack {
                Button("Cancel") { done() }.buttonStyle(.glass)
                Button(saving ? "Saving…" : "Save") {
                    saving = true
                    Task {
                        let ok = await model.saveProvider(id: existing?.id, name: name, kind: kind,
                                                          baseUrl: kind == "openai" ? baseUrl : nil, model: modelId,
                                                          apiKey: apiKey.isEmpty ? nil : apiKey)
                        saving = false
                        if ok { done() }
                    }
                }
                .buttonStyle(.glassProminent).tint(Theme.accent)
                .disabled(name.isEmpty || modelId.isEmpty || (existing == nil && apiKey.isEmpty))
            }
        }
        .textFieldStyle(.roundedBorder)
        .padding(12)
        .background(Color(hex: "#F6FAFE"), in: RoundedRectangle(cornerRadius: 10))
        .onAppear {
            if let e = existing {
                name = e.name; kind = e.kind; modelId = e.model; baseUrl = e.baseUrl ?? ""
            }
        }
        .onChange(of: kind) { _, k in if k == "anthropic", !claudeModels.contains(where: { $0.id == modelId }) { modelId = "claude-opus-5-5" } }
    }
}

struct AccountAgentCard: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let state = model.agentStates[account.id]
        let allowed = Set(state?.providerIds ?? [])
        SettingsCard(title: account.email, subtitle: "Which providers may read this account's mail.") {
            FlowRow(spacing: 6) {
                chip("Any provider", on: allowed.contains("*")) { toggle("*", allowed) }
                ForEach(model.providers) { p in
                    chip(p.name, on: allowed.contains(p.id) && !allowed.contains("*")) { toggle(p.id, allowed) }
                }
            }
            if allowed.isEmpty {
                Text("None: this account's mail never leaves your Mac.").font(.system(size: 11.5)).foregroundStyle(Theme.text3)
            }
            Toggle(isOn: Binding(get: { state?.digestEnabled ?? false },
                                 set: { model.setAgent(account: account.id, digestEnabled: $0) })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Clean messages & build directory").font(.system(size: 13, weight: .semibold))
                    Text("Removes signatures, disclaimers and clutter, and saves contact details from signatures to People. New mail and the last 30 days.")
                        .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                }
                // Label takes the row so the switch sits on the right edge.
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .toggleStyle(.switch)
            .disabled(allowed.isEmpty)
            if let s = state, s.digestEnabled {
                Text("\(s.done.formatted()) cleaned · \(s.pending.formatted()) waiting\(s.failed > 0 ? " · \(s.failed) skipped" : "")")
                    .font(.system(size: 11.5)).foregroundStyle(Theme.text2)
                if let e = s.error { Text(e).font(.system(size: 11.5)).foregroundStyle(Color(hex: "#9B2C2C")).lineLimit(2) }
            }
        }
    }

    private func toggle(_ id: String, _ allowed: Set<String>) {
        var next = allowed
        if id == "*" {
            next = allowed.contains("*") ? [] : ["*"]
        } else {
            next.remove("*")
            if next.contains(id) { next.remove(id) } else { next.insert(id) }
        }
        model.setAgent(account: account.id, providerIds: Array(next).sorted())
    }

    private func chip(_ title: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                if on { Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)) }
                Text(title)
            }
            .font(.system(size: 12))
            .padding(.horizontal, 10).frame(height: 24)
            .foregroundStyle(on ? Color(hex: "#164F82") : Theme.text2)
            .background(on ? Theme.accentSoft : Color(hex: "#F6FAFE"), in: Capsule())
            .overlay(Capsule().strokeBorder(on ? Color(hex: "#CFE0F2") : Theme.line))
        }
        .buttonStyle(.plain)
    }
}

struct DeveloperPane: View {
    @Environment(AppModel.self) private var model
    @State private var installMessage: String?

    var body: some View {
        Text("For people who drive Mermoz from their own scripts and agents. Nothing here is needed for everyday use.")
            .font(.system(size: 12)).foregroundStyle(Theme.text3)
        InstantSortingCard()
        SettingsCard(title: "Command line tool", subtitle: "Same mailbox, same speed, from your shell. Every command supports --json.") {
            Text("$ mermoz search \"from:marc audit\" --json\n$ mermoz thread M-1287\n$ mermoz draft reply M-1287 --file reply.txt\n$ mermoz send <draft>   # waits in Review")
                .font(.system(size: 11.5, design: .monospaced))
                .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(hex: "#F2F7FC"), in: RoundedRectangle(cornerRadius: 9))
                .textSelection(.enabled)
            HStack {
                Button("Install in PATH") { installMessage = CLIInstaller.install() }.buttonStyle(.glass)
                if let installMessage { Text(installMessage).font(.system(size: 12)).foregroundStyle(Theme.text2) }
            }
        }
        SettingsCard(title: "Agent apps (MCP)", subtitle: "Give coding agents and assistants native Mermoz tools.") {
            Text("mermoz mcp").font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                .padding(8).background(Color(hex: "#F2F7FC"), in: RoundedRectangle(cornerRadius: 8))
        }
        SettingsCard(title: "Agent feedback", subtitle: "Friction reported by agents with `mermoz feedback`.") {
            if model.feedback.isEmpty {
                Text("No feedback yet").font(.system(size: 12.5)).foregroundStyle(Theme.text3)
            }
            ForEach(model.feedback) { f in
                VStack(alignment: .leading, spacing: 2) {
                    Text(f.text).font(.system(size: 12.5))
                    Text([f.agent, f.command, Format.short(f.created)].compactMap { $0 }.joined(separator: " · "))
                        .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                }
                Divider()
            }
        }
        .task { await model.loadFeedback() }
    }
}

/// Bring-your-own TypeSafe key: new mail gets a suggested status and project in a
/// fraction of a second.
struct InstantSortingCard: View {
    @Environment(AppModel.self) private var model
    @State private var key = ""
    @State private var busy = false
    @State private var result: String?
    @State private var confirmResort = false

    var body: some View {
        SettingsCard(title: "Instant sorting (TypeSafe)",
                     subtitle: "Suggests a status and a project for each new email in about a quarter second, and works through older mail in your open columns in the background. Each email is sent to TypeSafe to be sorted; nothing moves until you accept.") {
            if model.router.configured {
                HStack(spacing: 8) {
                    Circle().fill(model.router.error == nil ? Theme.ok : Theme.danger).frame(width: 8, height: 8)
                    Text(model.router.error ?? "On — new mail is sorted as it arrives").font(.system(size: 12.5))
                    Spacer()
                    Button("Test") { Task { await test(nil) } }.buttonStyle(.glass).disabled(busy)
                    Button("Remove key", role: .destructive) { Task { _ = await model.setRouterKey(nil) } }.buttonStyle(.glass)
                }
            } else {
                HStack(spacing: 8) {
                    SecureField("TypeSafe API key", text: $key)
                        .textFieldStyle(.plain)
                        .padding(.horizontal, 10).frame(height: 30)
                        .background(Color(hex: "#F2F7FC"), in: RoundedRectangle(cornerRadius: 8))
                    Button("Turn on") { Task { await save() } }
                        .buttonStyle(.glassProminent)
                        .disabled(key.trimmingCharacters(in: .whitespaces).isEmpty || busy)
                }
            }
            if model.router.configured, let cap = model.router.dailyCap {
                HStack(spacing: 8) {
                    Text("Older mail sorted today: \((model.router.callsToday ?? 0).formatted()) emails")
                        .font(.system(size: 12.5)).foregroundStyle(Theme.text2)
                    Spacer()
                    Picker("Daily limit", selection: Binding(get: { cap }, set: { model.setDailyCap($0) })) {
                        ForEach(Self.caps(including: cap), id: \.self) { Text($0 == 0 ? "Off" : "\($0.formatted()) a day").tag($0) }
                    }
                    .fixedSize()
                }
            }
            if model.router.configured {
                HStack(spacing: 8) {
                    if let r = model.router.resort {
                        Text(ResortText.progress(r)).font(.system(size: 12.5)).foregroundStyle(Theme.text2)
                    } else {
                        Text("Take a fresh look at every email, archived ones included.")
                            .font(.system(size: 12.5)).foregroundStyle(Theme.text2)
                    }
                    Spacer()
                    Button("Sort my whole mailbox again") { confirmResort = true }
                        .buttonStyle(.glass)
                        .disabled(model.router.resort != nil)
                }
                .confirmationDialog("Sort your whole mailbox again?", isPresented: $confirmResort) {
                    Button("Sort everything again") { Task { await model.resortMailbox() } }
                } message: {
                    Text("Every email, archived ones included, gets a fresh suggestion of where it belongs. Nothing moves until you accept, and emails you already turned a suggestion down for are left alone. It runs in the background for a few hours, past the daily limit, and costs a few dollars on your TypeSafe account.")
                }
            }
            if let result { Text(result).font(.system(size: 12)).foregroundStyle(Theme.text2) }
        }
        .task { await model.loadRouter() }
    }

    /// Daily limits offered for background sorting, plus the current one if custom.
    static func caps(including cap: Int) -> [Int] {
        Array(Set([0, 2_000, 5_000, 20_000, 50_000, cap])).sorted()
    }

    /// Checks the key with one tiny request before keeping it.
    private func save() async {
        let k = key.trimmingCharacters(in: .whitespaces)
        guard await test(k) else { return }
        if await model.setRouterKey(k) { key = "" }
    }

    @discardableResult
    private func test(_ k: String?) async -> Bool {
        busy = true
        defer { busy = false }
        guard let r = await model.testRouter(key: k) else { result = nil; return false }
        result = "Working · answered in \(r.ms) ms"
        return r.ok
    }
}

/// Symlinks the bundled CLI into a PATH directory the user can write to.
enum CLIInstaller {
    static func install() -> String {
        guard let src = Bundle.main.resourceURL?.appendingPathComponent("mermoz"),
              FileManager.default.isExecutableFile(atPath: src.path) else {
            return "The command line tool isn't bundled in this build."
        }
        let fm = FileManager.default
        let candidates = [URL(fileURLWithPath: "/usr/local/bin"),
                          fm.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin")]
        for dir in candidates {
            if dir.path.hasPrefix(fm.homeDirectoryForCurrentUser.path) {
                try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
            }
            guard fm.isWritableFile(atPath: dir.path) else { continue }
            let dst = dir.appendingPathComponent("mermoz")
            try? fm.removeItem(at: dst)
            do {
                try fm.createSymbolicLink(at: dst, withDestinationURL: src)
                return "Installed at \(dst.path)"
            } catch { continue }
        }
        return "Couldn't find a writable folder in your PATH."
    }
}
