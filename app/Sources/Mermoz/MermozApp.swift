/// App entry point: main window and Settings scene sharing one model.
import AppKit
import SwiftUI

@main
struct MermozApp: App {
    @State private var model = AppModel()

    init() {
        // SwiftPM executables launch as background tools unless promoted to a regular app.
        NSApplication.shared.setActivationPolicy(.regular)
    }

    var body: some Scene {
        WindowGroup("Mermoz") {
            RootView()
                .environment(model)
                .preferredColorScheme(.light)
                .frame(minWidth: 980, minHeight: 620)
                .task { await model.start() }
                .onAppear { NSApplication.shared.activate() }
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .appSettings) {
                SettingsCommand()
            }
            CommandGroup(replacing: .newItem) {
                Button("New Message") { model.startNew() }.keyboardShortcut("n")
            }
            CommandMenu("Thread") {
                Button("Search or Ask…") { model.paletteMode = .search }.keyboardShortcut("k")
                Button("Reply") { model.startReply() }.keyboardShortcut("r", modifiers: [.command])
                Button("Mark Done") { model.markDoneSelected() }.keyboardShortcut("e", modifiers: [.command])
                Divider()
                Button("Keyboard Shortcuts") { model.showShortcuts.toggle() }.keyboardShortcut("/")
                Divider()
                Picker("Layout", selection: $model.layout) {
                    ForEach(LayoutMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
            }
        }

        // A regular window (not the Settings scene) so it can share the main window's chrome.
        Window("Mermoz Settings", id: "settings") {
            SettingsView()
                .environment(model)
                .preferredColorScheme(.light)
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 980, height: 680)
    }
}

/// ⌘, opens the Settings window.
struct SettingsCommand: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button("Settings…") { openWindow(id: "settings") }.keyboardShortcut(",")
    }
}
