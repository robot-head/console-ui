use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    time::Duration,
};

use console_api::{self as common, async_ops, instrument, resources, tasks};
use hdrhistogram::{Histogram, serialization::Deserializer};
use prost_types::{Duration as ProtoDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::config::{Lint, LintThresholds};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Running,
    Scheduled,
    Idle,
    Completed,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Timing {
    pub total: u64,
    pub busy: u64,
    pub scheduled: u64,
    pub idle: u64,
    pub polls: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRow {
    pub id: u64,
    pub name: String,
    pub target: String,
    pub location: String,
    pub fields: String,
    #[serde(default)]
    pub span_fields: BTreeMap<String, String>,
    pub blocking: bool,
    pub state: TaskState,
    pub timing: Timing,
    pub wakes: u64,
    pub self_wakes: u64,
    pub waker_clones: u64,
    pub waker_drops: u64,
    pub last_wake_ago: Option<u64>,
    pub size_bytes: Option<u64>,
    pub original_size_bytes: Option<u64>,
    pub warnings: Vec<Lint>,
}

impl TaskRow {
    pub fn active_wakers(&self) -> u64 {
        self.waker_clones.saturating_sub(self.waker_drops)
    }
    pub fn self_wake_percent(&self) -> f64 {
        percent(self.self_wakes, self.wakes)
    }

    pub fn lint(&mut self, enabled: &BTreeSet<Lint>) {
        self.lint_with(enabled, &LintThresholds::default());
    }
    pub fn lint_with(&mut self, enabled: &BTreeSet<Lint>, thresholds: &LintThresholds) {
        self.warnings = enabled
            .iter()
            .copied()
            .filter(|lint| match lint {
                Lint::SelfWakes => {
                    !self.blocking && self.self_wake_percent() > thresholds.self_wake_percent as f64
                }
                Lint::LostWaker => {
                    !self.blocking
                        && self.state == TaskState::Idle
                        && self.timing.polls > 0
                        && self.active_wakers() == 0
                }
                Lint::NeverYielded => {
                    !self.blocking
                        && self.state == TaskState::Running
                        && self.timing.polls <= 1
                        && self.timing.busy >= thresholds.never_yielded_ns
                }
                Lint::AutoBoxedFuture => self
                    .size_bytes
                    .zip(self.original_size_bytes)
                    .is_some_and(|(size, original)| size != original),
                Lint::LargeFuture => {
                    !self.blocking
                        && self
                            .size_bytes
                            .is_some_and(|size| size >= thresholds.large_future_bytes)
                }
            })
            .collect();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceRow {
    pub id: u64,
    pub kind: String,
    pub concrete_type: String,
    pub target: String,
    pub location: String,
    pub internal: bool,
    pub parent: Option<u64>,
    pub total: u64,
    pub completed: bool,
    pub attributes: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AsyncOpRow {
    pub id: u64,
    pub resource_id: u64,
    pub task_id: Option<u64>,
    pub task_name: String,
    pub source: String,
    pub timing: Timing,
    pub completed: bool,
    pub ready: Option<bool>,
    pub attributes: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistogramBin {
    pub upper_ns: u64,
    pub count: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Distribution {
    pub count: u64,
    pub min: u64,
    pub max: u64,
    /// p10, p25, p50, p75, p90, p95, p99 of samples inside the HDR range.
    pub percentiles: Vec<u64>,
    pub bins: Vec<HistogramBin>,
    pub high_outliers: u64,
    pub highest_outlier: Option<u64>,
    pub error: Option<String>,
    #[serde(skip)]
    pub raw_histogram: Vec<u8>,
}

impl Distribution {
    fn decode(bytes: &[u8], high_outliers: u64, highest_outlier: Option<u64>) -> Self {
        match Deserializer::new().deserialize::<u64, _>(&mut Cursor::new(bytes)) {
            Ok(histogram) => Self::from_histogram(histogram, high_outliers, highest_outlier),
            Err(error) => Self {
                error: Some(format!("Invalid HDR histogram: {error}")),
                ..Self::default()
            },
        }
    }

    fn from_histogram(
        hist: Histogram<u64>,
        high_outliers: u64,
        highest_outlier: Option<u64>,
    ) -> Self {
        use hdrhistogram::serialization::Serializer;
        let mut raw_histogram = Vec::new();
        let _ =
            hdrhistogram::serialization::V2Serializer::new().serialize(&hist, &mut raw_histogram);
        let max = hist.max();
        // Logarithmic buckets keep long latency tails visible without rendering thousands of bars.
        let mut bins = BTreeMap::<u64, u64>::new();
        for value in hist.iter_recorded() {
            let upper = value
                .value_iterated_to()
                .max(1)
                .checked_next_power_of_two()
                .unwrap_or(u64::MAX);
            *bins.entry(upper).or_default() += value.count_since_last_iteration();
        }
        Self {
            count: hist.len(),
            min: hist.min(),
            max,
            percentiles: [0.10, 0.25, 0.50, 0.75, 0.90, 0.95, 0.99]
                .map(|p| hist.value_at_quantile(p))
                .to_vec(),
            bins: bins
                .into_iter()
                .map(|(upper_ns, count)| HistogramBin { upper_ns, count })
                .collect(),
            high_outliers,
            highest_outlier,
            error: None,
            raw_histogram,
        }
    }

    fn from_proto(hist: &tasks::DurationHistogram) -> Self {
        Self::decode(
            &hist.raw_histogram,
            hist.high_outliers,
            hist.highest_outlier,
        )
    }
    pub fn difference(&self, baseline: &Self) -> Self {
        let decode =
            |bytes: &[u8]| Deserializer::new().deserialize::<u64, _>(&mut Cursor::new(bytes));
        let difference = (|| -> anyhow::Result<Self> {
            anyhow::ensure!(
                self.error.is_none() && baseline.error.is_none(),
                "Histogram is incomplete"
            );
            let mut end = decode(&self.raw_histogram)?;
            let start = decode(&baseline.raw_histogram)?;
            end.subtract(&start)?;
            anyhow::ensure!(
                self.high_outliers >= baseline.high_outliers,
                "Histogram counter reset"
            );
            Ok(Self::from_histogram(
                end,
                self.high_outliers - baseline.high_outliers,
                self.highest_outlier
                    .filter(|value| Some(*value) > baseline.highest_outlier),
            ))
        })();
        difference.unwrap_or_else(|e| Self {
            error: Some(format!("Interval histogram unavailable: {e:#}")),
            ..Default::default()
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskDetails {
    pub id: u64,
    #[serde(default)]
    pub updated_at: u64,
    pub poll: Distribution,
    pub scheduled: Distribution,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub now: u64,
    pub updates: u64,
    pub dropped_events: u64,
    pub tasks: Vec<TaskRow>,
    pub resources: Vec<ResourceRow>,
    pub async_ops: Vec<AsyncOpRow>,
    pub details: BTreeMap<u64, TaskDetails>,
}

#[derive(Clone, Default)]
pub struct Accumulator {
    now: u64,
    updates: u64,
    dropped_events: u64,
    metadata: BTreeMap<u64, common::Metadata>,
    tasks: BTreeMap<u64, (tasks::Task, tasks::Stats)>,
    resources: BTreeMap<u64, (resources::Resource, resources::Stats)>,
    ops: BTreeMap<u64, (async_ops::AsyncOp, async_ops::Stats)>,
    readiness: BTreeMap<u64, bool>,
    details: BTreeMap<u64, TaskDetails>,
}

impl Accumulator {
    pub fn now(&self) -> u64 {
        self.now
    }
    pub fn ready_to_record(&self) -> bool {
        self.updates > 0 && self.details.values().all(|d| d.updated_at <= self.now)
    }
    pub fn wire_details(&self) -> Vec<tasks::TaskDetails> {
        let histogram = |d: &Distribution| tasks::DurationHistogram {
            raw_histogram: d.raw_histogram.clone(),
            max_value: d.max,
            high_outliers: d.high_outliers,
            highest_outlier: d.highest_outlier,
        };
        self.details
            .values()
            .map(|d| tasks::TaskDetails {
                task_id: Some(common::Id { id: d.id }),
                now: Some(Timestamp {
                    seconds: (d.updated_at / 1_000_000_000) as i64,
                    nanos: (d.updated_at % 1_000_000_000) as i32,
                }),
                poll_times_histogram: (!d.poll.raw_histogram.is_empty()).then(|| {
                    tasks::task_details::PollTimesHistogram::Histogram(histogram(&d.poll))
                }),
                scheduled_times_histogram: (!d.scheduled.raw_histogram.is_empty())
                    .then(|| histogram(&d.scheduled)),
            })
            .collect()
    }
    pub fn wire_snapshot(&self) -> instrument::Update {
        instrument::Update {
            now: Some(Timestamp {
                seconds: (self.now / 1_000_000_000) as i64,
                nanos: (self.now % 1_000_000_000) as i32,
            }),
            new_metadata: Some(common::RegisterMetadata {
                metadata: self
                    .metadata
                    .iter()
                    .map(|(&id, value)| common::register_metadata::NewMetadata {
                        id: Some(common::MetaId { id }),
                        metadata: Some(value.clone()),
                    })
                    .collect(),
            }),
            task_update: Some(tasks::TaskUpdate {
                new_tasks: self.tasks.values().map(|(t, _)| t.clone()).collect(),
                stats_update: self.tasks.iter().map(|(&id, (_, s))| (id, *s)).collect(),
                dropped_events: 0,
            }),
            resource_update: Some(resources::ResourceUpdate {
                new_resources: self.resources.values().map(|(r, _)| r.clone()).collect(),
                stats_update: self
                    .resources
                    .iter()
                    .map(|(&id, (_, s))| (id, s.clone()))
                    .collect(),
                new_poll_ops: self
                    .readiness
                    .iter()
                    .map(|(&id, &ready)| resources::PollOp {
                        async_op_id: Some(common::Id { id }),
                        is_ready: ready,
                        ..Default::default()
                    })
                    .collect(),
                dropped_events: 0,
            }),
            async_op_update: Some(async_ops::AsyncOpUpdate {
                new_async_ops: self.ops.values().map(|(o, _)| o.clone()).collect(),
                stats_update: self
                    .ops
                    .iter()
                    .map(|(&id, (_, s))| (id, s.clone()))
                    .collect(),
                dropped_events: 0,
            }),
        }
    }
    pub fn apply(&mut self, update: instrument::Update) {
        if let Some(now) = update.now.as_ref().and_then(timestamp) {
            self.now = self.now.max(now);
        }
        self.updates += 1;
        if let Some(metadata) = update.new_metadata {
            for entry in metadata.metadata {
                if let (Some(id), Some(value)) = (entry.id, entry.metadata) {
                    self.metadata.insert(id.id, value);
                }
            }
        }
        if let Some(update) = update.task_update {
            self.dropped_events = self.dropped_events.saturating_add(update.dropped_events);
            for task in update.new_tasks {
                if let Some(id) = task.id {
                    self.tasks.entry(id.id).or_default().0 = task;
                }
            }
            for (id, stats) in update.stats_update {
                self.tasks.entry(id).or_default().1 = stats;
            }
        }
        if let Some(update) = update.resource_update {
            self.dropped_events = self.dropped_events.saturating_add(update.dropped_events);
            for resource in update.new_resources {
                if let Some(id) = resource.id {
                    self.resources.entry(id.id).or_default().0 = resource;
                }
            }
            for (id, stats) in update.stats_update {
                self.resources.entry(id).or_default().1 = stats;
            }
            for op in update.new_poll_ops {
                if let Some(id) = op.async_op_id {
                    self.readiness.insert(id.id, op.is_ready);
                }
            }
        }
        if let Some(update) = update.async_op_update {
            self.dropped_events = self.dropped_events.saturating_add(update.dropped_events);
            for op in update.new_async_ops {
                if let Some(id) = op.id {
                    self.ops.entry(id.id).or_default().0 = op;
                }
            }
            for (id, stats) in update.stats_update {
                self.ops.entry(id).or_default().1 = stats;
            }
        }
    }

    pub fn apply_details(&mut self, details: tasks::TaskDetails) {
        let updated_at = details.now.as_ref().and_then(timestamp).unwrap_or(self.now);
        let Some(id) = details.task_id else {
            return;
        };
        let poll = match details.poll_times_histogram {
            Some(tasks::task_details::PollTimesHistogram::Histogram(hist)) => {
                Distribution::from_proto(&hist)
            }
            Some(tasks::task_details::PollTimesHistogram::LegacyHistogram(bytes)) => {
                Distribution::decode(&bytes, 0, None)
            }
            None => Distribution::default(),
        };
        let scheduled = details
            .scheduled_times_histogram
            .as_ref()
            .map(Distribution::from_proto)
            .unwrap_or_default();
        self.details.insert(
            id.id,
            TaskDetails {
                id: id.id,
                updated_at,
                poll,
                scheduled,
            },
        );
    }

    pub fn prune(&mut self, retention: Option<Duration>) {
        let Some(retention) = retention else {
            return;
        };
        let retain_ns = retention.as_nanos().min(u64::MAX as u128) as u64;
        let now = self.now;
        let keep = |end: &Option<Timestamp>| {
            end.as_ref()
                .and_then(timestamp)
                .is_none_or(|end| now.saturating_sub(end) <= retain_ns)
        };
        self.tasks.retain(|_, (_, stats)| keep(&stats.dropped_at));
        self.resources
            .retain(|_, (_, stats)| keep(&stats.dropped_at));
        self.ops.retain(|_, (_, stats)| keep(&stats.dropped_at));
        self.details.retain(|id, _| self.tasks.contains_key(id));
        self.readiness.retain(|id, _| self.ops.contains_key(id));
        // Metadata may be used by later deltas, so retain it for this connection session.
    }

    fn metadata(&self, id: Option<common::MetaId>) -> Option<&common::Metadata> {
        id.and_then(|id| self.metadata.get(&id.id))
    }

    fn field(
        &self,
        field: &common::Field,
        fallback: Option<&common::Metadata>,
    ) -> (String, String) {
        let metadata = self.metadata(field.metadata_id).or(fallback);
        let name = match &field.name {
            Some(common::field::Name::StrName(name)) => name.clone(),
            Some(common::field::Name::NameIdx(index)) => metadata
                .and_then(|m| m.field_names.get(*index as usize))
                .cloned()
                .unwrap_or_else(|| format!("field[{index}]")),
            None => "?".into(),
        };
        let value = match &field.value {
            Some(common::field::Value::StrVal(v) | common::field::Value::DebugVal(v)) => v.clone(),
            Some(common::field::Value::U64Val(v)) => v.to_string(),
            Some(common::field::Value::I64Val(v)) => v.to_string(),
            Some(common::field::Value::BoolVal(v)) => v.to_string(),
            None => "?".into(),
        };
        (name, value)
    }

    fn attributes(
        &self,
        attributes: &[common::Attribute],
        metadata: Option<&common::Metadata>,
    ) -> String {
        attributes
            .iter()
            .filter_map(|attr| {
                attr.field.as_ref().map(|field| {
                    let (name, value) = self.field(field, metadata);
                    format!("{name}={value}{}", attr.unit.as_deref().unwrap_or_default())
                })
            })
            .collect::<Vec<_>>()
            .join("  ")
    }

    pub fn snapshot(&self, enabled: &BTreeSet<Lint>) -> Snapshot {
        let tasks: Vec<_> = self
            .tasks
            .iter()
            .filter(|(_, (task, _))| task.id.is_some())
            .map(|(&id, (task, stats))| {
                let meta = self.metadata(task.metadata);
                let fields: BTreeMap<_, _> =
                    task.fields.iter().map(|f| self.field(f, meta)).collect();
                let blocking = task.kind == tasks::task::Kind::Blocking as i32
                    || fields
                        .get("kind")
                        .is_some_and(|s| s == "blocking" || s == "block_on");
                let running = stats.poll_stats.as_ref().is_some_and(is_running);
                let scheduled = stats.last_wake.as_ref().and_then(timestamp)
                    > stats
                        .poll_stats
                        .as_ref()
                        .and_then(|p| p.last_poll_started.as_ref())
                        .and_then(timestamp);
                let state = if stats.dropped_at.is_some() {
                    TaskState::Completed
                } else if running {
                    TaskState::Running
                } else if scheduled || stats.poll_stats.as_ref().is_none_or(|p| p.polls == 0) {
                    TaskState::Scheduled
                } else {
                    TaskState::Idle
                };
                let mut row = TaskRow {
                    id,
                    name: fields
                        .get("task.name")
                        .cloned()
                        .unwrap_or_else(|| format!("task {id}")),
                    target: meta.map(|m| m.target.clone()).unwrap_or_default(),
                    location: location(
                        task.location
                            .as_ref()
                            .or_else(|| meta.and_then(|m| m.location.as_ref())),
                    ),
                    fields: fields
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("  "),
                    span_fields: fields.clone(),
                    blocking,
                    state,
                    timing: timing(
                        self.now,
                        &stats.created_at,
                        &stats.dropped_at,
                        stats.poll_stats.as_ref(),
                        stats.scheduled_time.as_ref(),
                        stats.last_wake.as_ref(),
                    ),
                    wakes: stats.wakes,
                    self_wakes: stats.self_wakes,
                    waker_clones: stats.waker_clones,
                    waker_drops: stats.waker_drops,
                    last_wake_ago: stats
                        .last_wake
                        .as_ref()
                        .and_then(timestamp)
                        .map(|w| self.now.saturating_sub(w)),
                    size_bytes: fields.get("size.bytes").and_then(|v| v.parse().ok()),
                    original_size_bytes: fields
                        .get("original_size.bytes")
                        .and_then(|v| v.parse().ok()),
                    warnings: vec![],
                };
                row.lint(enabled);
                row
            })
            .collect();
        let names: BTreeMap<_, _> = tasks.iter().map(|t| (t.id, t.name.as_str())).collect();
        let resources = self
            .resources
            .iter()
            .filter(|(_, (r, _))| r.id.is_some())
            .map(|(&id, (r, stats))| {
                let meta = self.metadata(r.metadata);
                let kind = match r.kind.as_ref().and_then(|k| k.kind.as_ref()) {
                    Some(resources::resource::kind::Kind::Known(_)) => "Timer".into(),
                    Some(resources::resource::kind::Kind::Other(kind)) => kind.clone(),
                    None => "Unknown".into(),
                };
                ResourceRow {
                    id,
                    kind,
                    concrete_type: r.concrete_type.clone(),
                    target: meta.map(|m| m.target.clone()).unwrap_or_default(),
                    location: location(r.location.as_ref()),
                    internal: r.is_internal,
                    parent: r.parent_resource_id.map(|id| id.id),
                    total: lifetime(self.now, &stats.created_at, &stats.dropped_at),
                    completed: stats.dropped_at.is_some(),
                    attributes: self.attributes(&stats.attributes, meta),
                }
            })
            .collect();
        let async_ops = self
            .ops
            .iter()
            .filter(|(_, (op, _))| op.id.is_some())
            .map(|(&id, (op, stats))| {
                let task_id = stats.task_id.map(|id| id.id);
                AsyncOpRow {
                    id,
                    resource_id: op.resource_id.map_or(0, |id| id.id),
                    task_id,
                    task_name: task_id
                        .and_then(|id| names.get(&id))
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    source: op.source.clone(),
                    timing: timing(
                        self.now,
                        &stats.created_at,
                        &stats.dropped_at,
                        stats.poll_stats.as_ref(),
                        None,
                        None,
                    ),
                    completed: stats.dropped_at.is_some(),
                    ready: self.readiness.get(&id).copied(),
                    attributes: self.attributes(&stats.attributes, self.metadata(op.metadata)),
                }
            })
            .collect();
        Snapshot {
            now: self.now,
            updates: self.updates,
            dropped_events: self.dropped_events,
            tasks,
            resources,
            async_ops,
            details: self.details.clone(),
        }
    }
}

fn is_running(poll: &common::PollStats) -> bool {
    poll.last_poll_started.as_ref().and_then(timestamp)
        > poll.last_poll_ended.as_ref().and_then(timestamp)
}

pub fn timestamp(time: &Timestamp) -> Option<u64> {
    if !(0..1_000_000_000).contains(&time.nanos) {
        return None;
    }
    u64::try_from(time.seconds)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(time.nanos as u64)
}

fn duration(duration: Option<&ProtoDuration>) -> u64 {
    duration
        .and_then(|d| {
            if !(0..1_000_000_000).contains(&d.nanos) {
                return None;
            }
            u64::try_from(d.seconds)
                .ok()?
                .checked_mul(1_000_000_000)?
                .checked_add(d.nanos as u64)
        })
        .unwrap_or_default()
}

fn lifetime(now: u64, start: &Option<Timestamp>, end: &Option<Timestamp>) -> u64 {
    start
        .as_ref()
        .and_then(timestamp)
        .map(|start| {
            end.as_ref()
                .and_then(timestamp)
                .unwrap_or(now)
                .saturating_sub(start)
        })
        .unwrap_or_default()
}

fn timing(
    now: u64,
    start: &Option<Timestamp>,
    end: &Option<Timestamp>,
    poll: Option<&common::PollStats>,
    scheduled: Option<&ProtoDuration>,
    wake: Option<&Timestamp>,
) -> Timing {
    let total = lifetime(now, start, end);
    let until = end.as_ref().and_then(timestamp).unwrap_or(now);
    let mut busy = poll.map_or(0, |p| duration(p.busy_time.as_ref()));
    if let Some(p) = poll.filter(|p| is_running(p)) {
        busy = busy.saturating_add(
            until.saturating_sub(
                p.last_poll_started
                    .as_ref()
                    .and_then(timestamp)
                    .unwrap_or(until),
            ),
        );
    }
    busy = busy.min(total);
    let mut scheduled_time = duration(scheduled);
    if poll.is_none_or(|p| !is_running(p)) {
        let last_start = poll
            .and_then(|p| p.last_poll_started.as_ref())
            .and_then(timestamp);
        if let Some(wake) = wake.and_then(timestamp).filter(|w| Some(*w) > last_start) {
            scheduled_time = scheduled_time.saturating_add(until.saturating_sub(wake));
        }
    }
    scheduled_time = scheduled_time.min(total.saturating_sub(busy));
    Timing {
        total,
        busy,
        scheduled: scheduled_time,
        idle: total.saturating_sub(busy).saturating_sub(scheduled_time),
        polls: poll.map_or(0, |p| p.polls),
    }
}

fn location(location: Option<&common::Location>) -> String {
    location
        .map(|l| {
            format!(
                "{}:{}:{}",
                l.file.as_deref().unwrap_or("?"),
                l.line.unwrap_or(0),
                l.column.unwrap_or(0)
            )
        })
        .unwrap_or_else(|| "unavailable".into())
}

pub fn percent(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64 * 100.0
    }
}

pub fn format_ns(ns: u64) -> String {
    match ns {
        0..1_000 => format!("{ns} ns"),
        1_000..1_000_000 => format!("{:.2} us", ns as f64 / 1_000.0),
        1_000_000..1_000_000_000 => format!("{:.2} ms", ns as f64 / 1_000_000.0),
        _ => format!("{:.2} s", ns as f64 / 1_000_000_000.0),
    }
}
