/// Window layout: glass sidebar, issue list or board, and the reading/compose pane.
import MermozKit
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        ZStack {
            Theme.ground.ignoresSafeArea()
            switch model.connection {
            case .connecting:
                ProgressView().controlSize(.small)
            case let .unreachable(msg):
                UnreachableView(message: msg)
            case .connected:
                if model.accounts.isEmpty {
                    WelcomeView()
                } else {
                    MainLayout()
                }
            }
            if model.paletteMode != nil {
                CommandPalette().transition(.opacity)
            }
            if model.showShortcuts {
                ShortcutsOverlay().transition(.opacity)
            }
        }
        .overlay(alignment: .bottom) { ToastView() }
        .background {
            // Window-level Esc: closes the palette, then the open message. Compose owns Esc
            // (discard) while it is open, so this one steps aside to avoid a shortcut clash.
            if model.compose == nil {
                Button("Close") { model.escape() }
                    .keyboardShortcut(.cancelAction)
                    .opacity(0).frame(width: 0, height: 0)
                    .accessibilityHidden(true)
            }
        }
        .animation(.easeOut(duration: 0.12), value: model.paletteMode)
        .animation(.easeOut(duration: 0.12), value: model.showShortcuts)
    }
}

struct MainLayout: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        HStack(spacing: 10) {
            SidebarView()
                .frame(width: 228)
            if model.layout == .board && !model.isReview {
                BoardView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .inspector(isPresented: Binding(get: { model.selectedThreadId != nil || model.compose != nil },
                                                    set: { if !$0 { model.open(nil); model.compose = nil } })) {
                        DetailPane().inspectorColumnWidth(min: 440, ideal: 560, max: 760)
                    }
            } else {
                Group {
                    if model.isReview { ReviewListView() } else { ThreadListView() }
                }
                .frame(width: 420)
                DetailPane().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        // The traffic lights get their own strip so they never collide with a panel corner.
        .padding(.horizontal, 10).padding(.bottom, 10).padding(.top, 36)
        .ignoresSafeArea(edges: .top)
    }
}

/// Right pane: compose takes priority, then a review draft, then the open thread.
struct DetailPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let c = model.compose {
                ComposeView(context: c).id(c.id)
            } else if model.isReview, let d = model.selectedDraft {
                ReviewDetailView(draft: d)
            } else if let d = model.detail {
                ThreadDetailView(detail: d)
            } else {
                EmptyDetail()
            }
        }
        .paperPanel()
    }
}

struct EmptyDetail: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: model.isReview ? "checkmark.seal" : "tray")
                .font(.system(size: 28, weight: .light))
                .foregroundStyle(Theme.text3)
            Text(model.isReview ? "Nothing waiting for your approval" : "Select a thread")
                .foregroundStyle(Theme.text3)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct WelcomeView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "envelope.open")
                .font(.system(size: 34, weight: .light))
                .foregroundStyle(Theme.accent)
            Text("Welcome to Mermoz").font(.system(size: 22, weight: .semibold)).foregroundStyle(Theme.text)
            Text("Fast, offline-ready mail where nothing gets lost.")
                .foregroundStyle(Theme.text2)
            Button {
                model.addGmail()
            } label: {
                Label("Add your Gmail account", systemImage: "plus")
                    .padding(.horizontal, 6)
            }
            .buttonStyle(.glassProminent)
            .controlSize(.large)
            .tint(Theme.accent)
            Text("Mermoz opens your browser so you can sign in with Google.")
                .font(.system(size: 12)).foregroundStyle(Theme.text3)
        }
        .padding(40)
        .frame(width: 440)
        .glassPanel(radius: 22)
    }
}

struct UnreachableView: View {
    @Environment(AppModel.self) private var model
    let message: String

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "bolt.horizontal.circle").font(.system(size: 28, weight: .light)).foregroundStyle(Theme.text3)
            Text(message).foregroundStyle(Theme.text2).multilineTextAlignment(.center)
            Button("Try again") { Task { await model.start() } }.buttonStyle(.glass)
        }
        .padding(32)
        .frame(width: 380)
        .glassPanel(radius: 20)
    }
}

struct ToastView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let t = model.toast {
            Text(t)
                .font(.system(size: 12.5, weight: .medium))
                .foregroundStyle(Theme.text)
                .padding(.horizontal, 14).padding(.vertical, 8)
                .glassEffect(.regular, in: Capsule())
                .padding(.bottom, 18)
                .task(id: t) {
                    try? await Task.sleep(for: .seconds(2.5))
                    if model.toast == t { model.toast = nil }
                }
        }
    }
}
