//! Verifies compatibility with the actual upstream producer, not only a protocol fixture.
use console_ui::{
    config::Settings,
    telemetry::{Command, Worker},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tracing_subscriber::prelude::*;

#[test]
fn real_subscriber_streams_tasks_resources_and_histograms() {
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let layer = console_subscriber::ConsoleLayer::builder()
        .server_addr(address)
        .publish_interval(Duration::from_millis(50))
        .spawn();
    tracing_subscriber::registry().with(layer).init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let mutex = Arc::new(tokio::sync::Mutex::new(0));
    let _task = tokio::task::Builder::new()
        .name("real-worker")
        .spawn(async move {
            loop {
                *mutex.lock().await += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .unwrap();
    let worker = Worker::start(
        Settings {
            target: format!("http://{address}"),
            ..Default::default()
        },
        None,
        None,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let id = loop {
        let state = worker.read();
        if !state.snapshot.resources.is_empty()
            && let Some(task) = state
                .snapshot
                .tasks
                .iter()
                .find(|t| t.name == "real-worker")
            && task.timing.polls > 0
        {
            break task.id;
        }
        assert!(Instant::now() < deadline, "No real telemetry: {state:?}");
        std::thread::sleep(Duration::from_millis(20));
    };
    worker.send(Command::Inspect(Some(id)));
    loop {
        let state = worker.read();
        if state
            .snapshot
            .details
            .get(&id)
            .is_some_and(|d| d.poll.count > 0)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "No histogram from actual subscriber: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
