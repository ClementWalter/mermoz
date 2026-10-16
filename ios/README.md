# Mermoz for iPhone

A standalone, local-first iPhone app. The SwiftUI interface calls the existing Rust
mail store in-process through `mermoz-mobile`; no Mac, daemon, socket, server or
network connection is needed for the demo mailbox.

This first milestone includes Maya Laurent's fictional mailbox, conversation
reading, workflow status filters, local full-text search and persistent status
changes. The store lives in the app's Application Support directory. Gmail sign-in,
sync, reply/compose, attachment viewing, notifications, AI and the card-based Feed
are future milestones. The desktop mailbox is independent of this store; this
prototype does not synchronize Mermoz workflow metadata between devices.

## Build and run

Requires Apple Silicon, Xcode with an iOS SDK and simulator runtime, Rust, uv and
XcodeGen. The deployment target is iOS 18. The project is generated from
`ios/project.yml`, and generated projects and build products are ignored by Git.

From the repository root:

```console
uv run scripts/build_app.py --platform ios-simulator --test --open
```

The builder installs the Rust target, compiles the embedded library in release
mode, generates the Xcode project, and runs unit integration tests followed by UI
tests in Release configuration. It selects a booted iPhone simulator or boots an
available one. `--simulator UDID` selects a specific available iPhone. `--open`
installs and launches the app, and opens Simulator when its UI is installed.

The simulator app is at
`build/ios/Build/Products/Release-iphonesimulator/MermozMobile.app`.

An unsigned device build uses:

```console
uv run scripts/build_app.py --platform ios-device
```

Installing on a physical phone or distributing through TestFlight additionally
requires Apple signing and provisioning; the device build does not install or
publish anything. In Xcode, select a signing team and enable code signing for the
app target before a device run.

## Engine boundary

`ios/Bridge/MermozMobile.h` defines handle and string ownership. The Swift actor
serializes calls off the main actor and frees each response. The Rust boundary
returns errors as JSON, contains unwinding panics, and exposes only local reads,
search and status updates. An explicit sandbox path keeps mobile data independent
of the desktop application's default paths. Existing Rust tests cover the store;
mobile-specific tests cover persistence, invalid requests and ABI ownership.
