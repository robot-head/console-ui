# Console UI

Native Tokio diagnostics for Windows 11 (Reactor/WinUI 3), Linux
([GTK4/Relm4](https://relm4.org/)), and macOS (SwiftUI/AppKit).
All three shells use the same Rust telemetry, analysis, recording, and workspace
controller. The CLI also works without a desktop. The application connects directly
to `console-subscriber` through `console-api` 0.9.

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

## Run on Linux

Supported distribution baseline: Ubuntu 24.04 LTS, x86_64, on Wayland or X11.
Source builds require GTK 4.6+, GLib/GIO 2.72+, a C compiler, pkg-config, and Rust
1.95+. The published tar package uses the build distribution's system GTK libraries;
it does not bundle or replace the desktop runtime.

```sh
sudo apt-get install libgtk-4-dev build-essential pkg-config
cargo run --locked -p console-ui-gtk
bash scripts/package-linux.sh
```

Extract `dist/console-ui-linux-x86_64.tar.gz`, put `bin` on PATH, and launch
`console-ui-gtk`. The archive also includes the headless `console-ui` command and a
desktop file under `share/applications`. Connect using the endpoint field or pass
an endpoint/`--config`, `--policy`, `--record`, or `--replay` at launch.
Ctrl+K finds tasks, Ctrl+F focuses the filter, and F5 reconnects. Lists display at
most 1,000 matching entities; narrow the query for larger runtimes.

## Run on macOS

The native app requires macOS 14+. Build on macOS with Xcode command-line tools and
Rust 1.95+. The package uses the build machine's architecture (Apple Silicon or
Intel); macOS CI validates Apple Silicon on macOS 15.

```sh
bash scripts/package-macos.sh
open 'dist/Console UI.app'
```

The app bundle contains the SwiftUI/AppKit shell, Rust library, and headless command
at `Contents/MacOS/console-ui`. Cmd+K opens task lookup, Cmd+O opens a capture, and
Shift+Cmd+J opens the selected source. Native tables, menus, panels, and macOS
appearance/accessibility services are used. See [distribution](docs/distribution.md)
for signing and notarization before public distribution.

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
  save application preferences in the per-user configuration directory. Native
  workspaces add project policies, saved queries, target tabs, and investigation views.

| Key | Action |
| --- | --- |
| Ctrl+K | Find a task by name or ID |
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

The application uses per-user preferences, then `console.toml` from the current
directory or an explicit `--config path`. A `--policy project.json` applies the same
versioned project lint policy as the GUI. Explicit CLI flags override policy values. `--allow` takes
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

Use the native Recording panel to start/stop capture, choose a file, browse sessions,
seek a timeline position, play at 0.1–16× speed, and bookmark or annotate incidents.
Replay state and available histograms never cross connection sessions. Opened files
load and seek in background jobs; switching tabs keeps each investigation independent.
Saved views preserve filters, columns, cursor, range, and selection. Workspace files
also preserve the active tab and recording position across restarts.

For a repeatable headless observation:

```sh
console-ui capture current.tcui --observe 10s --histogram-query 'name:worker-0' --max-streams 4
console-ui sessions current.tcui
console-ui compare baseline.tcui current.tcui --output comparison.json
console-ui check --rules examples/budgets.json --input current.tcui --baseline baseline.tcui --output results.json
console-ui export current.tcui report.html --format html --query 'name:worker-0'
```

Capture warms the selected histogram streams before seeding the recording so
interval percentile checks can subtract baseline counts. `check` can also observe a
live server with `--observe` and `--timeout`; Ctrl+C cancels and releases connections.
Exit codes are **0 pass, 1 assertion failure, 2 invalid configuration, 3 unavailable
or incomplete evidence**. Human summaries go to stderr and JSON results to stdout
or `--output`. See [automation and baseline promotion](docs/automation.md).

The native workbench adds typed AND/OR/negated filters (`busy>5ms !state:completed |
warning:self-wakes`), reusable global/project presets, configurable columns,
source-root mappings, and per-lint thresholds. Analysis charts use interval busy and
scheduled nanoseconds and polls/second for either the selected task or the sum of
retained tasks. Select a task or a ranked window row to inspect its source and
warnings. The recent-window duration and recording cursor define the analysis range;
lifetime-only inspector/table values remain labeled as lifetime values.

History is bounded to 8 MiB, 15 minutes, and 4,096 samples per tab. Charts split at
observed gaps, dropped events, pauses, counter resets, and session changes. Unknown
ownership in resource relationships is labeled explicitly. Histogram policies select
inspected tasks, an explicit ID set, or matching tasks with 1–32 simultaneous streams
and bounded rotation. The inspected task takes priority. Policy and coverage notices
are saved in capture metadata; missing histograms remain unknown.

CSV/JSON exports retain numeric values and scope/availability context. Self-contained
HTML reports include selectable chart/task/resource/comparison sections; SVG charts
include axes, units, and gaps. Preview and choose field keys, source paths, and
endpoint details to redact. Exports and redacted capture copies preserve the original;
these actions do not upload data. Selected chart ranges do not turn lifetime table
counters into interval values. See [document formats](docs/formats.md).

TCUI v2 recordings contain incremental protobuf updates, histogram frames, session
metadata, and reset markers. The reader also accepts TCUI v1. Existing captures are
never overwritten. Strict opening rejects corrupt/truncated frames, unknown versions,
and frames above 16 MiB. The explicit recovery action retains complete frames from a
truncated tail and marks the session incomplete. A shared index supports up to five
million frames; four replay checkpoints use a 32 MiB estimated budget (large states
above the per-entry budget are rebuilt in the background). `.tcui` is distinct from
the subscriber's raw tracing-event JSON format.

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

A Windows UI type check from Linux is
`cargo check --target x86_64-pc-windows-msvc --no-default-features --all-targets`
(no runtime staging or TLS C compilation). Native linking/staging requires Windows.
CI builds and tests the shared core, self-contained Windows package, GTK native app
on X11 and Wayland, and SwiftUI app bundle on macOS. Native smoke tests inspect live
tasks/resources and histograms, exercise Pause/Resume, and save screenshots.

Product feature tracking is in [the roadmap](docs/roadmap.md). CI artifacts include
packages and checksums; public release signing is a separate distribution step.
