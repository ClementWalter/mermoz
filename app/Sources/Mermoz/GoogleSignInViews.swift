/// Google mail sign-in help: why Google refused (company accounts), the client ID an
/// admin trusts, and signing in through the user's own Google sign-in client.
import AppKit
import MermozKit
import SwiftUI
import UniformTypeIdentifiers

/// Where a Workspace admin trusts an app.
private let adminConsolePath = "Google Admin console › Security › API controls › App access control"

/// Shown under "Add Gmail account": waiting for the browser, or why it didn't work.
struct GmailSignInStatusView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @State private var blockedHelp = false
    /// False where the own-client form already is (Settings › Developer).
    var offerOwnClient = true

    var body: some View {
        let s = model.gmailSignIn
        if s.isOutlook {
            OutlookSignInStatusView(state: s)
        } else if s.running {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Finish signing in in your browser…").font(.system(size: 12)).foregroundStyle(Theme.text2)
                }
                // Google shows its refusals on its own page, without coming back to Mermoz.
                DisclosureGroup("Google says access is blocked?", isExpanded: $blockedHelp) {
                    explanation(
                        "With a work account, your organization may only allow apps it trusts. Ask your Google Workspace admin to trust Mermoz (client ID below), or use your own sign-in client.",
                        clientId: s.appClientId
                    )
                }
                .font(.system(size: 12))
            }
        } else if let error = s.error {
            VStack(alignment: .leading, spacing: 4) {
                Text(s.problem == nil ? "Couldn't add the account" : "Google didn't let Mermoz in")
                    .font(.system(size: 12.5, weight: .semibold))
                explanation(error, clientId: s.problem?.clientId)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(hex: "#FFF6E5"), in: RoundedRectangle(cornerRadius: 8))
        }
    }

    @ViewBuilder
    private func explanation(_ text: String, clientId: String?) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(text).font(.system(size: 12)).foregroundStyle(Theme.text2)
                .fixedSize(horizontal: false, vertical: true)
            if let clientId {
                HStack(spacing: 8) {
                    Text(clientId).font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled).lineLimit(1).truncationMode(.middle)
                    Button("Copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(clientId, forType: .string)
                    }
                    .buttonStyle(.glass).controlSize(.small)
                }
                Text("Your admin adds it in the \(adminConsolePath).")
                    .font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if offerOwnClient {
                Button("Use your own sign-in client") {
                    model.settingsPane = .developer
                    openWindow(id: "settings")
                }
                .buttonStyle(.glass).controlSize(.small)
            }
        }
    }
}

/// Under "Add Outlook account": waiting for the browser, or why it didn't work.
struct OutlookSignInStatusView: View {
    let state: GmailSignIn

    var body: some View {
        if state.running {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Finish signing in in your browser…").font(.system(size: 12)).foregroundStyle(Theme.text2)
            }
        } else if let error = state.error {
            VStack(alignment: .leading, spacing: 4) {
                Text("Couldn't add the account").font(.system(size: 12.5, weight: .semibold))
                Text(error).font(.system(size: 12)).foregroundStyle(Theme.text2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(hex: "#FFF6E5"), in: RoundedRectangle(cornerRadius: 8))
        }
    }
}

/// Settings › Developer: add a Gmail account through the user's own Google OAuth
/// client, e.g. one their company made "Internal" in its Google Cloud organization.
struct OwnGoogleClientCard: View {
    @Environment(AppModel.self) private var model
    @State private var choosing = false
    @State private var clientId = ""
    @State private var secret = ""
    @State private var fileError: String?

    var body: some View {
        SettingsCard(title: "Your own Google sign-in client",
                     subtitle: "For a Google Workspace that blocks Mermoz: sign in through an OAuth client (type Desktop app) from your organization's Google Cloud project. The account keeps that client for mail and Drive.") {
            HStack(spacing: 8) {
                Button("Choose client_secret_….json…") { choosing = true }.buttonStyle(.glass)
                if let fileError { Text(fileError).font(.system(size: 12)).foregroundStyle(.red) }
            }
            .fileImporter(isPresented: $choosing, allowedContentTypes: [.json]) { result in
                guard case let .success(url) = result else { return }
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                do {
                    fileError = nil
                    model.addGmail(ownClient: .file(try String(contentsOf: url, encoding: .utf8)))
                } catch { fileError = "Couldn't read that file." }
            }
            Text("Or paste the client's ID and secret:").font(.system(size: 12)).foregroundStyle(Theme.text3)
            TextField("Client ID", text: $clientId).textFieldStyle(.roundedBorder)
            SecureField("Client secret", text: $secret).textFieldStyle(.roundedBorder)
            Button("Sign in with this client") {
                model.addGmail(ownClient: .ids(clientId: clientId.trimmingCharacters(in: .whitespacesAndNewlines),
                                               secret: secret.trimmingCharacters(in: .whitespacesAndNewlines)))
            }
            .buttonStyle(.glassProminent)
            .disabled(clientId.trimmingCharacters(in: .whitespaces).isEmpty || secret.trimmingCharacters(in: .whitespaces).isEmpty)
            GmailSignInStatusView(offerOwnClient: false)
        }
    }
}
