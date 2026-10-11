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
    @State private var pane: SettingsPane = .accounts

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                ForEach([SettingsPane.general, .accounts, .agents]) { navButton($0) }
                Spacer()
                // Developer lives apart, at the bottom, so everyday users never stumble on it.
                navButton(.developer).foregroundStyle(Theme.text3)
            }
            .padding(10)
            .frame(width: 200)
            .glassPanel(radius: 0)

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
                .padding(26)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Theme.paper)
        }
        .foregroundStyle(Theme.text)
        .frame(width: 760, height: 520)
        .background(Theme.ground)
    }

    private func navButton(_ p: SettingsPane) -> some View {
        Button { pane = p } label: {
            Label(p.rawValue, systemImage: p.icon)
                .font(.system(size: 13, weight: pane == p ? .semibold : .regular))
                .padding(.horizontal, 8).frame(height: 30)
                .frame(maxWidth: .infinity, alignment: .leading)
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
        SettingsCard(title: "Sending") {
            Toggle(isOn: Binding(get: { model.agentsMaySend }, set: { model.setAgentsMaySend($0) })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Agents may send without review").font(.system(size: 13, weight: .semibold))
                    Text("Off: every email an agent wants to send waits in Review for you.")
                        .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                }
            }
            .toggleStyle(.switch)
            Text("Only you can turn this on, here in the app. Agents can never approve their own emails.")
                .font(.system(size: 12)).foregroundStyle(Color(hex: "#5E2343"))
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(hex: "#FBEFF5"), in: RoundedRectangle(cornerRadius: 9))
        }
        SettingsCard(title: "AI providers", subtitle: "Coming soon: connect your own AI and choose which accounts it may read.") {
            Label("Built-in agent is not available yet", systemImage: "sparkle").font(.system(size: 12.5)).foregroundStyle(Theme.text3)
        }
    }
}

struct DeveloperPane: View {
    @Environment(AppModel.self) private var model
    @State private var installMessage: String?

    var body: some View {
        Text("For people who drive Mermoz from their own scripts and agents. Nothing here is needed for everyday use.")
            .font(.system(size: 12)).foregroundStyle(Theme.text3)
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
