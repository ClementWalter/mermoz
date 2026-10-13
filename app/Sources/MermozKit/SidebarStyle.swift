/// Sidebar wording and metrics shared by every section, so labels line up and the
/// automated-mail view reads the same everywhere.
import SwiftUI

/// The view of automated senders and mailing lists, one conversation per sender. Its API
/// name stays `feeds`; people see it as Subscriptions.
public enum Subscriptions {
    public static let title = "Subscriptions"
    public static let tooltip = "Newsletters, receipts and notifications, grouped by sender"
    public static let empty = "No newsletters, receipts or notifications waiting"
    /// Words the ⌘K palette also finds it by.
    public static let keywords = ["subscriptions", "newsletters", "receipts", "notifications", "feeds", "lists"]
}

public enum SidebarMetrics {
    /// Every sidebar icon sits in a box this wide, so labels start at one x.
    public static let iconWidth: CGFloat = 16
}

/// A sidebar icon centred in the shared fixed-width box.
public struct SidebarIconSlot<Icon: View>: View {
    let icon: Icon

    public init(@ViewBuilder _ icon: () -> Icon) { self.icon = icon() }

    public var body: some View {
        icon.frame(width: SidebarMetrics.iconWidth, alignment: .center)
    }
}
