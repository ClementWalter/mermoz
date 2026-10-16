/// End-to-end checks drive the real local mailbox without network access or a Mac service.
import XCTest

final class MailboxUITests: XCTestCase {
    @MainActor func testDemoMailboxOpensAConversation() throws {
        let app = XCUIApplication()
        app.launch()
        let firstThread = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'thread-' ")).firstMatch
        _ = firstThread.waitForExistence(timeout: 30)
        firstThread.tap()
        XCTAssertTrue(app.staticTexts["thread-subject"].waitForExistence(timeout: 10))
    }

    @MainActor func testSearchFindsLocalMail() throws {
        let app = XCUIApplication()
        app.launch()
        let search = app.searchFields.firstMatch
        _ = search.waitForExistence(timeout: 30)
        search.tap()
        search.typeText("Brewline")
        XCTAssertTrue(app.staticTexts["Your parcel from Brewline Coffee arrives tomorrow"].waitForExistence(timeout: 10))
    }

    @MainActor func testStatusSurvivesRelaunch() throws {
        let app = XCUIApplication()
        app.launch()
        let firstThread = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'thread-' ")).firstMatch
        _ = firstThread.waitForExistence(timeout: 30)
        let identifier = firstThread.identifier
        firstThread.tap()
        let status = app.buttons["thread-status"]
        _ = status.waitForExistence(timeout: 10)
        status.tap()
        app.buttons["Archive"].tap()
        // Wait for the local write before simulating a fresh app launch.
        _ = app.buttons.matching(identifier: "thread-status").matching(NSPredicate(format: "label CONTAINS 'Archive'")).firstMatch.waitForExistence(timeout: 10)
        app.terminate()
        app.launch()
        let savedThread = app.buttons[identifier]
        _ = savedThread.waitForExistence(timeout: 30)
        savedThread.tap()
        XCTAssertTrue(app.buttons.matching(identifier: "thread-status").matching(NSPredicate(format: "label CONTAINS 'Archive'")).firstMatch.waitForExistence(timeout: 10))
    }
}
