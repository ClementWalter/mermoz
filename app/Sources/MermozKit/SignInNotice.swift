/// The notice shown across the main window while Google wants the user to sign in to an
/// account again (an expired or removed sign-in): mail for it has stopped arriving.
import Foundation

public struct SignInNotice: Equatable, Sendable {
    public static let button = "Sign in again"
    public static let detail = "Google asked Mermoz to check it's still you. Your mail and your changes stay on this Mac and catch up once you sign in."

    /// The account to sign in first; the others follow from their own notice.
    public var accountId: String
    public var title: String

    /// Nil when every account is signed in. Hidden accounts count too: their mail stops all the same.
    public init?(accounts: [Account]) {
        let stopped = accounts.filter(\.mustSignInAgain)
        guard let first = stopped.first else { return nil }
        accountId = first.id
        title = stopped.count == 1
            ? "Sign in again to keep getting mail for \(first.email)"
            : "Sign in again to keep getting mail for \(first.email) and \(stopped.count - 1) more"
    }
}
