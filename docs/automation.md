# Performance assertions and baselines

`console-ui check --rules examples/budgets.json --input current.tcui` evaluates a
selected capture session. Omit `--input` for a live observation; use `--observe 10s`
and `--timeout 15s`. `--session` and `--baseline-session` are zero-based. `--policy`
loads the same project lint policy used by native workspaces, before explicit CLI
flags. Rules and results have `schema_version: 1`.

Rules select tasks with the shared query language. Supported metrics are `busy_ns`,
`scheduled_ns`, `polls`, `poll_p99_ns`, `scheduled_p99_ns`, and `warning_tasks`.
Timing/poll metrics use the maximum of matching tasks in the observation interval;
`warning_tasks` counts matching tasks with enabled warnings at the cursor. Lints
remain task-lifetime heuristics. Result rows include the query, units, maximum,
measured value, matched task count, and status. Reports include observation seconds.

Missing tasks, dropped events, counter resets, missing baseline histograms, zero
histogram samples, or outlier-limited percentiles cannot pass a rule. Live percentile
checks warm only matching task streams; more than 32 required streams may exhaust
the timeout. A recording's existing histogram policy determines offline coverage.
Use `capture --histogram-query 'name:worker-0'` to seed matching histogram baselines
before the observation starts. Ctrl+C releases subscriptions and flushes active
recordings. The CLI never overwrites an existing capture.

| Exit | Meaning |
| --- | --- |
| 0 | All configured assertions passed |
| 1 | An assertion/regression exceeded its limit |
| 2 | Invalid command/configuration/rules |
| 3 | Missing, ambiguous, incomplete, or unavailable telemetry |

`unavailable_exit_code` may be 1 or 3 (default 3), allowing CI to classify missing
evidence as a failing budget without treating it as a pass. The per-rule status
still says `unavailable`. Human summaries use stderr; stdout or `--output` contains
machine-readable JSON.

With `--baseline baseline.tcui`, the same matcher and thresholds as the GUI compare
interval counters normalized by duration. `comparison_query` (for example `entity:task name:worker-0`) limits comparison to
reliable identities; unnamed runtime-worker duplicates are deliberately ambiguous.
Matching uses task name/target/source/kind and resource kind/type/target/source;
operations include task/resource identity and operation source. Numeric IDs are
session-local and never used to match different captures. Resource busy/poll rates
aggregate their operations. The `comparison_policy` controls relative percentage,
absolute timing/latency increase in ns (ns/s for normalized timing), polling-rate
increase, and whether added/removed entities count as regressions.

## GitHub Actions and promotion

The manually dispatched [Performance checks](../.github/workflows/performance.yml)
workflow starts the included target, records a bounded observation, downloads a
selected prior run's baseline artifact when requested, evaluates budgets, and
retains captures/results for 30 days. Baseline artifacts include schema metadata,
source commit, platform, capture filename, and a SHA-256 checksum. Comparisons never
replace the baseline. Set **promote** explicitly to publish a new versioned baseline
artifact after the checks pass. Use its workflow run ID in a later comparison.
Changing the workload, policy, instrumentation version, or runner platform should
be an explicit new baseline version. Keep runner/platform and workload comparable;
normalization does not remove benchmark noise.
