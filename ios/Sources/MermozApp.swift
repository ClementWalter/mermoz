/// Standalone iPhone mailbox with app-owned storage and independent Gmail sync.
import SwiftUI

@main
struct MermozApp: App {
    @State private var model: MailboxModel

    init() {
        let support = URL.applicationSupportDirectory.appending(path: "Mermoz/Mail", directoryHint: .isDirectory)
        _model = State(initialValue: MailboxModel(directory: support))
    }

    var body: some Scene {
        WindowGroup { MailboxView(model: model) }
    }
}
