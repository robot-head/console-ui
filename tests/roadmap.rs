use console_api::{self as api, async_ops, instrument, resources, tasks};
use console_ui::{
    analysis::{self, History, RegressionPolicy},
    automation::{EXIT_ASSERTION, EXIT_UNAVAILABLE, Rule, Rules},
    config::{Lint, LintThresholds, Settings},
    investigation::{Investigation, Playback, Recording},
    model::{Accumulator, Snapshot},
    preferences::{EditorPreferences, PathMapping, Preferences, ProjectPolicy},
    query::Query,
    recording::Recorder,
    sharing::{ExportDocument, RedactionPolicy},
    telemetry::HistogramPolicy,
    workspace::{Action, Workspace},
};
use hdrhistogram::serialization::{Serializer, V2Serializer};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};
fn ts(s: i64) -> Option<prost_types::Timestamp> {
    Some(prost_types::Timestamp {
        seconds: s,
        nanos: 0,
    })
}
fn update(time: i64, polls: u64, busy_ms: i32, name: &str) -> instrument::Update {
    let fields = [("task.name", name), ("request.token", "secret-value")]
        .map(|(k, v)| api::Field {
            name: Some(api::field::Name::StrName(k.into())),
            value: Some(api::field::Value::StrVal(v.into())),
            ..Default::default()
        })
        .to_vec();
    instrument::Update {
        now: ts(time),
        task_update: Some(tasks::TaskUpdate {
            new_tasks: vec![tasks::Task {
                id: Some(api::Id { id: 1 }),
                fields,
                location: Some(api::Location {
                    file: Some("src/worker.rs".into()),
                    line: Some(9),
                    column: Some(2),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            stats_update: HashMap::from([(
                1,
                tasks::Stats {
                    created_at: ts(100),
                    wakes: 10,
                    self_wakes: 6,
                    waker_clones: 1,
                    poll_stats: Some(api::PollStats {
                        polls,
                        busy_time: Some(prost_types::Duration {
                            seconds: 0,
                            nanos: busy_ms * 1_000_000,
                        }),
                        last_poll_started: ts(100),
                        last_poll_ended: ts(100),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn sample(time: i64, polls: u64, busy_ms: i32) -> Snapshot {
    let mut acc = Accumulator::default();
    acc.apply(update(time, polls, busy_ms, "worker"));
    acc.snapshot(&Settings::default().warnings)
}
fn detail(time: i64) -> tasks::TaskDetails {
    let mut hist = hdrhistogram::Histogram::<u64>::new(3).unwrap();
    hist.record(100).unwrap();
    let mut raw = vec![];
    V2Serializer::new().serialize(&hist, &mut raw).unwrap();
    tasks::TaskDetails {
        task_id: Some(api::Id { id: 1 }),
        now: ts(time),
        poll_times_histogram: Some(tasks::task_details::PollTimesHistogram::LegacyHistogram(
            raw,
        )),
        ..Default::default()
    }
}

#[test]
fn preferences_are_atomic_versioned_and_bound_recent_targets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prefs.json");
    let mut prefs = Preferences::default();
    prefs
        .set_profile("local".into(), "http://localhost:6669".into())
        .unwrap();
    prefs.set_preset("slow".into(), "busy>1ms".into()).unwrap();
    for port in 1000..1050 {
        prefs.remember(&format!("http://localhost:{port}")).unwrap();
    }
    prefs.remember("http://localhost:1049").unwrap();
    assert_eq!(prefs.recent_targets.len(), 20);
    assert_eq!(prefs.recent_targets[0], "http://localhost:1049");
    prefs.save(&path).unwrap();
    assert_eq!(Preferences::load(&path).unwrap(), prefs);
    prefs.schema_version = 99;
    assert!(prefs.save(&path).is_err());
    assert_eq!(Preferences::load(&path).unwrap().schema_version, 1);
}

#[test]
fn queries_combine_quoted_fields_units_or_and_negation() {
    let s = sample(101, 20, 10);
    let t = &s.tasks[0];
    for q in [
        "busy>=10ms polls>10 !state:running",
        "name:missing | name:worker busy<1s",
        "field.request.token:secret-value",
        "source:worker.rs",
        "name:'worker'",
    ] {
        assert!(Query::parse(q).unwrap().task(t), "{q}");
    }
    assert!(!Query::parse("busy>11ms | name:missing").unwrap().task(t));
    for q in [
        "busy>later",
        "other:foo",
        "| name:x",
        "name:x |",
        "name:'oops",
        "polls>-1",
    ] {
        assert!(Query::parse(q).is_err(), "{q}");
    }
}

#[test]
fn source_mapping_uses_longest_root_and_quotes_arguments_without_a_shell() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a space.rs");
    std::fs::write(&file, "source").unwrap();
    let editor = EditorPreferences {
        mappings: vec![PathMapping {
            remote_root: "/remote/src".into(),
            local_root: dir.path().into(),
        }],
        ..Default::default()
    };
    let (_, args) = editor.command("/remote/src/a space.rs:12:3").unwrap();
    assert_eq!(args.len(), 2);
    assert!(args[1].contains("a space.rs:12:3"));
    assert!(editor.resolve("/remote/src/../missing:1:1").is_err());
}

#[test]
fn thresholds_and_project_precedence_share_the_same_lint_evaluation() {
    let mut t = sample(101, 5, 10).tasks.remove(0);
    assert!(t.warnings.contains(&Lint::SelfWakes));
    let thresholds = LintThresholds {
        self_wake_percent: 80,
        ..Default::default()
    };
    t.lint_with(&Settings::default().warnings, &thresholds);
    assert!(!t.warnings.contains(&Lint::SelfWakes));
    let defaults = Preferences::default();
    let policy = ProjectPolicy {
        schema_version: 1,
        settings: Some(Settings {
            lint_thresholds: thresholds.clone(),
            ..Default::default()
        }),
        presets: vec![],
    };
    assert_eq!(
        policy
            .apply(&defaults, None)
            .unwrap()
            .settings
            .lint_thresholds,
        thresholds
    );
    let override_settings = Settings::default();
    assert_eq!(
        policy
            .apply(&defaults, Some(&override_settings))
            .unwrap()
            .settings,
        override_settings
    );
}

#[test]
fn history_is_bounded_and_resets_at_session_boundaries() {
    let mut history = History::new(1024, 1_000_000_000_000);
    for n in 101..150 {
        history.push(1, &Arc::new(sample(n, n as u64, 10)));
    }
    assert!(history.retained_bytes() <= 1024);
    assert!(!history.points().is_empty());
    let summary = history.window(&sample(150, 150, 20), 100_000_000_000);
    assert!(!summary.complete);
    assert!(summary.note.is_some());
    history.push(2, &Arc::new(sample(101, 1, 1)));
    assert!(history.points().is_empty());
}

#[test]
fn recording_seek_isolates_sessions_and_never_uses_future_histograms() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seek.tcui");
    let mut writer = Recorder::create(&path).unwrap();
    writer.session("http://localhost:6669").unwrap();
    writer.update(&update(101, 1, 1, "first")).unwrap();
    writer.details(&detail(105)).unwrap();
    writer.update(&update(102, 2, 2, "first")).unwrap();
    writer.update(&update(106, 3, 3, "first")).unwrap();
    writer.session("http://localhost:7777").unwrap();
    writer.update(&update(201, 4, 4, "second")).unwrap();
    drop(writer);
    let mut recording = Recording::open(&path).unwrap();
    assert_eq!(recording.sessions.len(), 2);
    assert_eq!(
        recording.sessions[0].target.as_deref(),
        Some("http://localhost:6669")
    );
    assert!(
        recording
            .seek(0, 105_000_000_000, &Settings::default())
            .unwrap()
            .details
            .contains_key(&1)
    );
    let past = recording
        .seek(0, 103_000_000_000, &Settings::default())
        .unwrap();
    assert_eq!(past.tasks[0].timing.polls, 2);
    assert!(past.details.is_empty());
    assert_eq!(
        recording
            .seek(0, 101_000_000_000, &Settings::default())
            .unwrap()
            .tasks[0]
            .timing
            .polls,
        1
    );
    assert_eq!(
        recording
            .final_snapshot(&Settings::default())
            .unwrap()
            .tasks[0]
            .name,
        "second"
    );
}

#[test]
fn recovery_retains_only_complete_frames_and_playback_stops_at_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.tcui");
    let mut writer = Recorder::create(&path).unwrap();
    writer.update(&update(101, 1, 1, "worker")).unwrap();
    drop(writer);
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[1, 20, 0])
        .unwrap();
    assert!(Recording::open(&path).is_err());
    let recovered = Recording::open_with_recovery(&path, true).unwrap();
    assert!(recovered.sessions[0].incomplete);
    assert_eq!(recovered.warnings.len(), 1);
    let mut playback = Playback::new(0, 100);
    playback.speed(2.0).unwrap();
    playback.toggle();
    playback.advance(Duration::from_nanos(50), 180);
    assert_eq!(playback.cursor, 180);
    assert!(!playback.playing);
    assert!(playback.speed(f64::NAN).is_err());
}

#[test]
fn annotations_roundtrip_without_modifying_recording() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.tcui");
    std::fs::write(&path, b"untouched").unwrap();
    let mut investigation = Investigation::new(path.clone());
    investigation.bookmark("stall".into(), "note".into(), 0, 100, Some(1), None);
    let sidecar = Investigation::sidecar(&path);
    investigation.save(&sidecar).unwrap();
    assert_eq!(
        Investigation::load(&sidecar).unwrap().bookmarks[0].note,
        "note"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"untouched");
}

#[test]
fn comparison_matches_sources_instead_of_reusing_numeric_ids() {
    let a = sample(101, 10, 1);
    let mut b = sample(102, 1000, 30);
    b.tasks[0].id = 99;
    let comparison = analysis::compare(&a, &b, [1.0, 1.0], &RegressionPolicy::default()).unwrap();
    assert_eq!(comparison.rows[0].baseline, Some(1));
    assert_eq!(comparison.rows[0].current, Some(99));
    assert!(comparison.rows[0].regression);
    b.tasks.push(b.tasks[0].clone());
    assert_eq!(
        analysis::compare(&a, &b, [1.0, 1.0], &RegressionPolicy::default())
            .unwrap()
            .rows[0]
            .status,
        "ambiguous"
    );
}

#[test]
fn exports_escape_content_and_redaction_removes_values_from_raw_copies() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("raw.tcui");
    let output = dir.path().join("redacted.tcui");
    let mut writer = Recorder::create(&input).unwrap();
    writer.session("http://private-host:6669").unwrap();
    writer
        .update(&update(101, 1, 1, "<script>bad()</script>"))
        .unwrap();
    drop(writer);
    let policy = RedactionPolicy {
        fields: BTreeSet::from(["request.token".into()]),
        source_paths: true,
        endpoints: true,
    };
    policy.recording_copy(&input, &output).unwrap();
    let bytes = std::fs::read(&output).unwrap();
    assert!(
        !bytes
            .windows(b"secret-value".len())
            .any(|b| b == b"secret-value")
    );
    assert!(
        !bytes
            .windows(b"private-host".len())
            .any(|b| b == b"private-host")
    );
    assert!(
        !bytes
            .windows(b"src/worker.rs".len())
            .any(|b| b == b"src/worker.rs")
    );
    let snap = console_ui::recording::replay(&output, &Settings::default()).unwrap();
    let doc = ExportDocument::new(1, None, snap);
    let html = dir.path().join("report.html");
    doc.report(&html, &[], None).unwrap();
    let text = std::fs::read_to_string(html).unwrap();
    assert!(!text.contains("<script>bad()"));
    assert!(text.contains("&lt;script&gt;"));
    doc.csv(&dir.path().join("tasks.csv"), "tasks").unwrap();
    assert!(
        std::fs::read_to_string(dir.path().join("tasks.csv"))
            .unwrap()
            .contains("busy_ns")
    );
    assert!(policy.recording_copy(&input, &input).is_err());
}

#[test]
fn assertions_distinguish_failures_missing_samples_and_invalid_configuration() {
    let snapshot = sample(101, 20, 10);
    let rules = Rules {
        schema_version: 1,
        rules: vec![Rule {
            name: "poll budget".into(),
            query: "name:worker".into(),
            metric: "polls".into(),
            maximum: 10.0,
        }],
    };
    assert_eq!(
        rules.evaluate(&snapshot, false).unwrap().exit_code,
        EXIT_ASSERTION
    );
    let mut missing = rules.clone();
    missing.rules[0].metric = "poll_p99_ns".into();
    assert_eq!(
        missing.evaluate(&snapshot, false).unwrap().exit_code,
        EXIT_UNAVAILABLE
    );
    assert_eq!(
        rules.evaluate(&snapshot, true).unwrap().exit_code,
        EXIT_UNAVAILABLE
    );
    missing.rules[0].maximum = f64::NAN;
    assert!(missing.validate().is_err());
}

#[test]
fn histogram_policy_caps_and_rotates_detail_subscriptions() {
    let mut snapshot = sample(101, 1, 1);
    for id in 2..10 {
        let mut task = snapshot.tasks[0].clone();
        task.id = id;
        snapshot.tasks.push(task);
    }
    let policy = HistogramPolicy {
        mode: "matching".into(),
        max_streams: 3,
        rotate_seconds: 1,
        ..Default::default()
    };
    let first = policy.ids(&snapshot, Some(1), true, 0);
    let second = policy.ids(&snapshot, Some(1), true, 1);
    assert_eq!(first.len(), 3);
    assert_eq!(first[0], 1);
    assert_ne!(first, second);
    assert_eq!(policy.ids(&snapshot, Some(1), false, 0), vec![1]);
}

#[test]
fn contention_edges_report_pending_operations_without_claiming_ownership() {
    let mut acc = Accumulator::default();
    let mut update = update(101, 1, 1, "worker");
    update.resource_update = Some(resources::ResourceUpdate {
        new_resources: vec![resources::Resource {
            id: Some(api::Id { id: 2 }),
            concrete_type: "Mutex".into(),
            ..Default::default()
        }],
        new_poll_ops: vec![resources::PollOp {
            async_op_id: Some(api::Id { id: 3 }),
            is_ready: false,
            ..Default::default()
        }],
        ..Default::default()
    });
    update.async_op_update = Some(async_ops::AsyncOpUpdate {
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
                ..Default::default()
            },
        )]),
        ..Default::default()
    });
    acc.apply(update);
    let graph = analysis::contention_graph(
        &acc.snapshot(&Settings::default().warnings),
        Some("resource:2"),
        10,
    );
    assert_eq!(graph.edges.len(), 1);
    assert!(graph.edges[0].label.contains("pending"));
    assert_eq!(graph.nodes.len(), 2);
}

#[test]
fn workspaces_keep_targets_filters_profiles_and_saved_views_independent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let mut app = Workspace::with_preferences(
        Settings {
            target: "http://127.0.0.1:9".into(),
            ..Default::default()
        },
        path.clone(),
        None,
        None,
    )
    .unwrap();
    let first = app.active;
    app.dispatch(Action::Filter {
        expression: "busy>1ms".into(),
    })
    .unwrap();
    app.dispatch(Action::SaveView { name: "cpu".into() })
        .unwrap();
    app.dispatch(Action::SaveProfile {
        name: "dev".into(),
        target: "http://127.0.0.1:10".into(),
    })
    .unwrap();
    app.dispatch(Action::AddTarget {
        target: "http://127.0.0.1:10".into(),
        name: "other".into(),
    })
    .unwrap();
    assert_eq!(app.current().document.filter, "");
    app.dispatch(Action::SelectTab { id: first }).unwrap();
    assert_eq!(app.current().document.filter, "busy>1ms");
    let persisted = Preferences::load(&path).unwrap();
    assert_eq!(persisted.profiles.len(), 1);
    assert_eq!(persisted.saved_views.len(), 1);
    app.dispatch(Action::CloseTab { id: first }).unwrap();
    assert_eq!(app.tabs.len(), 1);
    assert!(app.dispatch(Action::CloseTab { id: app.active }).is_err());
}

#[test]
fn interval_histograms_subtract_counts_and_report_unavailable_baselines() {
    let mut acc = Accumulator::default();
    acc.apply(update(101, 1, 1, "worker"));
    acc.apply_details(detail(101));
    let baseline = acc.snapshot(&Settings::default().warnings);
    let mut histogram = hdrhistogram::Histogram::<u64>::new(3).unwrap();
    histogram.record(100).unwrap();
    histogram.record(10_000).unwrap();
    let mut bytes = vec![];
    V2Serializer::new()
        .serialize(&histogram, &mut bytes)
        .unwrap();
    acc.apply(update(102, 2, 2, "worker"));
    acc.apply_details(tasks::TaskDetails {
        task_id: Some(api::Id { id: 1 }),
        now: ts(102),
        poll_times_histogram: Some(tasks::task_details::PollTimesHistogram::LegacyHistogram(
            bytes,
        )),
        ..Default::default()
    });
    let end = acc.snapshot(&Settings::default().warnings);
    let (window, incomplete) = analysis::interval(&baseline, &end);
    assert!(!incomplete);
    assert_eq!(window.tasks[0].timing.polls, 1);
    assert_eq!(window.details[&1].poll.count, 1);
    assert!(window.details[&1].poll.percentiles[2] >= 10_000);
    let mut missing = baseline;
    missing.details.clear();
    let (window, _) = analysis::interval(&missing, &end);
    assert!(window.details[&1].poll.error.is_some());
}

#[test]
fn native_bridge_handles_errors_and_preserves_owned_string_lifetimes() {
    use console_ui::ffi::*;
    use std::ffi::{CStr, CString};
    let dir = tempfile::tempdir().unwrap();
    let settings = CString::new(
        serde_json::to_string(&Settings {
            target: "http://127.0.0.1:9".into(),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    let path = CString::new(dir.path().join("ffi.json").to_string_lossy().as_bytes()).unwrap();
    unsafe {
        let bridge = console_ui_create_with_settings(settings.as_ptr(), path.as_ptr());
        assert!(!bridge.is_null());
        assert_eq!(console_ui_abi_version(), 1);
        let action = CString::new(r#"{"action":"filter","expression":"busy>1ms"}"#).unwrap();
        let reply = console_ui_dispatch(bridge, action.as_ptr());
        let json: serde_json::Value =
            serde_json::from_slice(CStr::from_ptr(reply).to_bytes()).unwrap();
        assert_eq!(json["ok"], true);
        console_ui_string_free(reply);
        let view = console_ui_view(bridge);
        let json: serde_json::Value =
            serde_json::from_slice(CStr::from_ptr(view).to_bytes()).unwrap();
        assert_eq!(json["query"], "busy>1ms");
        console_ui_string_free(view);
        let malformed = CString::new(r#"{"action":"unknown"}"#).unwrap();
        let reply = console_ui_dispatch(bridge, malformed.as_ptr());
        let json: serde_json::Value =
            serde_json::from_slice(CStr::from_ptr(reply).to_bytes()).unwrap();
        assert_eq!(json["ok"], false);
        console_ui_string_free(reply);
        assert!(
            !CStr::from_ptr(console_ui_last_error())
                .to_bytes()
                .is_empty()
        );
        console_ui_destroy(bridge);
    }
}
