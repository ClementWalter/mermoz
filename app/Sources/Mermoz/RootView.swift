/// Window layout: glass sidebar, issue list or board, and the reading/compose pane.
import MermozKit
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model
    @State private var keyMonitor: Any?

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
            // The palette shows at once: a fade laid the whole window out again on each of its frames.
            if model.paletteMode != nil {
                CommandPalette()
            }
            if model.showShortcuts {
                ShortcutsOverlay().transition(.opacity)
            }
            // On a Feed card the picker is a popover; elsewhere a small panel.
            if let r = model.later, r.cardId == nil {
                LaterPanel(request: r).transition(.opacity)
            }
        }
        .overlay(alignment: .bottom) { ToastView() }
        .sheet(item: Binding(get: { model.drive.connectPrompt }, set: { model.drive.connectPrompt = $0 })) {
            DriveConnectSheet(prompt: $0)
        }
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
        .animation(.easeOut(duration: 0.12), value: model.showShortcuts)
        .onAppear { if keyMonitor == nil { keyMonitor = ThreadKeys.install(model: model) } }
        .onDisappear { if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil } }
    }
}

struct MainLayout: View {
    @Environment(AppModel.self) private var model
    // Pane widths are remembered across launches; drag the gaps between panes to change them.
    @AppStorage("layout.sidebarWidth") private var sidebarWidth = 228.0
    @AppStorage("layout.listWidth") private var listWidth = 420.0

    var body: some View {
        @Bindable var model = model
        HStack(spacing: 0) {
            SidebarView()
                .layoutBoundary()
                .frame(width: sidebarWidth)
            ResizeHandle(width: $sidebarWidth, range: 180...360)
            if model.selection == .people {
                PeopleListView().frame(width: listWidth)
                ResizeHandle(width: $listWidth, range: 300...760)
                ContactDetailPane().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if model.selection == .sort {
                SortView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if model.selection == .docs {
                DocsListView().frame(width: listWidth)
                ResizeHandle(width: $listWidth, range: 300...760)
                DetailPane().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if model.showsFeed {
                FeedView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .inspector(isPresented: Binding(get: { model.hasSelection || model.compose != nil },
                                                    set: { if !$0 { model.open(nil); model.compose = nil } })) {
                        DetailPane().layoutBoundary().inspectorColumnWidth(min: 380, ideal: 560, max: 900)
                    }
            } else if model.layout == .board && !model.isReview {
                BoardView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .inspector(isPresented: Binding(get: { model.hasSelection || model.compose != nil },
                                                    set: { if !$0 { model.open(nil); model.compose = nil } })) {
                        DetailPane().layoutBoundary().inspectorColumnWidth(min: 380, ideal: 560, max: 900)
                    }
            } else {
                Group {
                    if model.isReview { ReviewListView() } else { ThreadListView() }
                }
                .layoutBoundary()
                .frame(width: listWidth)
                ResizeHandle(width: $listWidth, range: 300...760)
                DetailPane().layoutBoundary()
            }
        }
        // The traffic lights get their own strip so they never collide with a panel corner.
        .padding(.horizontal, 10).padding(.bottom, 10).padding(.top, 36)
        .ignoresSafeArea(edges: .top)
    }
}

/// The gap between two panes; dragging it resizes the pane on its left.
struct ResizeHandle: View {
    @Binding var width: Double
    let range: ClosedRange<Double>
    var thickness: CGFloat = 10
    @State private var start: Double?
    @State private var hovering = false

    var body: some View {
        Rectangle()
            .fill(Color.clear)
            .frame(width: thickness)
            .overlay {
                // A faint guide appears on hover so the handle is discoverable.
                Capsule().fill(Theme.accent.opacity(hovering || start != nil ? 0.35 : 0)).frame(width: 3, height: 36)
            }
            .contentShape(Rectangle())
            .onHover { inside in
                hovering = inside
                if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() }
            }
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { v in
                        if start == nil { start = width }
                        width = min(range.upperBound, max(range.lowerBound, (start ?? width) + v.translation.width))
                    }
                    .onEnded { _ in start = nil }
            )
            .onTapGesture(count: 2) { width = range.lowerBound + (range.upperBound - range.lowerBound) * 0.3 }
            .accessibilityLabel("Resize")
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
                // The chat sits beside the thread so both stay readable while deciding.
                HStack(spacing: 0) {
                    ThreadDetailView(detail: d).frame(minWidth: 320)
                    if model.chatOpen {
                        Divider().overlay(Theme.line)
                        ChatPanel(thread: d.thread).frame(minWidth: 260, idealWidth: 340, maxWidth: 380)
                    }
                }
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
            Text(model.isReview ? "Nothing waiting for your approval" : "Select a conversation")
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
            GmailSignInStatusView()
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

extension View {
    /// Fills the space offered and never reports its content's size back up, so a change
    /// inside (another conversation, a moved selection) lays out this pane alone instead of
    /// the whole window.
    func layoutBoundary() -> some View {
        Color.clear.frame(maxWidth: .infinity, maxHeight: .infinity).overlay { self }
    }
}
