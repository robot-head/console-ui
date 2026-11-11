use crate::{
    config::{Lint, Settings, validate_target},
    model::{Accumulator, Snapshot},
    recording::{self, Recorder},
};
use anyhow::{Context, Result};
use console_api::{
    Id,
    instrument::{self, instrument_client::InstrumentClient},
    tasks,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};
use tokio::sync::mpsc;
use tonic::{
    Streaming,
    transport::{Channel, Endpoint},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    Offline,
}

#[derive(Clone, Debug)]
pub struct LiveState {
    pub status: ConnectionStatus,
    pub target: String,
    pub paused: bool,
    pub session: u64,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub snapshot: Arc<Snapshot>,
}
impl LiveState {
    pub fn new(target: String) -> Self {
        Self {
            status: ConnectionStatus::Connecting,
            target,
            paused: false,
            session: 0,
            error: None,
            notice: None,
            snapshot: Arc::new(Snapshot::default()),
        }
    }
}
#[derive(Debug)]
pub enum Command {
    Connect(String),
    Pause(bool),
    Inspect(Option<u64>),
    Warnings(BTreeSet<Lint>),
    Stop,
}

pub struct Worker {
    pub state: Arc<RwLock<LiveState>>,
    sender: mpsc::UnboundedSender<Command>,
}
impl Worker {
    pub fn start(
        settings: Settings,
        record: Option<PathBuf>,
        replay: Option<PathBuf>,
    ) -> Result<Self> {
        validate_target(&settings.target)?;
        settings.retention()?;
        let recorder = record.as_deref().map(Recorder::create).transpose()?;
        let state = Arc::new(RwLock::new(LiveState::new(settings.target.clone())));
        let shared = Arc::clone(&state);
        let (sender, receiver) = mpsc::unbounded_channel();
        thread::Builder::new()
            .name("console-telemetry".into())
            .spawn(move || {
                if let Some(path) = replay {
                    let result = recording::replay(&path, &settings);
                    let mut state = shared.write().unwrap();
                    state.status = ConnectionStatus::Offline;
                    state.target = path.display().to_string();
                    state.paused = true;
                    match result {
                        Ok(snapshot) => state.snapshot = Arc::new(snapshot),
                        Err(error) => state.error = Some(format!("{error:#}")),
                    }
                    return;
                }
                match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(run(settings, shared, receiver, recorder)),
                    Err(error) => {
                        let mut state = shared.write().unwrap();
                        state.status = ConnectionStatus::Disconnected;
                        state.error = Some(format!("Start telemetry runtime: {error}"));
                    }
                }
            })
            .context("Start telemetry worker")?;
        Ok(Self { state, sender })
    }
    pub fn send(&self, command: Command) {
        let _ = self.sender.send(command);
    }
    pub fn read(&self) -> LiveState {
        self.state.read().unwrap().clone()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Stop);
    }
}

async fn connect(
    target: &str,
) -> Result<(
    InstrumentClient<Channel>,
    Streaming<instrument::Update>,
    Streaming<instrument::State>,
)> {
    let endpoint =
        Endpoint::from_shared(target.to_owned())?.connect_timeout(Duration::from_secs(3));
    #[cfg(feature = "tls")]
    let endpoint = if target.starts_with("https://") {
        endpoint.tls_config(tonic::transport::ClientTlsConfig::new().with_native_roots())?
    } else {
        endpoint
    };
    #[cfg(not(feature = "tls"))]
    if target.starts_with("https://") {
        anyhow::bail!("HTTPS requires the 'tls' Cargo feature (enabled by default)");
    }
    let channel = endpoint
        .connect()
        .await
        .context("Connect to console-subscriber")?;
    let mut client = InstrumentClient::new(channel).max_decoding_message_size(16 * 1024 * 1024);
    let updates = client
        .watch_updates(instrument::InstrumentRequest {})
        .await?
        .into_inner();
    let state = client
        .watch_state(instrument::StateRequest {})
        .await?
        .into_inner();
    Ok((client, updates, state))
}
fn record_result(result: Result<()>, recorder: &mut Option<Recorder>, shared: &RwLock<LiveState>) {
    if let Err(error) = result {
        *recorder = None;
        shared.write().unwrap().notice = Some(format!("Recording stopped: {error:#}"));
    }
}
fn publish(accumulator: &Accumulator, settings: &Settings, shared: &RwLock<LiveState>) {
    shared.write().unwrap().snapshot = Arc::new(accumulator.snapshot(&settings.warnings));
}

async fn run(
    mut settings: Settings,
    shared: Arc<RwLock<LiveState>>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut recorder: Option<Recorder>,
) {
    let mut target = settings.target.clone();
    let retention = settings.retention().expect("validated retention");
    let mut requested_pause = false;
    let mut retry = 1u64;
    'reconnect: loop {
        {
            let mut state = shared.write().unwrap();
            state.status = ConnectionStatus::Connecting;
            state.target.clone_from(&target);
        }
        let connection = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(5), connect(&target)) => {
                result.map_err(anyhow::Error::from).and_then(|r| r)
            },
            command = commands.recv() => {
                match command {
                    Some(Command::Connect(next)) => target = next,
                    Some(Command::Pause(paused)) => requested_pause = paused,
                    Some(Command::Inspect(_)) => {},
                    Some(Command::Warnings(warnings)) => settings.warnings = warnings,
                    Some(Command::Stop) | None => return,
                }
                continue 'reconnect;
            }
        };
        match connection {
            Ok((mut client, mut updates, mut states)) => {
                retry = 1;
                let mut accumulator = Accumulator::default();
                let mut details: Option<Streaming<tasks::TaskDetails>> = None;
                {
                    let mut state = shared.write().unwrap();
                    state.status = ConnectionStatus::Connected;
                    state.error = None;
                    state.paused = false;
                    state.session += 1;
                    state.snapshot = Arc::new(Snapshot::default());
                }
                if let Some(writer) = &mut recorder {
                    let result = writer.reset();
                    record_result(result, &mut recorder, &shared);
                }
                if requested_pause {
                    match tokio::time::timeout(
                        Duration::from_secs(3),
                        client.pause(instrument::PauseRequest {}),
                    )
                    .await
                    {
                        Ok(Ok(_)) => shared.write().unwrap().paused = true,
                        _ => {
                            shared.write().unwrap().notice =
                                Some("Could not restore paused state".into())
                        }
                    }
                }
                loop {
                    tokio::select! {
                        update = updates.message() => match update {
                            Ok(Some(update)) => {
                                if let Some(writer) = &mut recorder {
                                    let result = writer.update(&update);
                                    record_result(result, &mut recorder, &shared);
                                }
                                accumulator.apply(update);
                                accumulator.prune(retention);
                                publish(&accumulator, &settings, &shared);
                            }
                            result => {
                                shared.write().unwrap().error = Some(match result {
                                    Err(error) => format!("Telemetry stream: {error}"),
                                    _ => "Target closed the telemetry stream".into(),
                                });
                                break;
                            }
                        },
                        state = states.message() => match state {
                            Ok(Some(state)) => shared.write().unwrap().paused = state.temporality == instrument::Temporality::Paused as i32,
                            _ => { shared.write().unwrap().error = Some("Target closed the state stream".into()); break; }
                        },
                        detail = async {
                            match &mut details {
                                Some(stream) => stream.message().await,
                                None => std::future::pending().await,
                            }
                        } => match detail {
                            Ok(Some(detail)) => {
                                if let Some(writer) = &mut recorder {
                                    let result = writer.details(&detail);
                                    record_result(result, &mut recorder, &shared);
                                }
                                accumulator.apply_details(detail);
                                publish(&accumulator, &settings, &shared);
                            }
                            Ok(None) => details = None,
                            Err(error) => {
                                details = None;
                                shared.write().unwrap().notice = Some(format!("Task detail stream: {error}"));
                            }
                        },
                        command = commands.recv() => match command {
                            Some(Command::Connect(next)) => { target = next; requested_pause = false; continue 'reconnect; }
                            Some(Command::Pause(paused)) => {
                                let result = tokio::time::timeout(Duration::from_secs(3), async {
                                    if paused { client.pause(instrument::PauseRequest {}).await.map(|_| ()) }
                                    else { client.resume(instrument::ResumeRequest {}).await.map(|_| ()) }
                                }).await;
                                match result {
                                    Ok(Ok(())) => {
                                        requested_pause = paused;
                                        shared.write().unwrap().paused = paused;
                                    }
                                    error => shared.write().unwrap().notice = Some(format!("Pause/resume failed: {error:?}")),
                                }
                            }
                            Some(Command::Inspect(id)) => {
                                details = None;
                                shared.write().unwrap().notice = None;
                                if let Some(id) = id {
                                    match tokio::time::timeout(Duration::from_secs(3), client.watch_task_details(
                                        instrument::TaskDetailsRequest { id: Some(Id { id }) })).await {
                                        Ok(Ok(response)) => details = Some(response.into_inner()),
                                        error => shared.write().unwrap().notice = Some(format!("Task details unavailable: {error:?}")),
                                    }
                                }
                            }
                            Some(Command::Warnings(warnings)) => {
                                settings.warnings = warnings;
                                publish(&accumulator, &settings, &shared);
                            }
                            Some(Command::Stop) | None => return,
                        }
                    }
                }
            }
            Err(error) => shared.write().unwrap().error = Some(format!("{error:#}")),
        }
        shared.write().unwrap().status = ConnectionStatus::Disconnected;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(retry)) => {},
            command = commands.recv() => match command {
                Some(Command::Connect(next)) => { target = next; requested_pause = false; retry = 1; }
                Some(Command::Pause(paused)) => requested_pause = paused,
                Some(Command::Inspect(_)) => {},
                Some(Command::Warnings(warnings)) => settings.warnings = warnings,
                Some(Command::Stop) | None => return,
            }
        }
        retry = (retry * 2).min(10);
    }
}
