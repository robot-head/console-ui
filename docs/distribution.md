# Native distributions

## Windows 11

`pwsh scripts/package-windows.ps1` stages a clean x64 self-contained Reactor/WinUI
build, the pinned Windows App Runtime, app-local VC++ DLLs, license notices, and a
SHA-256 checksum. Static CRT linkage covers the Rust executable; WinUI's native
DLL dependencies are app-local. Extract the whole ZIP. An installed Windows App
Runtime, Rust toolchain, or browser runtime is not required. ARM64 source packaging
is available with `-Architecture arm64`; CI validates x64 on windows-2025.

## Linux

`bash scripts/package-linux.sh` emits a tar.gz plus checksum containing native
`console-ui-gtk`, headless `console-ui`, a desktop entry, README, and notices. The
supported package baseline is Ubuntu 24.04 LTS x86_64 with its GTK4/GLib desktop
libraries. Source builds require GTK 4.6+/GLib 2.72+. X11 smoke uses Xvfb; Wayland
smoke uses a headless Weston compositor. Both launch actual GTK widgets, connect to
the instrumented workload, inspect histograms, and capture the rendered window.
Libraries remain system-managed; no GTK DLL/SO replacement is packaged.

## macOS

`bash scripts/package-macos.sh` builds the current architecture's Rust cdylib and
CLI, links the SwiftUI/AppKit/Charts shell, fixes the library's @rpath to the app's
Contents/Frameworks, and produces a zipped app bundle with checksum. Minimum macOS
is 14; CI uses macOS 15 Apple Silicon. To build Intel, use an Intel macOS runner;
combine matching native binaries/libraries with `lipo` before signing if a universal
release is needed. Rust owns telemetry workers and recording files; Swift owns UI
state and C strings returned by the bridge. Calls are serialized through the Rust
controller mutex. The Swift timer runs on MainActor; long file operations run as
Rust background jobs. Closing the final window/quitting destroys workers and
synchronizes active recordings. The bridge catches Rust panics and returns typed
JSON action/view results with explicit string ownership (see include/console_ui.h).

Development packages are ad-hoc signed. For public distribution, use a Developer ID
Application identity to sign the CLI, Rust library, and main executable with hardened
runtime and timestamp, then sign the outer app. Submit the resulting ZIP using
`xcrun notarytool submit --keychain-profile <profile> --wait`, staple the accepted
ticket with `xcrun stapler staple 'Console UI.app'`, and verify using `codesign
--verify --deep --strict` and `spctl --assess --type execute`. Recreate the ZIP and
checksum after stapling. Signing identities/notarization credentials belong in the
release environment; CI does not embed credentials or silently claim notarization.

Windows row scrolling uses the system UI Automation ScrollItem API on a separate
COM thread. Requests identify the current component/session/row and only scroll a
still-selected item; queued key repeats coalesce. This supplies explicit scrolling
while Reactor's public element references expose focus without bring-into-view.

## Release checks

The native CI jobs build packages and run display-backed smoke coverage. Artifacts
include ZIP/tar archives, checksums, logs, and screenshots. Public release publication,
trusted Windows signing, macOS notarization, and a clean Windows 11 VM distribution
check are explicit release-operator steps; they do not change the product feature
milestones. Preview packages are not installers/updaters.
