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

Development and preview packages are ad-hoc signed. For trusted distribution, use a Developer ID
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

CI runs development builds and display-backed native smoke tests. The release
workflow builds and tests extracted distribution archives. Artifacts include ZIP/tar
archives, checksums, provenance manifests, logs, and screenshots. Public release publication,
trusted Windows signing, macOS notarization, and a clean Windows 11 VM distribution
check are explicit release-operator steps; they do not change the product feature
milestones. Preview packages are not installers/updaters.

## GitHub Actions

[CI](../.github/workflows/ci.yml) runs on PRs, pushes to `master`, and manual dispatch.
It is also reusable by the release workflow. It checks workflow syntax, formatting,
Clippy, Rust tests, and native UI behavior on Windows 2025, Ubuntu 24.04 (X11 and
Wayland), and macOS 15 Apple Silicon. CI builds the development macOS app with
`bash scripts/build-macos.sh dev`; the release packager uses the same bundle builder
in release mode. Smoke evidence is retained for 14 days.

[Release builds](../.github/workflows/release.yml) support:

- A pushed `v<Cargo version>` tag: validate all application versions, run reusable CI,
  build and smoke-test all three extracted archives, verify asset checksums, and
  create or refresh a **draft** GitHub release. Reruns refuse to replace published
  release assets. The tag must already exist and still reference the built commit.
- Manual dispatch: run the same checks and packaging, retaining download artifacts
  for 30 days. A branch run produces preview artifacts; dispatching an existing
  version tag also stages its draft release.
- PRs changing workflows, native packaging, or smoke scripts: build/test previews
  alongside normal CI, without creating a tag or GitHub release.

The root and GTK `Cargo.toml` versions and both macOS bundle version fields must
match. Update them together before tagging. For example, after merging the workflows:

```sh
gh workflow run release.yml --ref master
# When releasing version 0.1.0 from the intended, reviewed commit:
git tag v0.1.0 <commit>
git push origin v0.1.0
```

Release archives are named `console-ui-<version>-windows-x64.zip`,
`console-ui-<version>-linux-x86_64.tar.gz`, and
`console-ui-<version>-macos-arm64.zip`. Each has a `.sha256` sidecar and a JSON
manifest with schema version, application version, platform, source commit, archive
name, and SHA-256 digest. Only the draft-release job receives `contents: write`;
build and verification jobs use read access. Production releases require the release
operator's signing/notarization and clean-machine checks before publication. Preview
releases must identify unsigned or ad-hoc-signed builds and any pending platform checks
in their release notes. If signing changes an archive, regenerate its checksum and
manifest before uploading it.
