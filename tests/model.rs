use console_api::{self as api, async_ops, instrument, resources, tasks};
use console_ui::{
    config::{Lint, Settings},
    model::{Accumulator, TaskState},
};
use hdrhistogram::serialization::{Serializer, V2Serializer};
use prost_types::Timestamp;
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

fn ts(seconds: i64) -> Option<Timestamp> {
    Some(Timestamp { seconds, nanos: 0 })
}
fn stats() -> tasks::Stats {
    tasks::Stats {
        created_at: ts(10),
        wakes: 10,
        self_wakes: 6,
        waker_clones: 3,
        poll_stats: Some(api::PollStats {
            polls: 2,
            last_poll_started: ts(12),
            last_poll_ended: ts(13),
            busy_time: Some(prost_types::Duration {
                seconds: 2,
                nanos: 0,
            }),
            ..Default::default()
        }),
        scheduled_time: Some(prost_types::Duration {
            seconds: 1,
            nanos: 0,
        }),
        ..Default::default()
    }
}
fn update(stats: tasks::Stats) -> instrument::Update {
    instrument::Update {
        now: ts(20),
        task_update: Some(tasks::TaskUpdate {
            new_tasks: vec![tasks::Task {
                id: Some(api::Id { id: 1 }),
                ..Default::default()
            }],
            stats_update: HashMap::from([(1, stats)]),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn snapshot(acc: &Accumulator) -> console_ui::model::Snapshot {
    acc.snapshot(&Settings::default().warnings)
}

#[test]
fn merges_incremental_stats_without_losing_task_metadata() {
    let mut acc = Accumulator::default();
    let mut initial = update(stats());
    initial.new_metadata = Some(api::RegisterMetadata {
        metadata: vec![api::register_metadata::NewMetadata {
            id: Some(api::MetaId { id: 42 }),
            metadata: Some(api::Metadata {
                target: "tokio::task".into(),
                field_names: vec!["task.name".into()],
                ..Default::default()
            }),
        }],
    });
    let task = &mut initial.task_update.as_mut().unwrap().new_tasks[0];
    task.metadata = Some(api::MetaId { id: 42 });
    task.fields.push(api::Field {
        name: Some(api::field::Name::NameIdx(0)),
        value: Some(api::field::Value::StrVal("worker".into())),
        ..Default::default()
    });
    acc.apply(initial);
    let mut next = stats();
    next.poll_stats.as_mut().unwrap().polls = 50;
    acc.apply(instrument::Update {
        now: ts(21),
        task_update: Some(tasks::TaskUpdate {
            stats_update: HashMap::from([(1, next)]),
            dropped_events: 7,
            ..Default::default()
        }),
        ..Default::default()
    });
    let snapshot = snapshot(&acc);
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.tasks[0].name, "worker");
    assert_eq!(snapshot.tasks[0].target, "tokio::task");
    assert_eq!(snapshot.tasks[0].timing.polls, 50);
    assert_eq!(snapshot.dropped_events, 7);
    assert_eq!(snapshot.updates, 2);
}

#[test]
fn timing_accounts_for_in_progress_polls_and_schedule_queue() {
    let mut acc = Accumulator::default();
    let mut running = stats();
    running.poll_stats.as_mut().unwrap().last_poll_started = ts(18);
    acc.apply(update(running));
    let task = &snapshot(&acc).tasks[0];
    assert_eq!(task.state, TaskState::Running);
    assert_eq!(task.timing.busy, 4_000_000_000);
    assert_eq!(task.timing.idle, 5_000_000_000);

    let mut scheduled = stats();
    scheduled.last_wake = ts(17);
    acc.apply(update(scheduled));
    let task = &snapshot(&acc).tasks[0];
    assert_eq!(task.state, TaskState::Scheduled);
    assert_eq!(task.timing.scheduled, 4_000_000_000);
    assert_eq!(task.timing.idle, 4_000_000_000);
}

#[test]
fn completed_lifetimes_stop_and_timing_never_underflows() {
    let mut acc = Accumulator::default();
    let mut completed = stats();
    completed.dropped_at = ts(15);
    completed.poll_stats.as_mut().unwrap().busy_time = Some(prost_types::Duration {
        seconds: 20,
        nanos: 0,
    });
    acc.apply(update(completed));
    let task = &snapshot(&acc).tasks[0];
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(task.timing.total, 5_000_000_000);
    assert_eq!(
        task.timing.busy + task.timing.scheduled + task.timing.idle,
        task.timing.total
    );
}

#[test]
fn lints_respect_thresholds_and_exclude_blocking_and_completed_lost_wakers() {
    let mut acc = Accumulator::default();
    let mut lost = stats();
    lost.waker_drops = 3;
    acc.apply(update(lost));
    assert!(snapshot(&acc).tasks[0].warnings.contains(&Lint::SelfWakes));
    assert!(snapshot(&acc).tasks[0].warnings.contains(&Lint::LostWaker));
    lost.dropped_at = ts(15);
    acc.apply(update(lost));
    assert!(!snapshot(&acc).tasks[0].warnings.contains(&Lint::LostWaker));
    let mut blocking = update(stats());
    blocking.task_update.as_mut().unwrap().new_tasks[0].kind = tasks::task::Kind::Blocking as i32;
    acc.apply(blocking);
    assert!(!snapshot(&acc).tasks[0].warnings.contains(&Lint::SelfWakes));
    assert!(
        acc.snapshot(&Default::default()).tasks[0]
            .warnings
            .is_empty()
    );

    let mut never_yielded = stats();
    let poll = never_yielded.poll_stats.as_mut().unwrap();
    poll.polls = 1;
    poll.last_poll_ended = None;
    acc.apply(update(never_yielded));
    assert!(
        snapshot(&acc).tasks[0]
            .warnings
            .contains(&Lint::NeverYielded)
    );
}

#[test]
fn future_size_fields_trigger_both_size_lints() {
    let mut acc = Accumulator::default();
    let mut value = update(stats());
    value.task_update.as_mut().unwrap().new_tasks[0].fields =
        [("size.bytes", 2048), ("original_size.bytes", 20000)]
            .map(|(name, size)| api::Field {
                name: Some(api::field::Name::StrName(name.into())),
                value: Some(api::field::Value::U64Val(size)),
                ..Default::default()
            })
            .to_vec();
    acc.apply(value);
    let task = &snapshot(&acc).tasks[0];
    assert_eq!(task.size_bytes, Some(2048));
    assert!(task.warnings.contains(&Lint::LargeFuture));
    assert!(task.warnings.contains(&Lint::AutoBoxedFuture));
}

#[test]
fn resource_operations_resolve_waiting_tasks_attributes_and_readiness() {
    let mut acc = Accumulator::default();
    let mut value = update(stats());
    value.resource_update = Some(resources::ResourceUpdate {
        new_resources: vec![resources::Resource {
            id: Some(api::Id { id: 2 }),
            concrete_type: "Mutex".into(),
            ..Default::default()
        }],
        stats_update: HashMap::from([(
            2,
            resources::Stats {
                created_at: ts(10),
                ..Default::default()
            },
        )]),
        new_poll_ops: vec![resources::PollOp {
            async_op_id: Some(api::Id { id: 3 }),
            is_ready: false,
            ..Default::default()
        }],
        ..Default::default()
    });
    value.async_op_update = Some(async_ops::AsyncOpUpdate {
        new_async_ops: vec![async_ops::AsyncOp {
            id: Some(api::Id { id: 3 }),
            resource_id: Some(api::Id { id: 2 }),
            source: "Mutex::lock".into(),
            ..Default::default()
        }],
        stats_update: HashMap::from([(
            3,
            async_ops::Stats {
                task_id: Some(api::Id { id: 1 }),
                created_at: ts(15),
                attributes: vec![api::Attribute {
                    field: Some(api::Field {
                        name: Some(api::field::Name::StrName("permits".into())),
                        value: Some(api::field::Value::U64Val(0)),
                        ..Default::default()
                    }),
                    unit: None,
                }],
                ..Default::default()
            },
        )]),
        ..Default::default()
    });
    acc.apply(value);
    let snapshot = snapshot(&acc);
    assert_eq!(snapshot.resources[0].concrete_type, "Mutex");
    assert_eq!(snapshot.async_ops[0].resource_id, 2);
    assert_eq!(snapshot.async_ops[0].task_name, "task 1");
    assert_eq!(snapshot.async_ops[0].ready, Some(false));
    assert_eq!(snapshot.async_ops[0].attributes, "permits=0");
}

#[test]
fn retention_prunes_completed_data_and_details_but_keeps_live_data() {
    let mut acc = Accumulator::default();
    let mut completed = stats();
    completed.dropped_at = ts(11);
    acc.apply(update(completed));
    acc.apply_details(tasks::TaskDetails {
        task_id: Some(api::Id { id: 1 }),
        ..Default::default()
    });
    acc.prune(None);
    assert_eq!(snapshot(&acc).tasks.len(), 1);
    acc.prune(Some(Duration::from_secs(5)));
    assert!(snapshot(&acc).tasks.is_empty());
    assert!(snapshot(&acc).details.is_empty());
    acc.apply(update(stats()));
    acc.prune(Some(Duration::ZERO));
    assert_eq!(snapshot(&acc).tasks.len(), 1);
}

#[test]
fn decodes_legacy_and_modern_hdr_histograms_and_reports_corruption() {
    let mut histogram = hdrhistogram::Histogram::<u64>::new(3).unwrap();
    for value in [100, 200, 300, 400, 9000] {
        histogram.record(value).unwrap();
    }
    let mut bytes = Vec::new();
    V2Serializer::new()
        .serialize(&histogram, &mut bytes)
        .unwrap();
    let mut acc = Accumulator::default();
    acc.apply_details(tasks::TaskDetails {
        task_id: Some(api::Id { id: 1 }),
        poll_times_histogram: Some(tasks::task_details::PollTimesHistogram::LegacyHistogram(
            bytes.clone(),
        )),
        scheduled_times_histogram: Some(tasks::DurationHistogram {
            raw_histogram: bytes.clone(),
            high_outliers: 3,
            highest_outlier: Some(50_000),
            ..Default::default()
        }),
        ..Default::default()
    });
    let details = &snapshot(&acc).details[&1];
    assert_eq!(details.poll.count, 5);
    assert_eq!(details.poll.percentiles[2], 300);
    assert_eq!(details.poll.bins.iter().map(|b| b.count).sum::<u64>(), 5);
    assert_eq!(details.scheduled.high_outliers, 3);
    assert_eq!(details.scheduled.highest_outlier, Some(50_000));
    acc.apply_details(tasks::TaskDetails {
        task_id: Some(api::Id { id: 1 }),
        poll_times_histogram: Some(tasks::task_details::PollTimesHistogram::Histogram(
            tasks::DurationHistogram {
                raw_histogram: bytes,
                ..Default::default()
            },
        )),
        ..Default::default()
    });
    assert_eq!(snapshot(&acc).details[&1].poll.count, 5);
    acc.apply_details(tasks::TaskDetails {
        task_id: Some(api::Id { id: 1 }),
        poll_times_histogram: Some(tasks::task_details::PollTimesHistogram::LegacyHistogram(
            vec![1, 2, 3],
        )),
        ..Default::default()
    });
    assert!(snapshot(&acc).details[&1].poll.error.is_some());
}

#[test]
fn malformed_timestamps_do_not_wrap_and_json_snapshot_is_serializable() {
    assert_eq!(
        console_ui::model::timestamp(&Timestamp {
            seconds: -1,
            nanos: 0
        }),
        None
    );
    assert_eq!(
        console_ui::model::timestamp(&Timestamp {
            seconds: 1,
            nanos: -1
        }),
        None
    );
    let json = serde_json::to_string(&console_ui::model::Snapshot {
        details: BTreeMap::new(),
        ..Default::default()
    })
    .unwrap();
    assert!(json.contains("tasks"));
}
