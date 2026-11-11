use console_api::{
    Id, field,
    instrument::{
        self as api,
        instrument_server::{Instrument, InstrumentServer},
    },
    tasks,
};
use console_ui::{
    config::Settings,
    telemetry::{Command, ConnectionStatus, LiveState, Worker},
};
use hdrhistogram::serialization::{Serializer, V2Serializer};
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, watch};
use tokio_stream::{
    Stream, StreamExt,
    wrappers::{BroadcastStream, TcpListenerStream, WatchStream},
};
use tonic::{Request, Response, Status};

type WireStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;
#[derive(Clone)]
struct Producer {
    connections: Arc<AtomicU64>,
    updates: broadcast::Sender<Result<api::Update, Status>>,
    paused: watch::Sender<bool>,
}

fn update(id: u64, name: &str) -> api::Update {
    api::Update {
        now: Some(prost_types::Timestamp {
            seconds: 100,
            nanos: 0,
        }),
        task_update: Some(tasks::TaskUpdate {
            new_tasks: vec![tasks::Task {
                id: Some(Id { id }),
                fields: vec![console_api::Field {
                    name: Some(field::Name::StrName("task.name".into())),
                    value: Some(field::Value::StrVal(name.into())),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tonic::async_trait]
impl Instrument for Producer {
    type WatchUpdatesStream = WireStream<api::Update>;
    async fn watch_updates(
        &self,
        _: Request<api::InstrumentRequest>,
    ) -> Result<Response<Self::WatchUpdatesStream>, Status> {
        let session = self.connections.fetch_add(1, Ordering::SeqCst) + 1;
        let stream = tokio_stream::iter([Ok(update(1, &format!("session-{session}")))])
            .chain(BroadcastStream::new(self.updates.subscribe()).filter_map(Result::ok));
        Ok(Response::new(Box::pin(stream)))
    }
    type WatchTaskDetailsStream = WireStream<tasks::TaskDetails>;
    async fn watch_task_details(
        &self,
        request: Request<api::TaskDetailsRequest>,
    ) -> Result<Response<Self::WatchTaskDetailsStream>, Status> {
        let id = request
            .into_inner()
            .id
            .ok_or_else(|| Status::invalid_argument("missing id"))?;
        if id.id != 1 {
            return Err(Status::not_found("task is not retained"));
        }
        let mut histogram = hdrhistogram::Histogram::<u64>::new(3).unwrap();
        histogram.record(100).unwrap();
        histogram.record(1000).unwrap();
        let mut raw = vec![];
        V2Serializer::new().serialize(&histogram, &mut raw).unwrap();
        Ok(Response::new(Box::pin(tokio_stream::iter([Ok(
            tasks::TaskDetails {
                task_id: Some(id),
                poll_times_histogram: Some(
                    tasks::task_details::PollTimesHistogram::LegacyHistogram(raw),
                ),
                ..Default::default()
            },
        )]))))
    }
    type WatchStateStream = WireStream<api::State>;
    async fn watch_state(
        &self,
        _: Request<api::StateRequest>,
    ) -> Result<Response<Self::WatchStateStream>, Status> {
        Ok(Response::new(Box::pin(
            WatchStream::new(self.paused.subscribe()).map(|paused| {
                Ok(api::State {
                    temporality: if paused { 1 } else { 0 },
                })
            }),
        )))
    }
    async fn pause(
        &self,
        _: Request<api::PauseRequest>,
    ) -> Result<Response<api::PauseResponse>, Status> {
        self.paused.send_replace(true);
        Ok(Response::new(api::PauseResponse {}))
    }
    async fn resume(
        &self,
        _: Request<api::ResumeRequest>,
    ) -> Result<Response<api::ResumeResponse>, Status> {
        self.paused.send_replace(false);
        Ok(Response::new(api::ResumeResponse {}))
    }
}

async fn wait(worker: &Worker, predicate: impl Fn(&LiveState) -> bool) -> LiveState {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let state = worker.read();
        if predicate(&state) {
            return state;
        }
        assert!(Instant::now() < deadline, "Timed out: {state:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streams_details_pause_resume_and_reconnects_without_mixing_sessions() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}", listener.local_addr().unwrap());
    let producer = Producer {
        connections: Arc::new(AtomicU64::new(0)),
        updates: broadcast::channel(16).0,
        paused: watch::channel(false).0,
    };
    let server = tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(InstrumentServer::new(producer.clone()))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    let worker = Worker::start(
        Settings {
            target,
            ..Default::default()
        },
        None,
        None,
    )
    .unwrap();
    let first = wait(&worker, |s| s.snapshot.tasks.len() == 1).await;
    assert_eq!(first.status, ConnectionStatus::Connected);
    assert_eq!(first.snapshot.tasks[0].name, "session-1");
    producer
        .updates
        .send(Ok(update(2, "old-session-task")))
        .unwrap();
    wait(&worker, |s| s.snapshot.tasks.len() == 2).await;
    worker.send(Command::Inspect(Some(1)));
    wait(&worker, |s| s.snapshot.details.contains_key(&1)).await;
    let directory = tempfile::tempdir().unwrap();
    let capture = directory.path().join("started-mid-session.tcui");
    worker.send(Command::StartRecording(capture.clone()));
    wait(&worker, |s| s.recording.as_ref() == Some(&capture)).await;
    worker.send(Command::StopRecording);
    wait(&worker, |s| s.recording.is_none()).await;
    let saved = console_ui::recording::replay(
        &capture,
        &Settings {
            retain_for: "none".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        saved.tasks.len(),
        2,
        "Mid-session recording must seed all existing entities"
    );
    assert_eq!(
        saved.details[&1].poll.count, 2,
        "Mid-session recording must seed the exact histogram counts"
    );
    assert!(
        console_ui::investigation::Recording::open(&capture)
            .unwrap()
            .sessions[0]
            .histogram_policy
            .is_some()
    );
    worker.send(Command::Pause(true));
    wait(&worker, |s| s.paused).await;
    assert!(*producer.paused.borrow());
    worker.send(Command::Pause(false));
    wait(&worker, |s| !s.paused).await;
    producer
        .updates
        .send(Err(Status::unavailable("target restarted")))
        .unwrap();
    wait(&worker, |s| s.status == ConnectionStatus::Disconnected).await;
    let second = wait(&worker, |s| s.session >= 2 && !s.snapshot.tasks.is_empty()).await;
    assert_eq!(second.snapshot.tasks.len(), 1);
    assert_eq!(second.snapshot.tasks[0].name, "session-2");
    assert!(second.snapshot.details.is_empty());
    worker.send(Command::Inspect(Some(999)));
    wait(&worker, |s| {
        s.notice.as_ref().is_some_and(|m| m.contains("unavailable"))
    })
    .await;
    assert_eq!(worker.read().status, ConnectionStatus::Connected);
    drop(worker);
    server.abort();
}
