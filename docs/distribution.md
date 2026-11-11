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

Development and preview packages are ad-hoc signed. Signed releases use
`bash scripts/package-macos.sh signed` to apply Developer ID signatures, hardened
runtime, timestamps, notarization, stapling, and Gatekeeper verification before
creating the final ZIP. Credentials are imported into a temporary keychain and
removed on success or failure. See [signing setup](signing.md) for the required
Apple credentials and exact verification steps.

Windows row scrolling uses the system UI Automation ScrollItem API on a separate
COM thread. Requests identify the current component/session/row and only scroll a
still-selected item; queued key repeats coalesce. This supplies explicit scrolling
while Reactor's public element references expose focus without bring-into-view.

## Release checks

CI runs development builds and display-backed native smoke tests. The release
workflow builds and tests extracted distribution archives. Artifacts include ZIP/tar
archives, checksums, source manifests, logs, and screenshots. Signed distributions
also include Sigstore verification bundles. Windows signing and macOS notarization
run automatically once providers are configured. Public publication and a clean
Windows 11 VM distribution check remain release-operator steps. Preview packages
are not installers/updaters.

## GitHub Actions

[CI](../.github/workflows/ci.yml) runs on PRs, pushes to `master`, and manual dispatch.
It is also reusable by the release workflow. It checks workflow syntax, formatting,
Clippy, Rust tests, and native UI behavior on Windows 2025, Ubuntu 24.04 (X11 and
Wayland), and macOS 15 Apple Silicon. CI builds the development macOS app with
`bash scripts/build-macos.sh dev`; the release packager uses the same bundle builder
in release mode. Smoke evidence is retained for 14 days.

[Release builds](../.github/workflows/release.yml) support:

- A pushed `v<Cargo version>` tag: validate versions, run reusable CI, sign and
  smoke-test all three extracted archives, independently verify signatures and
  checksums, and stage a **draft** GitHub release. Missing signing configuration
  fails the build. Reruns refuse to replace published assets. The tag must already
  exist and still reference the built commit.
- Manual dispatch defaults to `distribution=signed`, using `master` or a matching
  version tag. `distribution=preview` explicitly builds unsigned development
  artifacts on a branch, without secrets or OIDC permissions. Tag builds always
  require signing. Artifacts are retained for 30 days.
- PRs changing workflows, native packaging, or smoke scripts: build/test previews
  alongside normal CI, without creating a tag or GitHub release.

The root and GTK `Cargo.toml` versions and both macOS bundle version fields must
match, including the workspace lockfile. Configure the [release-signing environment
and providers](signing.md) first. For example, after merging and updating versions:

```sh
gh workflow run release.yml --ref master -f distribution=signed
# After updating application versions to 0.1.1 in the intended, reviewed commit:
git tag v0.1.1 <commit>
git push origin v0.1.1
```

Release archives are named `console-ui-<version>-windows-x64.zip`,
`console-ui-<version>-linux-x86_64.tar.gz`, and
`console-ui-<version>-macos-arm64.zip`. Each has a `.sha256` sidecar and a JSON
manifest with schema version, application version, platform, source commit, archive
name, SHA-256 digest, distribution mode, and expected signing identity. Signed
manifests include native verification evidence for Windows/macOS. Every archive,
checksum, and manifest has a `.sigstore.json` signature bundle (18 assets total).
Only the draft-release job receives `contents: write`; only signed package jobs
receive release credentials and `id-token: write`. Independent asset verification
uses read access and pins the workflow/ref identity, repository, and source commit.
See [consumer verification commands](signing.md#4-linux-and-archive-authenticity-sigstore).

Published `v0.1.0` remains an unsigned/ad-hoc-signed preview. Enable signed
distribution for a new version rather than replacing published download bytes.
