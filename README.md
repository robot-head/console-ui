# Console UI

A native Windows 11 Tokio diagnostics application built with Microsoft's
[windows-reactor](https://github.com/microsoft/windows-rs/tree/master/crates/libs/reactor)
and WinUI 3. It connects directly to the real `console-subscriber` gRPC service using
`console-api` 0.9. No web server, WebView, or browser UI is used.

The Reactor preview and its runtime setup helper are pinned to windows-rs commit
`0cfb708054687e5bc32c596a98cfcf1395344eab`. Keep both on the same revision.

## Run on Windows 11

Install Rust 1.95 or newer and Visual Studio 2022/2026 C++ build tools, including
the Windows SDK and MSVC x64 tools. Windows supplies `curl.exe` and `tar.exe`;
the first self-contained build downloads the matching Windows App Runtime from NuGet.

```powershell
cargo run --locked
```

In another terminal, run the included instrumented workload:

```powershell
cargo run --locked --example instrumented
```

The default endpoint is `http://127.0.0.1:6669`. The example exposes named tasks,
self-waking activity, timers, a semaphore, and mutex contention. Connect in the
window, or provide another endpoint:

```powershell
cargo run --locked -- http://192.168.0.42:9090 --retain-for 5min --palette dark
```

## Inspect runtime behavior

* **Tasks:** live Running, Scheduled, Idle, and Completed counts; clickable numeric
  sorting; text filtering; warnings-only filtering; names, targets, spawn locations,
  timing, and poll counts. Select a row and choose **Inspect selected**.
* **Task details:** total/busy/scheduled/idle durations and percentages, active
  wakers, clone/drop counts, last wake, self-wake ratio, future sizes, source fields,
  p10/p25/p50/p75/p90/p95/p99, and logarithmic poll/scheduling histograms. Both legacy
  and current HDR wire encodings are supported. High outliers are reported separately.
* **Resources:** lifetime, kind, type, target, PUB/INT visibility, source, and
  attributes. Inspect a resource to sort its async operations by timing, polls,
  task, source, readiness, and attributes; open the task behind a selected operation.
* **Connection:** automatic exponential reconnect (1–10 seconds), visible errors,
  dropped-event notices, and server Pause/Resume RPCs. A successful new connection
  starts a fresh session, preventing task-ID collisions with the previous process.
  Historical data remains visible while disconnected. Pausing affects the server
  and can affect other observers connected to that subscriber.
* **Settings:** toggle the five lints, select the system/light/dark palette, and
  save the effective settings to `console.toml` in the current directory.

| Key | Action |
| --- | --- |
| `t`, `r` | Tasks, resources |
| `j`/`k`, Up/Down | Move selection |
| `gg`/`G`, Home/End | First/last row |
| `h`/`l`, Left/Right | Sort column |
| `i` | Reverse sort |
| Enter | Inspect selection |
| Esc | Return to parent |
| Space | Pause/resume |
| `q` | Close window |

Shortcuts are suspended while editing text. Native controls support Tab navigation
and system accessibility/theme resources. Span fields and detailed source locations
can be selected and copied. `--ascii-only` replaces the warning symbol;
`--no-colors` uses neutral chart bars.

## Instrument your own application

```toml
[dependencies]
tokio = { version = "1", features = ["full", "tracing"] }
console-subscriber = "0.5"
```

Enable unstable Tokio instrumentation in the **target application's** workspace:

```toml
# .cargo/config.toml
[build]
rustflags = ["--cfg", "tokio_unstable"]
```

```rust
fn main() {
    console_subscriber::init();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all().build().unwrap();
    rt.block_on(async { /* application */ });
}
```

For an existing `tracing-subscriber` registry, attach
`console_subscriber::spawn()` as a layer instead of calling `init()`. The included
[example](examples/instrumented.rs) uses the composable builder. Set
`TOKIO_CONSOLE_BIND=127.0.0.1:7777` for a custom server address,
`TOKIO_CONSOLE_RETENTION=1h` for server retention, and
`TOKIO_CONSOLE_RECORD_PATH=raw-events.json` for the subscriber's raw tracing flight
recorder. Server retention is independent of this client's `--retain-for` setting.
Keep Tokio/runtime tracing targets enabled when composing logging filters.

## Configuration and headless commands

The application loads `console.toml` from the current directory when present, or
an explicit `--config path`. CLI flags override file settings. `--allow` takes
precedence over `--warn`. Durations accept `60s`, `5min`, or `none` (indefinite).

```powershell
console-ui.exe --retain-for 5min gen-config > console.toml
console-ui.exe --config console.toml -A self-wakes,large-future
console-ui.exe gen-completion powershell > console-ui.ps1
console-ui.exe http://127.0.0.1:6669 dump --timeout 10s --output snapshot.json
```

Lints: `self-wakes` (>50%), `lost-waker` (live idle task with no active wakers),
`never-yielded` (first poll running for at least 1 second), `large-future`
(at least 1,024 bytes), and `auto-boxed-future` (different original/boxed sizes).
Task timing uses the producer's timestamp, including in-progress poll/scheduling
time; it does not mix the client clock with a remote runtime's clock.

HTTP and HTTPS TCP endpoints are supported. HTTPS uses the operating system's
trusted roots. Unix-domain sockets, vsock, and terminal-specific locale options
are not supported by this Windows GUI. Terminal color flags are adapted as
described above. The library can also consume compatible producers from other
runtimes implementing console-api's Instrument service, including WatchUpdates,
WatchTaskDetails, WatchState, Pause, and Resume.

## Record and inspect offline

```powershell
console-ui.exe --record capture.tcui
console-ui.exe --replay capture.tcui
console-ui.exe --replay capture.tcui --retain-for none dump --output offline.json
```

Recordings store original incremental protobuf messages and reset markers between
connection sessions. Task histograms are captured while their detail stream is
open. Replay reconstructs the **final retained snapshot of the last session**;
it is not a time-scrubbing player. Use `--retain-for none` during replay to inspect
all completed objects captured in that session. Existing recordings are never
overwritten. Truncated frames, unknown versions, and frames exceeding 16 MiB are
rejected. `.tcui` is distinct from `console-subscriber`'s raw tracing-event format;
this application does not parse those raw JSON recordings.

## Build a self-contained distribution

Self-contained deployment is **enabled by default**. `build.rs` calls
`windows_reactor_setup::as_self_contained()` for Windows builds. That helper
embeds the deployment manifest and stages the matching Windows App Runtime DLLs,
PRI files, and resource directories beside the executable. The app's own MSVC
runtime is statically linked by `.cargo/config.toml`.

From a Visual Studio Developer PowerShell terminal:

```powershell
./scripts/package-windows.ps1
```

This builds from a dedicated clean output, copies the complete staged runtime,
adds app-local Visual C++ runtime DLLs used by WinUI, verifies required payload
files, and emits:

```text
dist/console-ui-win-x64.zip
dist/console-ui-win-x64.zip.sha256
```

Extract the **whole archive** and launch `console-ui.exe` on Windows 11. No Rust
toolchain or separately installed Windows App Runtime is needed on the destination.
The `.exe` alone is insufficient. There is no WebView in this app; the projection
DLL staged by Reactor does not require installing a browser runtime for this UI.

ARM64 packaging is available with `-Architecture arm64` after installing the
MSVC ARM64 tools, ARM64 CRT redistributables, and the Rust `aarch64-pc-windows-msvc`
target. The CI package targets x64.

For a framework-dependent developer build using an installed matching Windows
App Runtime, use `cargo build --no-default-features --features tls`. The `tls`
feature is enabled by default and must be present for HTTPS connections.

## Validation

```text
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

Core tests cover wire deltas, timing, lints, HDR decoding, resource/task relations,
retention, configuration, recording corruption, and an actual in-process gRPC
producer exercising details, Pause/Resume, stream failure, and reconnect isolation.
The included instrumented workload exercises the real console-subscriber service.

Linux can run the headless commands and core tests. A Windows UI type check from
Linux is `cargo check --target x86_64-pc-windows-msvc --no-default-features --all-targets`
(no runtime staging or TLS C compilation). Native linking, runtime staging, and
interactive WinUI execution require Windows tooling. The GitHub Actions workflow
builds/tests on Windows, packages the app, and runs UI Automation startup/navigation
checks with screenshot artifacts. Also test the extracted archive on a clean
Windows 11 VM without a preinstalled App Runtime before distribution.
