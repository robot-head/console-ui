# Documents and telemetry semantics

All application documents use UTF-8 JSON with `schema_version: 1`. Unsupported
versions are rejected. Saves use a temporary file in the destination directory,
synchronize it, then replace the destination. Capture files use create-new semantics.

## Preferences and project workspaces

Preferences are stored at the platform configuration location resolved by
`directories::ProjectDirs("org", "ConsoleUI", "ConsoleUI")`:

- Windows: `%APPDATA%\ConsoleUI\ConsoleUI\config\preferences.json`.
- Linux: `$XDG_CONFIG_HOME/consoleui/preferences.json`, normally `~/.config/consoleui/preferences.json`.
- macOS: `~/Library/Application Support/org.ConsoleUI.ConsoleUI/preferences.json`.

The library's `Preferences::path()` is authoritative if platform directory rules
change. Preferences contain settings, profiles, the 20 most recent targets, typed
filter presets, table layouts/sort, editor arguments/root mappings, histogram
policy, and reusable investigation views. Selected numeric histogram IDs are scoped
to their connection and cleared on restart/reconnect; mode, query, and limits persist. Editor arguments are separate argv
entries with `{file}`, `{line}`, and `{column}` substitutions; no shell evaluates
source paths. Longest matching source-root mapping wins. Missing files are reported.

A project policy contains `schema_version`, optional `settings`, and `presets`.
Precedence is application defaults → project settings/presets → explicit session
settings or CLI flags. Imported project policy does not replace global defaults.
Explicit complete Settings documents are complete session overrides. A workspace
contains `schema_version`, `name`, `active`, `tabs`, `policy`, and `views`. A recorded
tab stores its investigation view so cursor/session/filter/columns/selection restore
without opening a live connection. Restore jobs keep missing files isolated to their
tab. A live view intentionally clears saved numeric selection to avoid ID reuse.
A moved capture can be reopened from its new path; colocated sidecar references are
remapped to it. New Workspace provides recovery without deleting the invalid file.

A capture's `<name>.tcui.investigation.json` sidecar stores versioned bookmarks,
notes, named investigation views, and reusable filter presets. It does not modify
the capture. Bookmarks bind session and producer timestamp plus optional task/resource
ID. Views can be updated, renamed, duplicated, restored, and deleted independently.

## Filters

Clauses separated by spaces use AND; `|` joins alternatives with OR; `!` negates a
clause. Quote values containing spaces. Text fields include name, id, state, warning,
target, source/location, kind/type, visibility, fields/attributes, and `field.<key>`.
`entity:task`, `entity:resource`, or `entity:operation` restricts an entity type.
Numeric comparisons use `>`, `>=`, `<`, `<=`, and `=` with duration units for timing
and KiB/MiB for sizes. Examples: `busy>5ms !state:completed`, `polls>=100`,
`field.request.id:123 | warning:self-wakes`. Unsupported fields/units are errors.
A preset stores a query; a saved investigation additionally stores capture context.

## TCUI recording

Eight-byte header `TCUI\x02\0\0\0` (v1 is accepted). Each frame has a u8 kind,
little-endian u32 byte length (maximum 16 MiB), then payload:

| Kind | Payload |
| --- | --- |
| 1 | `console_api.instrument.Update` protobuf |
| 2 | `console_api.tasks.TaskDetails` protobuf |
| 3 | Empty session reset |
| 4 | JSON session metadata: target, histogram policy, coverage notice, pause boundary/time |

IDs and cumulative counters belong to one session. Metadata/detail frames do not
reset counters. A mid-session capture seeds metadata/entities/stats and available
HDR histograms before writing subsequent deltas. Detail timestamps gate availability
at a replay cursor; future samples are never borrowed. Strict reads reject malformed
frames; explicit recovery accepts only a truncated trailing frame, not arbitrary
corruption. Missing histogram streams are unknown, never zero latency.

## Exports and comparisons

Export JSON contains schema version, session, endpoint, optional selected chart
range, a numeric Snapshot, notes, and selected report sections. All timing values
and timestamps are integer nanoseconds; polls/wakes/bucket counts are integers.
HDR details include availability/error, in-range count and percentile values, bins,
and separate high-outlier count/maximum. CSV uses UTF-8 with CRLF records, RFC-style
quoted fields, unit-bearing headers, source identifiers, selected range, dropped
counts, and lifetime/availability indicators. CSV does not flatten HDR distributions;
use JSON for those. Table counters are lifetime values at the cursor even when a
chart/window range is selected. Window rankings and baseline comparisons use counter
deltas, and interval histograms subtract HDR bucket counts exactly.

Busy/scheduled charts sum interval deltas over retained tasks, or show the selected
task. Poll charts divide poll deltas by producer-time seconds. Bounded history may
omit pruned entities; resets, drops, pauses, and missing baselines are incomplete
evidence. Reports are standalone HTML with inline SVG and escaped text, requiring
no server, scripts, fonts, or network. Chart SVGs include axes/units/gap annotations.

Redaction removes selected field keys, identifying source-path fields/locations,
and/or endpoint metadata from exported copies. When formatted attribute strings
cannot be separated reliably, the full string is removed. Capture-policy queries
and notices are cleared on redacted copies to avoid duplicate values in metadata.
Comparison sections are omitted from redacted reports. Arbitrary secrets in a task
name or an unselected field require selecting that field or excluding the section;
redaction does not attempt to infer secrets. Original captures/sidecars stay intact.
