// swift-tools-version: 6.0
// Mermoz macOS app: SwiftUI client of the mermozd daemon.
import PackageDescription

let package = Package(
    name: "Mermoz",
    platforms: [.macOS("26.0")],
    products: [
        .executable(name: "Mermoz", targets: ["Mermoz"]),
    ],
    targets: [
        // Pure logic (protocol models, RPC framing, recipient checks) kept UI-free so it is unit-testable.
        .target(name: "MermozKit"),
        .executableTarget(name: "Mermoz", dependencies: ["MermozKit"]),
        .testTarget(name: "MermozKitTests", dependencies: ["MermozKit"]),
    ]
)
