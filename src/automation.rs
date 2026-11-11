//! Deterministic assertions with stable CLI exit status semantics.
use crate::{analysis::Comparison, model::Snapshot, query::Query};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, clap::Args)]
pub struct CheckOptions {
    #[arg(long)]
    pub rules: PathBuf,
    #[arg(long)]
    pub input: Option<PathBuf>,
    #[arg(long, default_value_t = 0)]
    pub session: usize,
    #[arg(long, default_value = "5s")]
    pub observe: String,
    #[arg(long, default_value = "10s")]
    pub timeout: String,
    #[arg(long)]
    pub baseline: Option<PathBuf>,
    #[arg(long, default_value_t = 0)]
    pub baseline_session: usize,
    #[arg(long)]
    pub output: Option<PathBuf>,
}
#[derive(Debug)]
pub struct CliFailure {
    pub code: i32,
    pub message: String,
}
impl std::fmt::Display for CliFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CliFailure {}
fn failure(code: i32, error: impl std::fmt::Display) -> anyhow::Error {
    CliFailure {
        code,
        message: error.to_string(),
    }
    .into()
}

pub fn capture_window(
    path: &std::path::Path,
    session: usize,
    settings: &crate::config::Settings,
) -> Result<(Snapshot, f64)> {
    capture_range(path, session, settings, None)
}
pub fn capture_range(
    path: &std::path::Path,
    session: usize,
    settings: &crate::config::Settings,
    range: Option<[u64; 2]>,
) -> Result<(Snapshot, f64)> {
    let mut recording = crate::investigation::Recording::open(path)?;
    let selected = recording
        .sessions
        .get(session)
        .ok_or_else(|| anyhow::anyhow!("Capture session is unavailable"))?
        .clone();
    ensure!(
        !selected.incomplete && selected.end > selected.start,
        "Capture needs a complete observation interval"
    );
    let [start, end] = range.unwrap_or([selected.start, selected.end]);
    ensure!(
        start >= selected.start && end <= selected.end && end > start,
        "Comparison range lies outside its session"
    );
    Ok((
        recording.window(session, start, end, settings)?,
        (end - start) as f64 / 1e9,
    ))
}
pub fn check(options: &CheckOptions, settings: &crate::config::Settings) -> Result<i32> {
    let rules: Rules =
        crate::preferences::load_json(&options.rules).map_err(|e| failure(EXIT_CONFIG, e))?;
    rules.validate().map_err(|e| failure(EXIT_CONFIG, e))?;
    let (snapshot, seconds, incomplete) = if let Some(path) = &options.input {
        let (snapshot, seconds) = capture_window(path, options.session, settings)
            .map_err(|e| failure(EXIT_UNAVAILABLE, e))?;
        (snapshot, seconds, false)
    } else {
        let duration =
            humantime::parse_duration(&options.observe).map_err(|e| failure(EXIT_CONFIG, e))?;
        if duration.is_zero() {
            return Err(failure(
                EXIT_CONFIG,
                "Observation duration must be positive",
            ));
        }
        let timeout =
            humantime::parse_duration(&options.timeout).map_err(|e| failure(EXIT_CONFIG, e))?;
        if timeout.is_zero() {
            return Err(failure(EXIT_CONFIG, "Telemetry timeout must be positive"));
        }
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = cancelled.clone();
        std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Runtime::new() {
                runtime.block_on(async {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        signal.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
        });
        let mut live_settings = settings.clone();
        live_settings.retain_for = "none".into();
        let worker = crate::telemetry::Worker::start(live_settings, None, None)
            .map_err(|e| failure(EXIT_UNAVAILABLE, e))?;
        worker.send(crate::telemetry::Command::HistogramPolicy(
            crate::telemetry::HistogramPolicy {
                mode: "matching".into(),
                max_streams: 32,
                ..Default::default()
            },
        ));
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| failure(EXIT_CONFIG, "Telemetry timeout is too large"))?;
        let start = loop {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(failure(EXIT_UNAVAILABLE, "Observation cancelled"));
            }
            let s = worker.read();
            if s.snapshot.updates > 0 {
                let need_histograms = rules.rules.iter().any(|r| r.metric.ends_with("p99_ns"));
                if !need_histograms
                    || s.snapshot
                        .tasks
                        .iter()
                        .all(|t| s.snapshot.details.contains_key(&t.id))
                {
                    break s;
                }
            }
            if Instant::now() >= deadline {
                return Err(failure(
                    EXIT_UNAVAILABLE,
                    s.error
                        .unwrap_or("Timed out waiting for complete telemetry/histograms".into()),
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let end_time = Instant::now()
            .checked_add(duration)
            .ok_or_else(|| failure(EXIT_CONFIG, "Observation duration is too large"))?;
        while Instant::now() < end_time {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(failure(EXIT_UNAVAILABLE, "Observation cancelled"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let end = worker.read();
        let (snapshot, incomplete) = crate::analysis::interval(&start.snapshot, &end.snapshot);
        let seconds = end.snapshot.now.saturating_sub(start.snapshot.now) as f64 / 1e9;
        (
            snapshot,
            seconds,
            incomplete
                || start.session != end.session
                || end.status != crate::telemetry::ConnectionStatus::Connected,
        )
    };
    let mut report = rules.evaluate(&snapshot, incomplete)?;
    if let Some(path) = &options.baseline {
        let (base, base_seconds) = capture_window(path, options.baseline_session, settings)
            .map_err(|e| failure(EXIT_UNAVAILABLE, e))?;
        let comparison = crate::analysis::compare(
            &base,
            &snapshot,
            [base_seconds, seconds],
            &rules.comparison_policy,
        )
        .map_err(|e| failure(EXIT_UNAVAILABLE, e))?;
        report = report.with_comparison(comparison);
    }
    let bytes = serde_json::to_vec_pretty(&report)?;
    if let Some(path) = &options.output {
        crate::preferences::atomic_write(path, &bytes)?;
    } else {
        println!("{}", String::from_utf8(bytes)?);
    }
    Ok(report.exit_code)
}

pub const EXIT_PASS: i32 = 0;
pub const EXIT_ASSERTION: i32 = 1;
pub const EXIT_CONFIG: i32 = 2;
pub const EXIT_UNAVAILABLE: i32 = 3;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    pub query: String,
    pub metric: String,
    pub maximum: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub schema_version: u32,
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub comparison_policy: crate::analysis::RegressionPolicy,
}
impl Rules {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "Unsupported assertion rules version"
        );
        ensure!(!self.rules.is_empty(), "At least one assertion is required");
        ensure!(
            self.comparison_policy.relative_percent.is_finite()
                && self.comparison_policy.relative_percent >= 0.0,
            "Comparison percentage must be finite and nonnegative"
        );
        for r in &self.rules {
            ensure!(!r.name.trim().is_empty(), "Assertion needs a name");
            ensure!(
                r.maximum.is_finite() && r.maximum >= 0.0,
                "Assertion maximum must be finite and nonnegative"
            );
            ensure!(
                [
                    "busy_ns",
                    "scheduled_ns",
                    "polls",
                    "warning_tasks",
                    "poll_p99_ns",
                    "scheduled_p99_ns"
                ]
                .contains(&r.metric.as_str()),
                "Unsupported assertion metric {}",
                r.metric
            );
            Query::parse(&r.query)?;
        }
        Ok(())
    }
    pub fn evaluate(&self, snapshot: &Snapshot, incomplete: bool) -> Result<AssertionReport> {
        self.validate()?;
        let mut results = vec![];
        for rule in &self.rules {
            let query = Query::parse(&rule.query)?;
            let selected: Vec<_> = snapshot.tasks.iter().filter(|t| query.task(t)).collect();
            let mut available = !incomplete && !selected.is_empty();
            let measured = if rule.metric == "warning_tasks" {
                Some(selected.iter().filter(|t| !t.warnings.is_empty()).count() as f64)
            } else {
                let mut values = vec![];
                for t in &selected {
                    let value = match rule.metric.as_str() {
                        "busy_ns" => Some(t.timing.busy),
                        "scheduled_ns" => Some(t.timing.scheduled),
                        "polls" => Some(t.timing.polls),
                        "poll_p99_ns" => snapshot
                            .details
                            .get(&t.id)
                            .filter(|d| {
                                d.poll.count > 0
                                    && d.poll.high_outliers == 0
                                    && d.poll.error.is_none()
                            })
                            .and_then(|d| d.poll.percentiles.get(6).copied()),
                        "scheduled_p99_ns" => snapshot
                            .details
                            .get(&t.id)
                            .filter(|d| {
                                d.scheduled.count > 0
                                    && d.scheduled.high_outliers == 0
                                    && d.scheduled.error.is_none()
                            })
                            .and_then(|d| d.scheduled.percentiles.get(6).copied()),
                        _ => None,
                    };
                    if let Some(v) = value {
                        values.push(v);
                    } else {
                        available = false;
                    }
                }
                values.into_iter().max().map(|n| n as f64)
            };
            results.push(AssertionResult {
                name: rule.name.clone(),
                metric: rule.metric.clone(),
                maximum: rule.maximum,
                measured,
                matched_tasks: selected.len(),
                query: rule.query.clone(),
                unit: if rule.metric.ends_with("_ns") {
                    "ns"
                } else {
                    "count"
                }
                .into(),
                status: if !available {
                    "unavailable"
                } else if measured.is_some_and(|m| m > rule.maximum) {
                    "failed"
                } else {
                    "passed"
                }
                .into(),
            });
        }
        let exit_code = if results.iter().any(|r| r.status == "unavailable") {
            EXIT_UNAVAILABLE
        } else if results.iter().any(|r| r.status == "failed") {
            EXIT_ASSERTION
        } else {
            EXIT_PASS
        };
        Ok(AssertionReport {
            schema_version: 1,
            exit_code,
            results,
            comparison: None,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssertionResult {
    pub name: String,
    pub metric: String,
    pub maximum: f64,
    pub measured: Option<f64>,
    pub matched_tasks: usize,
    pub query: String,
    pub unit: String,
    pub status: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssertionReport {
    pub schema_version: u32,
    pub exit_code: i32,
    pub results: Vec<AssertionResult>,
    pub comparison: Option<Comparison>,
}
impl AssertionReport {
    pub fn with_comparison(mut self, comparison: Comparison) -> Self {
        if comparison.rows.iter().any(|r| r.status == "ambiguous") {
            self.exit_code = EXIT_UNAVAILABLE;
        } else if comparison.rows.iter().any(|r| r.regression) && self.exit_code == EXIT_PASS {
            self.exit_code = EXIT_ASSERTION;
        }
        self.comparison = Some(comparison);
        self
    }
}
