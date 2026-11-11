use clap::Parser;
use console_api::{Id, instrument, tasks};
use console_ui::{
    config::{Cli, Lint, Settings, validate_target},
    recording::{Recorder, replay},
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "console-ui-test-{}-{}.tcui",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn target_validation_is_explicit_about_windows_transports() {
    for target in [
        "http://127.0.0.1:6669",
        "https://example.com:443",
        "http://[::1]:6669",
    ] {
        validate_target(target).unwrap();
    }
    for target in [
        "file://localhost/tmp/console",
        "vsock://2:6669",
        "ftp://host",
        "http://",
        "http://user:pass@host",
        "http://host/path",
        "http://host?token=secret",
    ] {
        assert!(validate_target(target).is_err(), "{target}");
    }
}

#[test]
fn config_roundtrips_and_command_line_overrides_are_applied() {
    let file = Temp::new();
    let settings = Settings {
        retain_for: "5min".into(),
        ..Default::default()
    };
    std::fs::write(&file.0, toml::to_string(&settings).unwrap()).unwrap();
    let cli = Cli::try_parse_from([
        "console-ui",
        "http://localhost:7777",
        "--config",
        file.0.to_str().unwrap(),
        "--retain-for",
        "none",
        "-W",
        "self-wakes",
        "-A",
        "self-wakes,large-future",
        "--ascii-only",
        "gen-config",
    ])
    .unwrap();
    let loaded = Settings::load(&cli).unwrap();
    assert_eq!(loaded.target, "http://localhost:7777");
    assert_eq!(loaded.retention().unwrap(), None);
    assert!(!loaded.warnings.contains(&Lint::SelfWakes));
    assert!(!loaded.warnings.contains(&Lint::LargeFuture));
    assert!(loaded.ascii_only);
    let roundtrip: Settings = toml::from_str(&toml::to_string(&loaded).unwrap()).unwrap();
    assert_eq!(roundtrip, loaded);
}

#[test]
fn invalid_retention_and_unknown_config_fields_fail() {
    assert!(
        Settings {
            retain_for: "later".into(),
            ..Default::default()
        }
        .retention()
        .is_err()
    );
    assert!(toml::from_str::<Settings>("typo = true").is_err());
}

#[test]
fn records_wire_messages_and_resets_between_connection_sessions() {
    let file = Temp::new();
    let mut recorder = Recorder::create(&file.0).unwrap();
    assert!(Recorder::create(&file.0).is_err());
    let update = |id| instrument::Update {
        now: Some(prost_types::Timestamp {
            seconds: 20,
            nanos: 0,
        }),
        task_update: Some(tasks::TaskUpdate {
            new_tasks: vec![tasks::Task {
                id: Some(Id { id }),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    recorder.update(&update(1)).unwrap();
    recorder.reset().unwrap();
    recorder.update(&update(2)).unwrap();
    recorder
        .details(&tasks::TaskDetails {
            task_id: Some(Id { id: 2 }),
            ..Default::default()
        })
        .unwrap();
    drop(recorder);
    let snapshot = replay(&file.0, &Settings::default()).unwrap();
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.tasks[0].id, 2);
    assert!(snapshot.details.contains_key(&2));
}

#[test]
fn replay_rejects_unknown_truncated_and_oversized_frames() {
    let file = Temp::new();
    for bytes in [
        b"other format".to_vec(),
        b"TCUI\x01\0\0\0\x01\x05\0\0\0\x01".to_vec(),
        b"TCUI\x01\0\0\0\x01\xff\xff\xff\xff".to_vec(),
        b"TCUI\x01\0\0\0\x04\0\0\0\0".to_vec(),
    ] {
        std::fs::write(&file.0, bytes).unwrap();
        assert!(replay(&file.0, &Settings::default()).is_err());
    }
}
