/// The app target loads in-process: a model can be made without a daemon.
import Testing
@testable import Mermoz

@MainActor @Test func aModelStartsWithNoThreads() {
    #expect(AppModel().threads.isEmpty)
}
