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
            CommandMenu("Go") {
                // Disabled while composing or searching so ⌘← keeps moving the text cursor there.
                Button("Back") { model.goBack() }
                    .keyboardShortcut(.leftArrow, modifiers: .command)
                    .disabled(!model.canGoBack || model.compose != nil || model.paletteMode != nil)
                Button("Forward") { model.goForward() }
                    .keyboardShortcut(.rightArrow, modifiers: .command)
                    .disabled(!model.canGoForward || model.compose != nil || model.paletteMode != nil)
            }
            // ⌘Z undoes the last change to threads, unless a text field is being edited.
            CommandGroup(replacing: .undoRedo) {
                Button(model.undoLabel.map { "Undo \($0)" } ?? "Undo") {
                    if NSApp.keyWindow?.firstResponder is NSText {
                        NSApp.sendAction(Selector(("undo:")), to: nil, from: nil)
                    } else {
                        model.undo()
                    }
                }
                .keyboardShortcut("z", modifiers: .command)
                Button("Redo") { NSApp.sendAction(Selector(("redo:")), to: nil, from: nil) }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
            }
            CommandMenu("Thread") {
                Button("Search or Ask…") { model.paletteMode = .search }.keyboardShortcut("k")
                Button("Reply") { model.startReply() }.keyboardShortcut("r", modifiers: [.command])
                Button("Mark Done") { model.markDoneSelected() }.keyboardShortcut("e", modifiers: [.command])
                // Disabled while writing or searching, so ⌘⌫ keeps deleting text in fields.
                Button("Delete") { model.moveSelected(to: "deleted") }
                    .keyboardShortcut(.delete, modifiers: [.command])
                    .disabled(model.selectedThreadId == nil || model.compose != nil || model.paletteMode != nil)
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
