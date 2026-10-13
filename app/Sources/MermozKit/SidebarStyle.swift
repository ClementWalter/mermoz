/// Sidebar metrics shared by every section, so labels line up.
import SwiftUI

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
