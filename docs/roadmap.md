# Product roadmap implementation

The roadmap is implemented in PR [#1](https://github.com/robot-head/console-ui/pull/1).
Issues remain associated with their release milestones until review and merge.
The Windows workbench exposes the complete investigation workflow. GTK4/Relm4 and
SwiftUI/AppKit provide native shells over the same typed Rust controller and shared
formats; their native presentation follows each platform's controls.

| Milestone | Product features | Issues | Implementation |
| --- | --- | --- | --- |
| Faster everyday debugging | Connection profiles/recent targets; advanced filters; named presets; configurable stable tables; keyboard task lookup; mapped editor source links; native recording/file dialogs | [#2](https://github.com/robot-head/console-ui/issues/2)–[#8](https://github.com/robot-head/console-ui/issues/8) | preferences, query, workspace, native workbench |
| Explain bottlenecks | Bounded busy/scheduling/poll trends; recent/custom-duration analysis windows; lint thresholds; warning evidence; task/resource relationships | [#9](https://github.com/robot-head/console-ui/issues/9)–[#13](https://github.com/robot-head/console-ui/issues/13) | analysis, model, history and native charts |
| Investigate recordings | Session browser; indexed background seek; playback/speed; bookmarks/notes; bounded and persisted histogram policies/coverage | [#14](https://github.com/robot-head/console-ui/issues/14)–[#18](https://github.com/robot-head/console-ui/issues/18) | recording, investigation, telemetry, workspace |
| Compare and share findings | Matched/ambiguous task/resource/operation comparison; ranked regressions/improvements; complete saved views; CSV/JSON; SVG/offline reports; redaction preview/copies | [#19](https://github.com/robot-head/console-ui/issues/19)–[#24](https://github.com/robot-head/console-ui/issues/24) | analysis, sharing, workspace |
| Support larger workflows | Independent/reorderable target tabs; portable project workspaces; project filters/lint policy; deterministic CLI budgets; versioned CI baselines and explicit promotion | [#25](https://github.com/robot-head/console-ui/issues/25)–[#29](https://github.com/robot-head/console-ui/issues/29) | workspace, preferences, automation, Performance checks workflow |
| Native Linux and macOS applications | GTK4/Relm4 Linux GUI; SwiftUI/AppKit macOS GUI and typed Rust bridge; native packages and display-backed smoke tests | [#30](https://github.com/robot-head/console-ui/issues/30), [#31](https://github.com/robot-head/console-ui/issues/31) | platforms/linux, platforms/macos, ffi, package/smoke scripts |

## Verification

Core tests exercise the real subscriber/gRPC streams, reconnect/session isolation,
mid-session capture seeding, exact HDR bucket subtraction, history bounds/gaps,
queries/settings/project precedence, seek/truncation/recovery, async tab/cursor
restoration, identity comparison, redacted copies, exported report sections, owned
native bridge strings, and actual CLI exit codes. Native CI validates self-contained
Windows packaging/UI Automation, GTK on X11 and Wayland, and SwiftUI app packaging
and live diagnostic smoke. Package checksums, logs, and screenshots are CI artifacts.
See the PR checks for the current platform validation status.

Public signing/notarization and release publication are documented in
[distribution.md](distribution.md). CLI budgets/baseline promotion and stable result
semantics are documented in [automation.md](automation.md); persistence/export/wire
formats and telemetry availability are documented in [formats.md](formats.md).
