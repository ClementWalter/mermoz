/// Standalone iPhone mailbox with an app-owned local demo store.
import SwiftUI

@main
struct MermozApp: App {
    @State private var model: MailboxModel

    init() {
        let support = URL.applicationSupportDirectory.appending(path: "Mermoz/Demo", directoryHint: .isDirectory)
        _model = State(initialValue: MailboxModel(directory: support))
    }

    var body: some Scene {
        WindowGroup { MailboxView(model: model) }
    }
}
