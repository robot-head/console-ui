//! Bounded time-series analysis, diagnostic evidence, contention relationships and comparisons.
use crate::{
    config::{Lint, Settings},
    model::{Snapshot, TaskRow, Timing},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Counters {
    pub total: u64,
    pub busy: u64,
    pub scheduled: u64,
    pub polls: u64,
}
impl From<&Timing> for Counters {
    fn from(t: &Timing) -> Self {
        Self {
            total: t.total,
            busy: t.busy,
            scheduled: t.scheduled,
            polls: t.polls,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub session: u64,
    pub now: u64,
    pub dropped_events: u64,
    pub tasks: BTreeMap<u64, Counters>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrendPoint {
    pub now: u64,
    pub busy_ns: u64,
    pub scheduled_ns: u64,
    pub polls_per_second: f64,
    pub gap: bool,
}
pub struct History {
    samples: VecDeque<Sample>,
    bytes: usize,
    budget: usize,
    duration_ns: u64,
}
impl Default for History {
    fn default() -> Self {
        Self::new(8 * 1024 * 1024, 900_000_000_000)
    }
}
impl History {
    pub fn new(budget: usize, duration_ns: u64) -> Self {
        Self {
            samples: VecDeque::new(),
            bytes: 0,
            budget: budget.max(1024),
            duration_ns,
        }
    }
    pub fn push(&mut self, session: u64, snapshot: &Arc<Snapshot>) {
        if self
            .samples
            .back()
            .is_some_and(|s| s.session == session && s.now == snapshot.now)
        {
            return;
        }
        if self
            .samples
            .back()
            .is_some_and(|s| s.session != session || s.now > snapshot.now)
        {
            self.samples.clear();
            self.bytes = 0;
        }
        let sample = Sample {
            session,
            now: snapshot.now,
            dropped_events: snapshot.dropped_events,
            tasks: snapshot
                .tasks
                .iter()
                .map(|t| (t.id, Counters::from(&t.timing)))
                .collect(),
        };
        let size = sample.tasks.len() * 80 + 64;
        if size > self.budget {
            self.samples.clear();
            self.bytes = 0;
            return;
        }
        self.bytes += size;
        self.samples.push_back(sample);
        while self.bytes > self.budget
            || self.samples.len() > 4096
            || self
                .samples
                .front()
                .is_some_and(|s| snapshot.now.saturating_sub(s.now) > self.duration_ns)
        {
            if let Some(old) = self.samples.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.tasks.len() * 80 + 64);
            } else {
                break;
            }
        }
    }
    pub fn points(&self) -> Vec<TrendPoint> {
        self.samples
            .iter()
            .zip(self.samples.iter().skip(1))
            .map(|(a, b)| {
                let mut point = TrendPoint {
                    now: b.now,
                    gap: b.dropped_events > a.dropped_events || b.now <= a.now,
                    ..Default::default()
                };
                let mut polls = 0u64;
                for (id, end) in &b.tasks {
                    let zero = Counters::default();
                    if let Some(start) = a
                        .tasks
                        .get(id)
                        .or_else(|| (end.total <= b.now.saturating_sub(a.now)).then_some(&zero))
                    {
                        point.busy_ns = point
                            .busy_ns
                            .saturating_add(end.busy.saturating_sub(start.busy));
                        point.scheduled_ns = point
                            .scheduled_ns
                            .saturating_add(end.scheduled.saturating_sub(start.scheduled));
                        polls = polls.saturating_add(end.polls.saturating_sub(start.polls));
                        point.gap |= end.busy < start.busy
                            || end.polls < start.polls
                            || end.scheduled < start.scheduled;
                    } else {
                        point.gap = true;
                    }
                }
                point.gap |= a.tasks.keys().any(|id| !b.tasks.contains_key(id));
                let seconds = b.now.saturating_sub(a.now) as f64 / 1e9;
                point.polls_per_second = if seconds > 0.0 {
                    polls as f64 / seconds
                } else {
                    0.0
                };
                point
            })
            .collect()
    }
    pub fn window(&self, current: &Snapshot, since: u64) -> WindowSummary {
        let anchor = self
            .samples
            .iter()
            .rev()
            .find(|s| s.now <= since)
            .or_else(|| self.samples.front());
        let Some(anchor) = anchor else {
            return WindowSummary {
                complete: false,
                note: Some("No retained history for this range".into()),
                ..Default::default()
            };
        };
        let mut tasks = Vec::new();
        let duration = current.now.saturating_sub(anchor.now);
        for task in &current.tasks {
            let counters = anchor.tasks.get(&task.id);
            let new = counters.is_none() && task.timing.total <= duration;
            tasks.push(WindowTask {
                id: task.id,
                busy_ns: counters
                    .map(|s| task.timing.busy.saturating_sub(s.busy))
                    .unwrap_or(if new { task.timing.busy } else { 0 }),
                scheduled_ns: counters
                    .map(|s| task.timing.scheduled.saturating_sub(s.scheduled))
                    .unwrap_or(if new { task.timing.scheduled } else { 0 }),
                polls: counters
                    .map(|s| task.timing.polls.saturating_sub(s.polls))
                    .unwrap_or(if new { task.timing.polls } else { 0 }),
                available: counters.map_or(new, |s| {
                    task.timing.busy >= s.busy
                        && task.timing.scheduled >= s.scheduled
                        && task.timing.polls >= s.polls
                }),
            });
        }
        let missing = anchor
            .tasks
            .keys()
            .any(|id| !current.tasks.iter().any(|t| t.id == *id));
        let complete = anchor.now <= since
            && anchor.dropped_events == current.dropped_events
            && tasks.iter().all(|t| t.available)
            && !missing;
        WindowSummary { start:anchor.now, end:current.now, complete, tasks,
            note: (!complete).then(|| "The range contains unavailable history, pruned tasks, dropped events, or counter resets; values cover the available evidence".into()) }
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WindowTask {
    pub id: u64,
    pub busy_ns: u64,
    pub scheduled_ns: u64,
    pub polls: u64,
    pub available: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WindowSummary {
    pub start: u64,
    pub end: u64,
    pub complete: bool,
    pub note: Option<String>,
    pub tasks: Vec<WindowTask>,
}

pub fn interval(baseline: &Snapshot, current: &Snapshot) -> (Snapshot, bool) {
    let mut result = current.clone();
    let span = current.now.saturating_sub(baseline.now);
    let mut incomplete =
        current.now <= baseline.now || current.dropped_events > baseline.dropped_events;
    incomplete |= baseline
        .tasks
        .iter()
        .any(|old| !current.tasks.iter().any(|t| t.id == old.id));
    for task in &mut result.tasks {
        if let Some(old) = baseline.tasks.iter().find(|t| t.id == task.id) {
            incomplete |= task.timing.polls < old.timing.polls
                || task.timing.busy < old.timing.busy
                || task.timing.scheduled < old.timing.scheduled;
            task.timing.total = task.timing.total.saturating_sub(old.timing.total);
            task.timing.busy = task.timing.busy.saturating_sub(old.timing.busy);
            task.timing.scheduled = task.timing.scheduled.saturating_sub(old.timing.scheduled);
            task.timing.idle = task.timing.idle.saturating_sub(old.timing.idle);
            task.timing.polls = task.timing.polls.saturating_sub(old.timing.polls);
            if let Some(details) = result.details.get_mut(&task.id) {
                if let Some(start) = baseline.details.get(&task.id) {
                    details.poll = details.poll.difference(&start.poll);
                    details.scheduled = details.scheduled.difference(&start.scheduled);
                } else {
                    details.poll = crate::model::Distribution {
                        error: Some("No baseline histogram for this interval".into()),
                        ..Default::default()
                    };
                    details.scheduled = crate::model::Distribution {
                        error: Some("No baseline histogram for this interval".into()),
                        ..Default::default()
                    };
                }
            }
        } else {
            incomplete |= task.timing.total > span;
        }
        task.timing.total = task.timing.total.min(span);
    }
    for resource in &mut result.resources {
        if let Some(old) = baseline.resources.iter().find(|r| r.id == resource.id) {
            resource.total = resource.total.saturating_sub(old.total);
        } else {
            incomplete |= resource.total > span;
        }
        resource.total = resource.total.min(span);
    }
    for op in &mut result.async_ops {
        if let Some(old) = baseline.async_ops.iter().find(|o| o.id == op.id) {
            incomplete |= op.timing.busy < old.timing.busy || op.timing.polls < old.timing.polls;
            op.timing.total = op.timing.total.saturating_sub(old.timing.total);
            op.timing.busy = op.timing.busy.saturating_sub(old.timing.busy);
            op.timing.scheduled = op.timing.scheduled.saturating_sub(old.timing.scheduled);
            op.timing.idle = op.timing.idle.saturating_sub(old.timing.idle);
            op.timing.polls = op.timing.polls.saturating_sub(old.timing.polls);
        } else {
            incomplete |= op.timing.total > span;
        }
        op.timing.total = op.timing.total.min(span);
    }
    (result, incomplete)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub task: u64,
    pub lint: Lint,
    pub measured: String,
    pub threshold: String,
    pub explanation: String,
    pub source: String,
}
pub fn evidence(task: &TaskRow, settings: &Settings) -> Vec<Evidence> {
    task.warnings.iter().map(|&lint| {
        let (measured, threshold, explanation) = match lint {
            Lint::SelfWakes => (format!("{:.1}%", task.self_wake_percent()),format!("> {}%",settings.lint_thresholds.self_wake_percent),"Frequent self-wakes can indicate a busy-looping future. Inspect poll and scheduled latency."),
            Lint::LostWaker => (format!("{} active wakers",task.active_wakers()),"0 active wakers on an idle live task".into(),"No recorded active waker can make progress. Missing telemetry can also affect this heuristic."),
            Lint::NeverYielded => (crate::model::format_ns(task.timing.busy),crate::model::format_ns(settings.lint_thresholds.never_yielded_ns),"The first poll has not returned within the configured threshold. Inspect blocking or CPU-bound work."),
            Lint::LargeFuture => (format!("{} bytes",task.size_bytes.unwrap_or(0)),format!("{} bytes",settings.lint_thresholds.large_future_bytes),"The future exceeds the configured stack-size threshold. Inspect its captured state."),
            Lint::AutoBoxedFuture => (format!("{} -> {} bytes",task.original_size_bytes.unwrap_or(0),task.size_bytes.unwrap_or(0)),"Original and driving future sizes differ".into(),"The runtime boxed this future. Inspect its original captured state."),
        };
        Evidence { task:task.id, lint, measured, threshold, explanation:explanation.into(), source:task.location.clone() }
    }).collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphNode {
    pub key: String,
    pub label: String,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub label: String,
    pub operation: Option<u64>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContentionGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub truncated: bool,
}
pub fn contention_graph(snapshot: &Snapshot, focus: Option<&str>, limit: usize) -> ContentionGraph {
    let mut graph = ContentionGraph::default();
    for task in &snapshot.tasks {
        graph.nodes.push(GraphNode {
            key: format!("task:{}", task.id),
            label: task.name.clone(),
            kind: "task".into(),
        });
    }
    for r in &snapshot.resources {
        graph.nodes.push(GraphNode {
            key: format!("resource:{}", r.id),
            label: r.concrete_type.clone(),
            kind: "resource".into(),
        });
        if let Some(parent) = r.parent {
            graph.edges.push(GraphEdge {
                from: format!("resource:{}", r.id),
                to: format!("resource:{parent}"),
                label: "internal component".into(),
                operation: None,
            });
        }
    }
    for op in snapshot.async_ops.iter().filter(|o| !o.completed) {
        if let Some(task) = op.task_id {
            graph.edges.push(GraphEdge {
                from: format!("task:{task}"),
                to: format!("resource:{}", op.resource_id),
                label: format!(
                    "{} / {}",
                    op.source,
                    match op.ready {
                        Some(false) => "pending",
                        Some(true) => "ready operation (ownership unknown)",
                        None => "readiness unknown",
                    }
                ),
                operation: Some(op.id),
            });
        }
    }
    if let Some(focus) = focus {
        graph.edges.retain(|e| e.from == focus || e.to == focus);
        let keys: BTreeSet<_> = graph
            .edges
            .iter()
            .flat_map(|e| [&e.from, &e.to])
            .cloned()
            .chain(std::iter::once(focus.to_owned()))
            .collect();
        graph.nodes.retain(|n| keys.contains(&n.key));
    }
    // Keep connected pairs together. A large task list must not crowd every resource
    // out of the bounded graph and hide all contention evidence.
    let original_len = graph.nodes.len();
    let mut selected = BTreeSet::new();
    if let Some(focus) = focus {
        selected.insert(focus.to_owned());
    }
    for edge in &graph.edges {
        let additional = [&edge.from, &edge.to]
            .iter()
            .filter(|key| !selected.contains(**key))
            .count();
        if selected.len() + additional <= limit {
            selected.insert(edge.from.clone());
            selected.insert(edge.to.clone());
        }
    }
    for node in &graph.nodes {
        if selected.len() >= limit {
            break;
        }
        selected.insert(node.key.clone());
    }
    graph.nodes.retain(|n| selected.contains(&n.key));
    graph.truncated = graph.nodes.len() < original_len;
    let keys: BTreeSet<_> = graph.nodes.iter().map(|n| &n.key).collect();
    graph
        .edges
        .retain(|e| keys.contains(&e.from) && keys.contains(&e.to));
    graph
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RegressionPolicy {
    pub relative_percent: f64,
    pub absolute_ns: u64,
    pub polls_increase: u64,
    pub added_is_regression: bool,
    pub removed_is_regression: bool,
}
impl Default for RegressionPolicy {
    fn default() -> Self {
        Self {
            relative_percent: 10.0,
            absolute_ns: 1_000_000,
            polls_increase: 100,
            added_is_regression: false,
            removed_is_regression: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComparisonRow {
    pub key: String,
    pub kind: String,
    pub name: String,
    pub warnings_added: Vec<String>,
    pub warnings_removed: Vec<String>,
    pub baseline: Option<u64>,
    pub current: Option<u64>,
    pub status: String,
    pub busy_per_second: [f64; 2],
    pub scheduled_per_second: [f64; 2],
    pub polls_per_second: [f64; 2],
    pub poll_p99_ns: [Option<u64>; 2],
    pub scheduled_p99_ns: [Option<u64>; 2],
    pub regression: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comparison {
    pub schema_version: u32,
    pub rows: Vec<ComparisonRow>,
    pub notes: Vec<String>,
}
pub fn compare(
    a: &Snapshot,
    b: &Snapshot,
    seconds: [f64; 2],
    policy: &RegressionPolicy,
) -> Result<Comparison> {
    ensure!(
        seconds.iter().all(|s| s.is_finite() && *s > 0.0),
        "Comparison windows must have positive duration"
    );
    ensure!(
        policy.relative_percent.is_finite() && policy.relative_percent >= 0.0,
        "Regression percentage must be nonnegative"
    );
    struct Entity {
        id: u64,
        kind: &'static str,
        name: String,
        timing: Timing,
        warnings: BTreeSet<String>,
        p99: Option<u64>,
        scheduled_p99: Option<u64>,
    }
    fn task_key(t: &TaskRow) -> String {
        serde_json::to_string(&("task", &t.name, &t.target, &t.location, t.blocking)).unwrap()
    }
    fn resource_key(r: &crate::model::ResourceRow) -> String {
        serde_json::to_string(&(
            "resource",
            &r.kind,
            &r.concrete_type,
            &r.target,
            &r.location,
            r.internal,
        ))
        .unwrap()
    }
    fn percentile(d: &crate::model::Distribution) -> Option<u64> {
        (d.error.is_none() && d.high_outliers == 0 && d.count > 0)
            .then(|| d.percentiles.get(6).copied())
            .flatten()
    }
    fn group(s: &Snapshot) -> BTreeMap<String, Vec<Entity>> {
        let mut entities = BTreeMap::<String, Vec<Entity>>::new();
        for t in &s.tasks {
            let d = s.details.get(&t.id);
            entities.entry(task_key(t)).or_default().push(Entity {
                id: t.id,
                kind: "task",
                name: t.name.clone(),
                timing: t.timing.clone(),
                warnings: t.warnings.iter().map(|w| w.name().to_owned()).collect(),
                p99: d.and_then(|d| percentile(&d.poll)),
                scheduled_p99: d.and_then(|d| percentile(&d.scheduled)),
            });
        }
        for r in &s.resources {
            let mut timing = Timing {
                total: r.total,
                ..Default::default()
            };
            for op in s.async_ops.iter().filter(|op| op.resource_id == r.id) {
                timing.busy = timing.busy.saturating_add(op.timing.busy);
                timing.idle = timing.idle.saturating_add(op.timing.idle);
                timing.polls = timing.polls.saturating_add(op.timing.polls);
            }
            entities.entry(resource_key(r)).or_default().push(Entity {
                id: r.id,
                kind: "resource",
                name: r.concrete_type.clone(),
                timing,
                warnings: BTreeSet::new(),
                p99: None,
                scheduled_p99: None,
            });
        }
        for op in &s.async_ops {
            let resource = s
                .resources
                .iter()
                .find(|r| r.id == op.resource_id)
                .map(resource_key);
            let task = op
                .task_id
                .and_then(|id| s.tasks.iter().find(|t| t.id == id))
                .map(task_key);
            let key = serde_json::to_string(&("operation", resource, task, &op.source)).unwrap();
            entities.entry(key).or_default().push(Entity {
                id: op.id,
                kind: "operation",
                name: format!("{} / {}", op.task_name, op.source),
                timing: op.timing.clone(),
                warnings: BTreeSet::new(),
                p99: None,
                scheduled_p99: None,
            });
        }
        entities
    }
    let left = group(a);
    let right = group(b);
    let keys: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
    let regressed = |values: [f64; 2], absolute: f64| {
        values[1] - values[0] > absolute
            && (values[0] == 0.0 || (values[1] / values[0] - 1.0) * 100.0 > policy.relative_percent)
    };
    let mut rows = vec![];
    for key in keys {
        let aa = left.get(&key);
        let bb = right.get(&key);
        let at = aa.and_then(|v| v.first());
        let bt = bb.and_then(|v| v.first());
        let ambiguous = aa.is_some_and(|v| v.len() > 1) || bb.is_some_and(|v| v.len() > 1);
        let metric = |f: fn(&Timing) -> u64| {
            [
                at.map_or(0.0, |t| f(&t.timing) as f64 / seconds[0]),
                bt.map_or(0.0, |t| f(&t.timing) as f64 / seconds[1]),
            ]
        };
        let busy = metric(|t| t.busy);
        let scheduled = metric(|t| t.scheduled);
        let polls = metric(|t| t.polls);
        let p99 = [at.and_then(|t| t.p99), bt.and_then(|t| t.p99)];
        let scheduled_p99 = [
            at.and_then(|t| t.scheduled_p99),
            bt.and_then(|t| t.scheduled_p99),
        ];
        let quantile_regression = |values: [Option<u64>; 2]| {
            values[0]
                .zip(values[1])
                .is_some_and(|(x, y)| regressed([x as f64, y as f64], policy.absolute_ns as f64))
        };
        let regression = !ambiguous
            && match (at, bt) {
                (None, Some(_)) => policy.added_is_regression,
                (Some(_), None) => policy.removed_is_regression,
                (Some(_), Some(_)) => {
                    regressed(busy, policy.absolute_ns as f64)
                        || regressed(scheduled, policy.absolute_ns as f64)
                        || regressed(polls, policy.polls_increase as f64)
                        || quantile_regression(p99)
                        || quantile_regression(scheduled_p99)
                }
                _ => false,
            };
        let empty = BTreeSet::new();
        let aw = at.map_or(&empty, |t| &t.warnings);
        let bw = bt.map_or(&empty, |t| &t.warnings);
        rows.push(ComparisonRow {
            key,
            kind: bt.or(at).unwrap().kind.into(),
            name: bt.or(at).unwrap().name.clone(),
            baseline: at.map(|t| t.id),
            current: bt.map(|t| t.id),
            status: if ambiguous {
                "ambiguous"
            } else if at.is_none() {
                "added"
            } else if bt.is_none() {
                "removed"
            } else if regression {
                "regression"
            } else {
                "matched"
            }
            .into(),
            busy_per_second: busy,
            scheduled_per_second: scheduled,
            polls_per_second: polls,
            poll_p99_ns: p99,
            scheduled_p99_ns: scheduled_p99,
            regression,
            warnings_added: bw.difference(aw).cloned().collect(),
            warnings_removed: aw.difference(bw).cloned().collect(),
        });
    }
    rows.sort_by_key(|r| (!r.regression, r.kind.clone(), r.name.clone()));
    Ok(Comparison { schema_version:1,rows,notes:vec!["Entities match by source, target, kind and name; resource operations also include their task/resource identity. Duplicate identities are ambiguous, never asserted as regressions.".into(),"Rates use observation-window deltas. Resource busy/poll rates aggregate its operations. Missing or outlier-limited histograms are unknown. Added/removed entities follow the configured policy.".into()] })
}
