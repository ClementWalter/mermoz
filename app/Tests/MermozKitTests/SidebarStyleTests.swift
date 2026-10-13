/// Sidebar icons share one box width whatever their shape; the automated-mail view is
/// called Subscriptions.
import AppKit
import SwiftUI
import Testing
@testable import MermozKit

/// Width SwiftUI lays the slot out at, hosted like a sidebar row's icon.
@MainActor
private func slotWidth<V: View>(_ icon: V) -> CGFloat {
    NSHostingView(rootView: SidebarIconSlot { icon }).fittingSize.width
}

@MainActor @Test func aNarrowDotTakesTheSharedWidth() {
    #expect(slotWidth(Circle().frame(width: 8, height: 8)) == SidebarMetrics.iconWidth)
}

@MainActor @Test func aWideSymbolTakesTheSharedWidth() {
    #expect(slotWidth(Image(systemName: "dot.radiowaves.up.forward").font(.system(size: 11))) == SidebarMetrics.iconWidth)
}

@MainActor @Test func aSquareTakesTheSharedWidth() {
    #expect(slotWidth(RoundedRectangle(cornerRadius: 3).frame(width: 9, height: 9)) == SidebarMetrics.iconWidth)
}

@Test func theViewIsCalledSubscriptions() {
    #expect(Subscriptions.title == "Subscriptions")
}

@Test func theTooltipSaysWhatItHolds() {
    #expect(Subscriptions.tooltip == "Newsletters, receipts and notifications, grouped by sender")
}

@Test func thePaletteStillFindsItByItsOldName() {
    #expect(Subscriptions.keywords.contains("feeds"))
}
