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
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    thread,
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_stream::{StreamExt, StreamMap};
use tonic::{
    Streaming,
    transport::{Channel, Endpoint},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    pub recording: Option<PathBuf>,
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
            recording: None,
        }
    }
}
#[derive(Debug)]
pub enum Command {
    Connect(String),
    Pause(bool),
    Inspect(Option<u64>),
    Warnings(BTreeSet<Lint>),
    Settings(Settings),
    Disconnect,
    StartRecording(PathBuf),
    StopRecording,
    HistogramPolicy(HistogramPolicy),
    Stop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistogramPolicy {
    pub mode: String,
    pub tasks: Vec<u64>,
    pub query: String,
    pub max_streams: usize,
    pub rotate_seconds: u64,
}
impl Default for HistogramPolicy {
    fn default() -> Self {
        Self {
            mode: "inspected".into(),
            tasks: vec![],
            query: String::new(),
            max_streams: 4,
            rotate_seconds: 30,
        }
    }
}
impl HistogramPolicy {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            ["inspected", "selected", "matching"].contains(&self.mode.as_str()),
            "Unknown histogram capture policy"
        );
        anyhow::ensure!(
            (1..=32).contains(&self.max_streams) && self.rotate_seconds > 0,
            "Histogram policy needs 1–32 streams and positive rotation duration"
        );
        crate::query::Query::parse(&self.query)?;
        Ok(())
    }
    pub fn ids(
        &self,
        snapshot: &Snapshot,
        inspected: Option<u64>,
        recording: bool,
        tick: u64,
    ) -> Vec<u64> {
        let mut result: Vec<_> = inspected.into_iter().collect();
        if !recording {
            return result;
        }
        let mut candidates: Vec<_> = match self.mode.as_str() {
            "selected" => self.tasks.clone(),
            "matching" => {
                let query = crate::query::Query::parse(&self.query).unwrap_or_default();
                snapshot
                    .tasks
                    .iter()
                    .filter(|t| t.state != crate::model::TaskState::Completed && query.task(t))
                    .map(|t| t.id)
                    .collect()
            }
            _ => vec![],
        };
        candidates.sort_unstable();
        candidates.dedup();
        candidates.retain(|id| !result.contains(id));
        if !candidates.is_empty() {
            let offset = ((tick / self.rotate_seconds.max(1)) as usize * self.max_streams)
                % candidates.len();
            candidates.rotate_left(offset);
        }
        result.extend(
            candidates
                .into_iter()
                .take(self.max_streams.saturating_sub(result.len())),
        );
        result
    }
}

pub struct Worker {
    pub state: Arc<RwLock<LiveState>>,
    sender: mpsc::UnboundedSender<Command>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}
impl Worker {
    pub fn start(
        settings: Settings,
        record: Option<PathBuf>,
        replay: Option<PathBuf>,
    ) -> Result<Self> {
        validate_target(&settings.target)?;
        settings.retention()?;
        settings.lint_thresholds.validate()?;
        let recorder = record.as_deref().map(Recorder::create).transpose()?;
        let state = Arc::new(RwLock::new(LiveState::new(settings.target.clone())));
        state.write().unwrap().recording = record;
        let shared = Arc::clone(&state);
        let (sender, receiver) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
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
        Ok(Self {
            state,
            sender,
            thread: Mutex::new(Some(thread)),
        })
    }
    pub fn send(&self, command: Command) {
        let _ = self.sender.send(command);
    }
    pub fn read(&self) -> LiveState {
        self.state.read().unwrap().clone()
    }
    pub fn offline(snapshot: Snapshot, path: &Path) -> Self {
        let mut state = LiveState::new(path.display().to_string());
        state.status = ConnectionStatus::Offline;
        state.paused = true;
        state.snapshot = Arc::new(snapshot);
        let (sender, _) = mpsc::unbounded_channel();
        Self {
            state: Arc::new(RwLock::new(state)),
            sender,
            thread: Mutex::new(None),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Stop);
        if let Ok(slot) = self.thread.get_mut()
            && let Some(thread) = slot.take()
        {
            let _ = thread.join();
        }
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
        let mut state = shared.write().unwrap();
        state.recording = None;
        state.notice = Some(format!("Recording stopped: {error:#}"));
    }
}
fn publish(accumulator: &Accumulator, settings: &Settings, shared: &RwLock<LiveState>) {
    let mut snapshot = accumulator.snapshot(&settings.warnings);
    for t in &mut snapshot.tasks {
        t.lint_with(&settings.warnings, &settings.lint_thresholds);
    }
    shared.write().unwrap().snapshot = Arc::new(snapshot);
}

fn idle(
    command: Command,
    target: &mut String,
    settings: &mut Settings,
    policy: &mut HistogramPolicy,
    recorder: &mut Option<Recorder>,
    shared: &RwLock<LiveState>,
    auto: &mut bool,
) -> bool {
    match command {
        Command::Connect(next) => {
            if let Err(e) = validate_target(&next) {
                shared.write().unwrap().notice = Some(e.to_string());
            } else {
                *target = next;
                *auto = true;
            }
        }
        Command::Disconnect => *auto = false,
        Command::Warnings(w) => settings.warnings = w,
        Command::Settings(next) => {
            if next.retention().is_ok() && next.lint_thresholds.validate().is_ok() {
                *settings = next;
            }
        }
        Command::HistogramPolicy(next) => {
            if next.validate().is_ok() {
                *policy = next;
            }
        }
        Command::StopRecording => {
            if let Some(mut writer) = recorder.take()
                && let Err(e) = writer.flush()
            {
                shared.write().unwrap().notice = Some(e.to_string());
            }
            shared.write().unwrap().recording = None;
        }
        Command::StartRecording(_) => {
            shared.write().unwrap().notice = Some("Connect before starting a recording".into())
        }
        Command::Stop => {
            if let Some(writer) = recorder {
                let _ = writer.flush();
            }
            return false;
        }
        _ => {}
    }
    true
}

async fn run(
    mut settings: Settings,
    shared: Arc<RwLock<LiveState>>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut recorder: Option<Recorder>,
) {
    let mut target = settings.target.clone();
    let mut auto_connect = true;
    let mut policy = HistogramPolicy::default();
    let mut requested_pause = false;
    let mut retry = 1u64;
    'reconnect: loop {
        if !auto_connect {
            shared.write().unwrap().status = ConnectionStatus::Disconnected;
            match commands.recv().await {
                Some(command) => {
                    if !idle(
                        command,
                        &mut target,
                        &mut settings,
                        &mut policy,
                        &mut recorder,
                        &shared,
                        &mut auto_connect,
                    ) {
                        return;
                    }
                }
                None => return,
            }
            continue;
        }
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
                    Some(Command::Pause(paused)) => requested_pause = paused,
                    Some(command)=>if !idle(command,&mut target,&mut settings,&mut policy,&mut recorder,&shared,&mut auto_connect){return;},
                    None=>return,
                }
                continue 'reconnect;
            }
        };
        match connection {
            Ok((mut client, mut updates, mut states)) => {
                retry = 1;
                let mut accumulator = Accumulator::default();
                let mut details = StreamMap::<u64, Streaming<tasks::TaskDetails>>::new();
                let mut inspected = None;
                let mut retired = BTreeSet::new();
                let mut refresh = tokio::time::interval(Duration::from_secs(1));
                refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut ticks = 0u64;
                {
                    let mut state = shared.write().unwrap();
                    state.status = ConnectionStatus::Connected;
                    state.error = None;
                    state.paused = false;
                    state.session += 1;
                    state.snapshot = Arc::new(Snapshot::default());
                }
                if let Some(writer) = &mut recorder {
                    let result = writer.session(&target);
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
                                accumulator.prune(settings.retention().ok().flatten());
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
                        detail = details.next(), if !details.is_empty() => match detail {
                            Some((_,Ok(detail))) => {
                                if let Some(writer) = &mut recorder {
                                    let result = writer.details(&detail);
                                    record_result(result, &mut recorder, &shared);
                                }
                                accumulator.apply_details(detail);
                                publish(&accumulator, &settings, &shared);
                            }
                            None => {},
                            Some((id,Err(error))) => {
                                details.remove(&id);retired.insert(id);
                                shared.write().unwrap().notice = Some(format!("Task detail stream: {error}"));
                            }
                        },
                        _=refresh.tick()=>{
                            let snapshot=shared.read().unwrap().snapshot.clone();
                            let wanted=policy.ids(&snapshot,inspected,recorder.is_some() || policy.mode != "inspected",ticks);ticks+=1;
                            for id in details.keys().copied().collect::<Vec<_>>(){if !wanted.contains(&id){details.remove(&id);}}
                            let subscribe:Vec<_>=wanted.into_iter().filter(|id|!retired.contains(id)).collect();
                            for id in subscribe{
                                if details.contains_key(&id){continue;}
                                match tokio::time::timeout(Duration::from_millis(300),client.watch_task_details(instrument::TaskDetailsRequest{id:Some(Id{id})})).await{
                                    Ok(Ok(response))=>{details.insert(id,response.into_inner());},
                                    error=>{retired.insert(id);if Some(id)==inspected{shared.write().unwrap().notice=Some(format!("Task details unavailable: {error:?}"));}},
                                }
                            }
                        },
                        command = commands.recv() => match command {
                            Some(Command::Connect(next)) => { target = next; requested_pause = false; continue 'reconnect; }
                            Some(Command::Disconnect)=>{auto_connect=false;requested_pause=false;continue 'reconnect;}
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
                                inspected=id;
                                shared.write().unwrap().notice = None;
                                if let Some(id) = id {
                                    retired.remove(&id);
                                    match tokio::time::timeout(Duration::from_secs(3), client.watch_task_details(
                                        instrument::TaskDetailsRequest { id: Some(Id { id }) })).await {
                                        Ok(Ok(response)) => {details.insert(id,response.into_inner());},
                                        error => shared.write().unwrap().notice = Some(format!("Task details unavailable: {error:?}")),
                                    }
                                }
                            }
                            Some(Command::Warnings(warnings)) => {
                                settings.warnings = warnings;
                                publish(&accumulator, &settings, &shared);
                            }
                            Some(Command::Settings(next))=>{
                                if next.retention().is_ok()&&next.lint_thresholds.validate().is_ok(){settings=next;publish(&accumulator,&settings,&shared);}
                            },
                            Some(Command::HistogramPolicy(next))=>{if next.validate().is_ok(){policy=next;retired.clear();}},
                            Some(Command::StartRecording(path))=>{
                                if recorder.is_some(){shared.write().unwrap().notice=Some("A recording is already active".into());continue;}
                                let result=(||->Result<Recorder>{let mut writer=Recorder::create(&path)?;writer.session(&target)?;writer.update(&accumulator.wire_snapshot())?;for detail in accumulator.wire_details(){writer.details(&detail)?;}Ok(writer)})();
                                match result{Ok(writer)=>{recorder=Some(writer);shared.write().unwrap().recording=Some(path);},Err(error)=>shared.write().unwrap().notice=Some(format!("Start recording: {error:#}"))}
                            },
                            Some(Command::StopRecording)=>{idle(Command::StopRecording,&mut target,&mut settings,&mut policy,&mut recorder,&shared,&mut auto_connect);},
                            Some(Command::Stop) | None => {if let Some(writer)=&mut recorder{let _=writer.flush();}return;},
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
                Some(Command::Pause(paused)) => requested_pause = paused,
                Some(command)=>{if !idle(command,&mut target,&mut settings,&mut policy,&mut recorder,&shared,&mut auto_connect){return;}retry=1;},
                None=>return,
            }
        }
        retry = (retry * 2).min(10);
    }
}
