# Mermoz for iPhone

A standalone, local-first iPhone app. The SwiftUI interface calls the existing Rust
mail store in-process through `mermoz-mobile`; no Mac, daemon, socket or Mermoz
server is needed. Gmail accounts sync directly with Google while the app is active.

The beta includes native Google consent with PKCE, resumable mailbox import,
incremental sync, conversation reading, workflow status filters, local full-text
search, pagination and replies. Replies are saved locally and require an explicit
send confirmation. Unconfirmed deliveries remain visible in Drafts and cannot be
silently sent again. Maya Laurent's fictional mailbox is available for an offline
trial and is hidden when a real Gmail account connects.

Mail lives in the app's Application Support directory. Refresh credentials live
only in the iPhone Keychain. Foreground sync pauses when the app backgrounds and
resumes on return; this build has no background notifications. New-message compose,
attachment viewing, AI and the card-based Feed are future milestones. The desktop
mailbox is independent of this store; Mermoz workflow metadata does not synchronize
between devices except where represented by Gmail labels.

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

Installing on a physical phone or distributing through TestFlight requires Apple
signing and provisioning; the device build does not install or publish anything.
The `TestFlight` workflow checks Apple access before an optional tested archive and
internal upload. It uses the repository's Apple API-key secrets and produces a
machine-readable distribution status artifact.

The registered bundle is `ai.mermoz.iphone`, and the App Store Connect app is
`6823699933` on team `6878QWQ83Y`. Apple preflight verifies access to that record.
The API key cannot create bundle IDs, so initial registration uses the account
holder's Developer and App Store Connect sessions. Apple's public API cannot
create the App Store Connect app record.

The distribution certificate and `Mermoz iPhone App Store` provisioning profile
support manual App Store signing without registering a physical phone. Keep the
certificate's private key outside Git. The automatic-signing workflow still needs
an archive and upload verification; the local release uses the signed-in Xcode
account and the manual profile.

Xcode can access the local Apple development account, but a direct build targeting
the paired iPhone currently stops because Developer Mode is disabled on that phone.
That route requires enabling Settings > Privacy & Security > Developer Mode on the
device before Xcode can register it and create the development provisioning profile.

The Google iOS client is registered in the existing `mermoz-mail` project and its
public identifier and callback scheme are in `project.yml`. Google's consent
screen currently restricts access to the project's test users.

## Engine boundary

`ios/Bridge/MermozMobile.h` defines handle and string ownership. The Swift actor
serializes calls off the main actor and frees each response. The Rust boundary
returns errors as JSON, contains unwinding panics, and exposes mailbox operations,
Google consent and explicit draft sending. An explicit sandbox path keeps mobile data independent
of the desktop application's default paths. Existing Rust tests cover the store;
mobile-specific tests cover persistence, invalid requests and ABI ownership.
HTTP integration tests exercise authorization-code exchange, native token refresh,
mail import, sending and cancellation against a local Gmail stub. Simulator tests
also exercise real Keychain storage and on-screen navigation, search, status
persistence and reply drafting.
