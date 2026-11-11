//! Shared application controller. Native shells render this state and dispatch typed actions.
use crate::{
    analysis::{self, Comparison, History, RegressionPolicy, TrendPoint, WindowSummary},
    config::{LintThresholds, Settings},
    investigation::{Bookmark, Investigation, InvestigationView, Playback, Recording, SessionInfo},
    model::{Snapshot, TaskRow},
    preferences::{
        self, EditorPreferences, Preferences, ProjectPolicy, TablePreferences, load_json, save_json,
    },
    query::Query,
    sharing::{ExportDocument, RedactionPolicy},
    telemetry::{Command, ConnectionStatus, HistogramPolicy, Worker},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, mpsc},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub kind: String,
    pub id: u64,
    pub session: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TabDocument {
    pub id: u64,
    pub name: String,
    pub target: String,
    pub recording: Option<PathBuf>,
    pub filter: String,
    pub tables: BTreeMap<String, TablePreferences>,
    pub view: Option<InvestigationView>,
    pub histogram_policy: HistogramPolicy,
    pub active_preset: Option<String>,
}
impl Default for TabDocument {
    fn default() -> Self {
        Self {
            id: 1,
            name: "Local runtime".into(),
            target: Settings::default().target,
            recording: None,
            filter: String::new(),
            tables: BTreeMap::new(),
            view: None,
            histogram_policy: Default::default(),
            active_preset: None,
        }
    }
}
pub struct Tab {
    pub document: TabDocument,
    pub worker: Arc<Worker>,
    pub history: History,
    pub selection: Option<Selection>,
    pub recording: Option<Recording>,
    pub playback: Option<Playback>,
    pub investigation: Option<Investigation>,
    pub window_ns: Option<u64>,
    pub policy: HistogramPolicy,
    last_session: u64,
    seek_generation: u64,
    seeking: bool,
    seek_request: Option<(usize, u64)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceDocument {
    pub schema_version: u32,
    pub name: String,
    pub active: u64,
    pub tabs: Vec<TabDocument>,
    pub policy: ProjectPolicy,
    pub views: Vec<InvestigationView>,
}
enum Job {
    Open(Box<Opened>),
    Finished(String),
    OpenFailed {
        tab: u64,
        message: String,
    },
    Compared(Box<Comparison>),
    Seeked {
        tab: u64,
        generation: u64,
        result: Result<Box<Seeked>>,
    },
}
struct Seeked {
    recording: Recording,
    snapshot: Snapshot,
    history: History,
    session: usize,
}
struct Opened {
    tab: u64,
    recording: Recording,
    snapshot: Snapshot,
    investigation: Investigation,
    restore: Option<InvestigationView>,
    history: History,
}
pub struct Workspace {
    pub preferences: Preferences,
    pub settings: Settings,
    pub name: String,
    pub active: u64,
    pub tabs: Vec<Tab>,
    pub views: Vec<InvestigationView>,
    pub project_policy: ProjectPolicy,
    pub notice: Option<String>,
    pub comparison: Option<Comparison>,
    pub pending: usize,
    pub report_sections: Vec<String>,
    preview_policy: Option<RedactionPolicy>,
    lookup_query: Option<String>,
    path: PathBuf,
    cli_settings: Option<Settings>,
    session_settings: Option<Settings>,
    next_id: u64,
    jobs: mpsc::Sender<Result<Job>>,
    results: mpsc::Receiver<Result<Job>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Tick,
    Lookup {
        text: String,
    },
    SelectInSession {
        kind: String,
        id: u64,
        session: u64,
    },
    Preview {
        policy: RedactionPolicy,
    },
    ReportSections {
        sections: Vec<String>,
    },
    AddTarget {
        target: String,
        name: String,
    },
    SelectTab {
        id: u64,
    },
    CloseTab {
        id: u64,
    },
    MoveTab {
        id: u64,
        index: usize,
    },
    Connect {
        target: String,
    },
    Disconnect,
    Pause,
    Filter {
        expression: String,
    },
    Select {
        kind: String,
        id: u64,
    },
    ClearSelection,
    SaveProfile {
        name: String,
        target: String,
    },
    DeleteProfile {
        name: String,
    },
    RenameProfile {
        name: String,
        new_name: String,
    },
    ApplyProfile {
        name: String,
    },
    SavePreset {
        name: String,
    },
    SaveProjectPreset {
        name: String,
    },
    DeleteProjectPreset {
        name: String,
    },
    DeletePreset {
        name: String,
    },
    RenamePreset {
        name: String,
        new_name: String,
        #[serde(default)]
        project: bool,
    },
    ApplyPreset {
        name: String,
    },
    Table {
        table: String,
        preferences: TablePreferences,
    },
    Editor {
        preferences: EditorPreferences,
    },
    OpenSource,
    Thresholds {
        thresholds: LintThresholds,
    },
    Settings {
        settings: Settings,
    },
    Appearance {
        palette: crate::config::Palette,
    },
    LintEnabled {
        lint: crate::config::Lint,
        enabled: bool,
    },
    StartRecording {
        path: PathBuf,
    },
    StopRecording,
    OpenRecording {
        path: PathBuf,
        recover: bool,
    },
    Session {
        index: usize,
    },
    Seek {
        cursor: u64,
    },
    Playback,
    Speed {
        speed: f64,
    },
    Window {
        nanoseconds: Option<u64>,
    },
    HistogramPolicy {
        policy: HistogramPolicy,
    },
    Bookmark {
        name: String,
        note: String,
    },
    EditBookmark {
        id: u64,
        name: String,
        note: String,
    },
    DeleteBookmark {
        id: u64,
    },
    GoBookmark {
        id: u64,
    },
    SaveView {
        name: String,
    },
    LoadView {
        name: String,
    },
    DeleteView {
        name: String,
    },
    RenameView {
        name: String,
        new_name: String,
    },
    DuplicateView {
        name: String,
        new_name: String,
    },
    SaveWorkspace {
        path: PathBuf,
        name: String,
    },
    OpenWorkspace {
        path: PathBuf,
    },
    NewWorkspace {
        name: String,
    },
    LoadPolicy {
        path: PathBuf,
    },
    SavePolicy {
        path: PathBuf,
    },
    Compare {
        path: PathBuf,
        session: usize,
        policy: RegressionPolicy,
    },
    Export {
        path: PathBuf,
        format: String,
        table: String,
        redaction: RedactionPolicy,
    },
    RedactRecording {
        path: PathBuf,
        policy: RedactionPolicy,
    },
}

#[derive(Serialize)]
pub struct TabSummary {
    pub id: u64,
    pub name: String,
    pub target: String,
    pub status: ConnectionStatus,
    pub recording: Option<PathBuf>,
}
#[derive(Serialize)]
pub struct View {
    pub name: String,
    pub active: u64,
    pub tabs: Vec<TabSummary>,
    pub status: ConnectionStatus,
    pub paused: bool,
    pub target: String,
    pub recording: Option<PathBuf>,
    pub session: u64,
    pub snapshot: Snapshot,
    pub inspection: Snapshot,
    pub lookup_tasks: Vec<TaskRow>,
    pub export_preview: Option<ExportDocument>,
    pub histogram_policy: HistogramPolicy,
    pub histogram_notice: Option<String>,
    pub report_sections: Vec<String>,
    pub project_presets: Vec<String>,
    pub unfiltered_counts: [usize; 3],
    pub query: String,
    pub selection: Option<Selection>,
    pub trends: Vec<TrendPoint>,
    pub window: Option<WindowSummary>,
    pub graph: analysis::ContentionGraph,
    pub evidence: Vec<analysis::Evidence>,
    pub sessions: Vec<SessionInfo>,
    pub cursor: Option<u64>,
    pub playing: bool,
    pub speed: f64,
    pub bookmarks: Vec<Bookmark>,
    pub views: Vec<InvestigationView>,
    pub preferences: Preferences,
    pub comparison: Option<Comparison>,
    pub notice: Option<String>,
    pub pending: usize,
}

impl Workspace {
    pub fn shutdown(&self) {
        for tab in &self.tabs {
            tab.worker.shutdown();
        }
    }
    pub fn new(
        settings: Settings,
        record: Option<PathBuf>,
        replay: Option<PathBuf>,
    ) -> Result<Self> {
        Self::with_preferences(settings, Preferences::path()?, record, replay)
    }
    pub fn with_preferences(
        settings: Settings,
        path: PathBuf,
        record: Option<PathBuf>,
        replay: Option<PathBuf>,
    ) -> Result<Self> {
        let preferences = Preferences::load(&path)?;
        let (sender, receiver) = mpsc::channel();
        let views = preferences.saved_views.clone();
        let cli_settings = (settings != preferences.settings).then_some(settings.clone());
        let mut result = Self {
            preferences,
            settings: settings.clone(),
            name: "Untitled workspace".into(),
            active: 0,
            tabs: vec![],
            views,
            project_policy: ProjectPolicy {
                schema_version: 1,
                settings: None,
                presets: vec![],
            },
            notice: None,
            comparison: None,
            pending: 0,
            report_sections: crate::sharing::default_report_sections(),
            preview_policy: None,
            lookup_query: None,
            path,
            session_settings: cli_settings.clone(),
            cli_settings,
            next_id: 1,
            jobs: sender,
            results: receiver,
        };
        if let Some(path) = replay {
            result.open_recording(path, false, None);
        } else {
            result.add_target(settings.target.clone(), "Local runtime".into(), record)?;
        }
        Ok(result)
    }
    pub fn current(&self) -> &Tab {
        self.tabs
            .iter()
            .find(|t| t.document.id == self.active)
            .expect("active tab exists")
    }
    pub fn current_mut(&mut self) -> &mut Tab {
        self.tabs
            .iter_mut()
            .find(|t| t.document.id == self.active)
            .expect("active tab exists")
    }
    pub fn persist(&self) -> Result<()> {
        self.preferences.save(&self.path)
    }
    pub fn effective_preferences(&self) -> Result<Preferences> {
        let mut prefs = self
            .project_policy
            .apply(&self.preferences, self.session_settings.as_ref())?;
        prefs.settings = self.settings.clone();
        Ok(prefs)
    }
    fn apply_settings(&mut self, settings: Settings) -> Result<()> {
        settings.retention()?;
        settings.lint_thresholds.validate()?;
        self.settings = settings.clone();
        for tab in &self.tabs {
            tab.worker.send(Command::Settings(settings.clone()));
            if tab.document.recording.is_some() {
                let mut state = tab.worker.state.write().unwrap();
                let mut snapshot = (*state.snapshot).clone();
                for task in &mut snapshot.tasks {
                    task.lint_with(&settings.warnings, &settings.lint_thresholds);
                }
                state.snapshot = Arc::new(snapshot);
            }
        }
        Ok(())
    }

    fn add_target(&mut self, target: String, name: String, record: Option<PathBuf>) -> Result<()> {
        crate::config::validate_target(&target)?;
        ensure!(!name.trim().is_empty(), "A target tab needs a name");
        let mut settings = self.settings.clone();
        settings.target = target.clone();
        let worker = Arc::new(Worker::start(settings, record, None)?);
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab {
            document: TabDocument {
                id,
                name,
                target: target.clone(),
                tables: self.preferences.tables.clone(),
                histogram_policy: self.preferences.histogram_policy.clone(),
                ..Default::default()
            },
            worker,
            history: History::default(),
            selection: None,
            recording: None,
            playback: None,
            investigation: None,
            window_ns: None,
            policy: self.preferences.histogram_policy.clone(),
            last_session: 0,
            seek_generation: 0,
            seeking: false,
            seek_request: None,
        });
        self.active = id;
        self.current()
            .worker
            .send(Command::HistogramPolicy(self.current().policy.clone()));
        self.preferences.remember(&target)?;
        self.persist()
    }
    fn open_recording(&mut self, path: PathBuf, recover: bool, restore: Option<InvestigationView>) {
        let tab = self.next_id;
        self.next_id += 1;
        let worker = Arc::new(Worker::offline(Snapshot::default(), &path));
        self.tabs.push(Tab {
            document: TabDocument {
                id: tab,
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                target: path.display().to_string(),
                recording: Some(path.clone()),
                filter: restore.as_ref().map_or(String::new(), |v| v.filter.clone()),
                tables: restore
                    .as_ref()
                    .map_or(self.preferences.tables.clone(), |v| v.tables.clone()),
                view: restore.clone(),
                histogram_policy: Default::default(),
                active_preset: None,
            },
            worker,
            history: History::default(),
            selection: None,
            recording: None,
            playback: None,
            investigation: None,
            window_ns: None,
            policy: HistogramPolicy::default(),
            last_session: 0,
            seek_generation: 0,
            seeking: false,
            seek_request: None,
        });
        self.active = tab;
        self.pending += 1;
        self.notice = Some(format!("Opening {}…", path.display()));
        let settings = self.settings.clone();
        let jobs = self.jobs.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Job> {
                let mut recording = Recording::open_with_recovery(&path, recover)?;
                let snapshot = if let Some(view) = &restore {
                    recording.seek(view.session, view.cursor, &settings)?
                } else {
                    recording.final_snapshot(&settings)?
                };
                let session = restore
                    .as_ref()
                    .map_or_else(|| recording.sessions.last().unwrap().index, |v| v.session);
                let history = recording.history(session, snapshot.now, &settings)?;
                let sidecar = Investigation::sidecar(&path);
                let mut investigation = if sidecar.exists() {
                    Investigation::load(&sidecar)?
                } else {
                    Investigation::new(path.clone())
                };
                let original = investigation.recording.clone();
                investigation.recording = path.clone();
                for view in &mut investigation.views {
                    if view.recording.as_ref() == Some(&original) {
                        view.recording = Some(path.clone());
                    }
                }
                Ok(Job::Open(Box::new(Opened {
                    tab,
                    recording,
                    snapshot,
                    investigation,
                    restore,
                    history,
                })))
            })();
            let job = result.unwrap_or_else(|error| Job::OpenFailed {
                tab,
                message: format!("Open {}: {error:#}", path.display()),
            });
            let _ = jobs.send(Ok(job));
        });
    }
    fn background(&mut self, work: impl FnOnce() -> Result<String> + Send + 'static) {
        self.pending += 1;
        let sender = self.jobs.clone();
        std::thread::spawn(move || {
            let _ = sender.send(work().map(Job::Finished));
        });
    }
    pub fn tick(&mut self) -> Result<()> {
        while let Ok(result) = self.results.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            match result {
                Ok(Job::Finished(message)) => self.notice = Some(message),
                Ok(Job::Compared(comparison)) => {
                    self.comparison = Some(*comparison);
                    self.notice = Some("Capture comparison complete".into());
                }
                Ok(Job::OpenFailed { tab, message }) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.document.id == tab) {
                        tab.worker.state.write().unwrap().error = Some(message.clone());
                    }
                    self.notice = Some(message);
                }
                Ok(Job::Seeked {
                    tab,
                    generation,
                    result,
                }) => {
                    if let Some(target) = self.tabs.iter_mut().find(|t| t.document.id == tab) {
                        target.seeking = false;
                        if generation == target.seek_generation {
                            match result {
                                Ok(result) => {
                                    let mut state = target.worker.state.write().unwrap();
                                    state.snapshot = Arc::new(result.snapshot);
                                    state.session = result.session as u64 + 1;
                                    state.error = None;
                                    target.history = result.history;
                                    target.recording = Some(result.recording);
                                }
                                Err(error) => {
                                    if let Some(p) = &mut target.playback {
                                        p.playing = false;
                                    }
                                    self.notice = Some(format!("Seek recording: {error:#}"));
                                }
                            }
                        }
                    }
                }
                Ok(Job::Open(opened)) => {
                    let Opened {
                        tab,
                        recording,
                        snapshot,
                        investigation,
                        restore,
                        history,
                    } = *opened;
                    if let Some(target) = self.tabs.iter_mut().find(|t| t.document.id == tab) {
                        let (session, cursor) = restore
                            .as_ref()
                            .map(|v| (v.session, v.cursor))
                            .unwrap_or_else(|| {
                                let s = recording.sessions.last().unwrap();
                                (s.index, s.end)
                            });
                        {
                            let mut live = target.worker.state.write().unwrap();
                            live.snapshot = Arc::new(snapshot);
                            live.session = session as u64 + 1;
                            live.error = None;
                        }
                        target.last_session = session as u64 + 1;
                        target.history = history;
                        target.playback = Some(Playback::new(session, cursor));
                        target.window_ns = restore
                            .as_ref()
                            .and_then(|v| v.range.map(|r| r[1].saturating_sub(r[0])));
                        let warning = recording.warnings.join("; ");
                        target.recording = Some(recording);
                        target.investigation = Some(investigation);
                        let mut missing_selection = false;
                        if let Some((kind, id)) = restore.and_then(|v| v.selected) {
                            let live = target.worker.read();
                            let exists = if kind == "task" {
                                live.snapshot.tasks.iter().any(|t| t.id == id)
                            } else {
                                missing_selection = true;
                                live.snapshot.resources.iter().any(|r| r.id == id)
                            };
                            if exists {
                                target.selection = Some(Selection {
                                    kind,
                                    id,
                                    session: live.session,
                                });
                            } else {
                                self.notice = Some(
                                    "The saved entity is not retained at this recording position"
                                        .into(),
                                );
                            }
                        }
                        if !warning.is_empty() {
                            self.notice = Some(warning);
                        } else if self.pending == 0 && !missing_selection {
                            self.notice = None;
                        }
                    }
                }
                Err(error) => self.notice = Some(format!("{error:#}")),
            }
        }
        for tab in &mut self.tabs {
            if let (Some(recording), Some(playback)) = (&mut tab.recording, &mut tab.playback) {
                let end = recording.sessions[playback.session].end;
                if !tab.seeking && tab.seek_request.is_none() && playback.tick(end) {
                    tab.seek_generation += 1;
                    tab.seek_request = Some((playback.session, playback.cursor));
                }
            }
            if !tab.seeking
                && let Some((session, cursor)) = tab.seek_request.take()
            {
                tab.seeking = true;
                self.pending += 1;
                let mut recording = tab.recording.as_ref().unwrap().fork();
                let settings = self.settings.clone();
                let history = tab.history.clone();
                let sender = self.jobs.clone();
                let id = tab.document.id;
                let generation = tab.seek_generation;
                std::thread::spawn(move || {
                    let result = (|| -> Result<Box<Seeked>> {
                        let snapshot = recording.seek(session, cursor, &settings)?;
                        let history =
                            recording.extend_history(history, session, cursor, &settings)?;
                        Ok(Box::new(Seeked {
                            recording,
                            snapshot,
                            history,
                            session,
                        }))
                    })();
                    let _ = sender.send(Ok(Job::Seeked {
                        tab: id,
                        generation,
                        result,
                    }));
                });
            }
            let live = tab.worker.read();
            if live.session != tab.last_session {
                tab.selection = None;
                if tab.last_session > 0 && tab.policy.mode == "selected" {
                    tab.policy.tasks.clear();
                    tab.document.histogram_policy.tasks.clear();
                }
                tab.last_session = live.session;
            }
            if tab
                .selection
                .as_ref()
                .is_some_and(|s| !match s.kind.as_str() {
                    "task" => live.snapshot.tasks.iter().any(|t| t.id == s.id),
                    "resource" => live.snapshot.resources.iter().any(|r| r.id == s.id),
                    "operation" => live.snapshot.async_ops.iter().any(|o| o.id == s.id),
                    _ => false,
                })
            {
                tab.selection = None;
                self.notice =
                    Some("The selected entity is no longer retained at this position".into());
            }
            if !live.paused && live.status == ConnectionStatus::Connected {
                tab.history.push(live.session, &live.snapshot);
            } else if tab.document.recording.is_none() {
                tab.history.mark_gap();
            }
        }
        Ok(())
    }
    fn select(&mut self, kind: String, id: u64) -> Result<()> {
        let tab = self.current_mut();
        let live = tab.worker.read();
        ensure!(
            match kind.as_str() {
                "task" => live.snapshot.tasks.iter().any(|t| t.id == id),
                "resource" => live.snapshot.resources.iter().any(|r| r.id == id),
                "operation" => live.snapshot.async_ops.iter().any(|o| o.id == id),
                _ => false,
            },
            "Selected entity is unavailable in this session"
        );
        tab.selection = Some(Selection {
            kind: kind.clone(),
            id,
            session: live.session,
        });
        tab.worker
            .send(Command::Inspect((kind == "task").then_some(id)));
        Ok(())
    }
    fn seek(&mut self, session: usize, cursor: u64) -> Result<()> {
        let tab = self.current_mut();
        let recording = tab.recording.as_ref().context("Open a recording first")?;
        let info = recording
            .sessions
            .get(session)
            .context("Session is unavailable")?;
        ensure!(
            (info.start..=info.end).contains(&cursor),
            "Cursor lies outside the selected session"
        );
        let playback = tab.playback.as_mut().unwrap();
        if playback.session != session {
            tab.selection = None;
        }
        playback.session = session;
        playback.seek(cursor);
        tab.seek_generation += 1;
        tab.seek_request = Some((session, cursor));
        Ok(())
    }
    fn annotations(&mut self) -> Result<&mut Investigation> {
        self.current_mut()
            .investigation
            .as_mut()
            .context("Open a recording to annotate it")
    }
    fn save_annotations(&self) -> Result<()> {
        let investigation = self
            .current()
            .investigation
            .as_ref()
            .context("Open a recording to annotate it")?;
        investigation.save(&Investigation::sidecar(&investigation.recording))
    }
    pub fn find_tasks(&self, text: &str) -> Vec<TaskRow> {
        let query = text.to_lowercase();
        let snapshot = self.current().worker.read().snapshot;
        let mut tasks: Vec<_> = snapshot
            .tasks
            .iter()
            .filter(|t| t.name.to_lowercase().contains(&query) || t.id.to_string() == text)
            .cloned()
            .collect();
        tasks.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        tasks.truncate(100);
        tasks
    }
    fn rename_view(&mut self, name: &str, new_name: &str, duplicate: bool) -> Result<()> {
        ensure!(!new_name.trim().is_empty(), "View needs a name");
        let views = self.view()?.views;
        ensure!(
            !views.iter().any(|v| v.name == new_name),
            "A view with that name already exists"
        );
        let mut view = views
            .into_iter()
            .find(|v| v.name == name)
            .context("View is unavailable")?;
        view.name = new_name.to_owned();
        if self.views.iter().any(|v| v.name == name) {
            if !duplicate {
                self.views.retain(|v| v.name != name);
            }
            self.views.push(view);
            self.preferences.saved_views = self.views.clone();
            self.persist()?;
        } else {
            let investigation = self.annotations()?;
            if !duplicate {
                investigation.views.retain(|v| v.name != name);
            }
            investigation.views.push(view);
            self.save_annotations()?;
        }
        Ok(())
    }
    pub fn dispatch(&mut self, action: Action) -> Result<()> {
        if !matches!(&action, Action::Tick) {
            self.notice = None;
        }
        let duplicate_view = matches!(&action, Action::DuplicateView { .. });
        match action {
            Action::Tick => self.tick()?,
            Action::Lookup { text } => self.lookup_query = Some(text),
            Action::SelectInSession { kind, id, session } => {
                ensure!(
                    session == self.current().worker.read().session,
                    "The runtime session changed; find the entity again"
                );
                self.select(kind, id)?;
            }
            Action::Preview { policy } => self.preview_policy = Some(policy),
            Action::ReportSections { sections } => {
                ensure!(
                    !sections.is_empty()
                        && sections
                            .iter()
                            .all(|s| crate::sharing::default_report_sections().contains(s)),
                    "Choose known report sections"
                );
                self.report_sections = sections;
            }
            Action::AddTarget { target, name } => self.add_target(target, name, None)?,
            Action::SelectTab { id } => {
                ensure!(
                    self.tabs.iter().any(|t| t.document.id == id),
                    "Tab is unavailable"
                );
                self.active = id;
            }
            Action::CloseTab { id } => {
                ensure!(self.tabs.len() > 1, "Keep at least one tab open");
                if let Some(t) = self.tabs.iter().find(|t| t.document.id == id) {
                    t.worker.send(Command::StopRecording);
                }
                if let Some(tab) = self.tabs.iter().find(|t| t.document.id == id) {
                    tab.worker.shutdown();
                }
                self.tabs.retain(|t| t.document.id != id);
                if self.active == id {
                    self.active = self.tabs[0].document.id;
                }
            }
            Action::MoveTab { id, index } => {
                let old = self
                    .tabs
                    .iter()
                    .position(|t| t.document.id == id)
                    .context("Tab is unavailable")?;
                let tab = self.tabs.remove(old);
                self.tabs.insert(index.min(self.tabs.len()), tab);
            }
            Action::Connect { target } => {
                crate::config::validate_target(&target)?;
                ensure!(
                    self.current().document.recording.is_none(),
                    "Open a live target tab to connect"
                );
                self.current_mut().document.target = target.clone();
                self.current().worker.send(Command::Connect(target.clone()));
                self.preferences.remember(&target)?;
                self.persist()?;
            }
            Action::Disconnect => self.current().worker.send(Command::Disconnect),
            Action::Pause => {
                let worker = &self.current().worker;
                let live = worker.read();
                ensure!(
                    live.status == ConnectionStatus::Connected,
                    "Target is not connected"
                );
                worker.send(Command::Pause(!live.paused));
                self.notice = Some("Pause/resume changes the subscriber server and affects every observer of this endpoint, including duplicate target tabs".into());
            }
            Action::Filter { expression } => {
                Query::parse(&expression)?;
                self.current_mut().document.filter = expression;
                self.current_mut().selection = None;
            }
            Action::Select { kind, id } => self.select(kind, id)?,
            Action::ClearSelection => self.current_mut().selection = None,
            Action::SaveProfile { name, target } => {
                self.preferences.set_profile(name, target)?;
                self.persist()?;
            }
            Action::DeleteProfile { name } => {
                self.preferences.profiles.retain(|p| p.name != name);
                self.persist()?;
            }
            Action::RenameProfile { name, new_name } => {
                ensure!(
                    !self.preferences.profiles.iter().any(|p| p.name == new_name),
                    "A profile already has this name"
                );
                let profile = self
                    .preferences
                    .profiles
                    .iter_mut()
                    .find(|p| p.name == name)
                    .context("Profile is unavailable")?;
                ensure!(!new_name.trim().is_empty(), "A profile needs a name");
                profile.name = new_name;
                self.persist()?;
            }
            Action::ApplyProfile { name } => {
                let p = self
                    .preferences
                    .profiles
                    .iter()
                    .find(|p| p.name == name)
                    .context("Profile is unavailable")?
                    .clone();
                self.dispatch(Action::Connect { target: p.target })?;
            }
            Action::SavePreset { name } => {
                ensure!(
                    !self.project_policy.presets.iter().any(|p| p.name == name),
                    "This name belongs to a project preset; update the project preset or choose another name"
                );
                self.preferences
                    .set_preset(name.clone(), self.current().document.filter.clone())?;
                self.current_mut().document.active_preset = Some(name);
                self.persist()?;
            }
            Action::DeletePreset { name } => {
                self.preferences.filter_presets.retain(|p| p.name != name);
                self.persist()?;
            }
            Action::SaveProjectPreset { name } => {
                ensure!(!name.trim().is_empty(), "Preset needs a name");
                let expression = self.current().document.filter.clone();
                Query::parse(&expression)?;
                self.project_policy.presets.retain(|p| p.name != name);
                self.project_policy.presets.push(preferences::FilterPreset {
                    name: name.clone(),
                    expression,
                });
                self.current_mut().document.active_preset = Some(name);
            }
            Action::DeleteProjectPreset { name } => {
                self.project_policy.presets.retain(|p| p.name != name)
            }
            Action::ApplyPreset { name } => {
                let expression = self
                    .effective_preferences()?
                    .filter_presets
                    .iter()
                    .find(|p| p.name == name)
                    .context("Preset is unavailable")?
                    .expression
                    .clone();
                self.dispatch(Action::Filter { expression })?;
                self.current_mut().document.active_preset = Some(name);
            }
            Action::RenamePreset {
                name,
                new_name,
                project,
            } => {
                ensure!(!new_name.trim().is_empty(), "Preset needs a name");
                let presets = if project {
                    &mut self.project_policy.presets
                } else {
                    &mut self.preferences.filter_presets
                };
                ensure!(
                    !presets.iter().any(|p| p.name == new_name),
                    "A preset with that name already exists"
                );
                presets
                    .iter_mut()
                    .find(|p| p.name == name)
                    .context("Preset is unavailable")?
                    .name = new_name.clone();
                for tab in &mut self.tabs {
                    if tab.document.active_preset.as_ref() == Some(&name) {
                        tab.document.active_preset = Some(new_name.clone());
                    }
                }
                if !project {
                    self.persist()?;
                }
            }
            Action::Table { table, preferences } => {
                preferences.validate()?;
                ensure!(
                    ["tasks", "resources", "operations"].contains(&table.as_str()),
                    "Unknown table"
                );
                self.current_mut()
                    .document
                    .tables
                    .insert(table.clone(), preferences.clone());
                self.preferences.tables.insert(table, preferences);
                self.persist()?;
            }
            Action::Editor { preferences } => {
                preferences.validate()?;
                self.preferences.editor = preferences;
                self.persist()?;
            }
            Action::OpenSource => {
                let tab = self.current();
                let selection = tab
                    .selection
                    .as_ref()
                    .context("Select a task or resource")?;
                let live = tab.worker.read();
                ensure!(
                    selection.session == live.session,
                    "Selection belongs to an earlier session"
                );
                let location = if selection.kind == "task" {
                    live.snapshot
                        .tasks
                        .iter()
                        .find(|t| t.id == selection.id)
                        .map(|t| &t.location)
                } else {
                    live.snapshot
                        .resources
                        .iter()
                        .find(|r| r.id == selection.id)
                        .map(|r| &r.location)
                }
                .context("Source location is unavailable")?;
                self.preferences.editor.open(location)?;
            }
            Action::Thresholds { thresholds } => {
                thresholds.validate()?;
                let mut settings = self.settings.clone();
                settings.lint_thresholds = thresholds;
                self.dispatch(Action::Settings { settings })?;
            }
            Action::Settings { settings } => {
                self.apply_settings(settings.clone())?;
                self.session_settings = Some(settings.clone());
                if self.project_policy.settings.is_none() && self.project_policy.presets.is_empty()
                {
                    self.preferences.settings = settings;
                    self.persist()?;
                }
            }
            Action::Appearance { palette } => {
                let mut settings = self.settings.clone();
                settings.palette = palette;
                self.dispatch(Action::Settings { settings })?;
            }
            Action::LintEnabled { lint, enabled } => {
                let mut settings = self.settings.clone();
                if enabled {
                    settings.warnings.insert(lint);
                } else {
                    settings.warnings.remove(&lint);
                }
                self.dispatch(Action::Settings { settings })?;
            }
            Action::StartRecording { path } => {
                ensure!(
                    self.current().document.recording.is_none(),
                    "Cannot record an offline view"
                );
                self.current().worker.send(Command::StartRecording(path));
            }
            Action::StopRecording => self.current().worker.send(Command::StopRecording),
            Action::OpenRecording { path, recover } => self.open_recording(path, recover, None),
            Action::Session { index } => {
                let cursor = self
                    .current()
                    .recording
                    .as_ref()
                    .context("Open a recording")?
                    .sessions
                    .get(index)
                    .context("Session is unavailable")?
                    .start;
                self.seek(index, cursor)?;
            }
            Action::Seek { cursor } => {
                let session = self
                    .current()
                    .playback
                    .as_ref()
                    .context("Open a recording")?
                    .session;
                self.seek(session, cursor)?;
            }
            Action::Playback => {
                let tab = self.current_mut();
                let playback = tab.playback.as_mut().context("Open a recording")?;
                let info = &tab
                    .recording
                    .as_ref()
                    .context("Recording is unavailable")?
                    .sessions[playback.session];
                if !playback.playing && playback.cursor >= info.end {
                    playback.seek(info.start);
                    tab.seek_generation += 1;
                    tab.seek_request = Some((playback.session, playback.cursor));
                }
                playback.toggle();
            }
            Action::Speed { speed } => self
                .current_mut()
                .playback
                .as_mut()
                .context("Open a recording")?
                .speed(speed)?,
            Action::Window { nanoseconds } => {
                ensure!(nanoseconds != Some(0), "Analysis window must be positive");
                self.current_mut().window_ns = nanoseconds;
            }
            Action::HistogramPolicy { policy } => {
                policy.validate()?;
                self.current()
                    .worker
                    .send(Command::HistogramPolicy(policy.clone()));
                self.current_mut().document.histogram_policy = policy.clone();
                self.current_mut().policy = policy.clone();
                self.preferences.histogram_policy = policy;
                self.persist()?;
            }
            Action::Bookmark { name, note } => {
                ensure!(!name.trim().is_empty(), "Bookmark needs a name");
                let p = self
                    .current()
                    .playback
                    .as_ref()
                    .context("Open a recording")?;
                let (session, cursor) = (p.session, p.cursor);
                let selected = self.current().selection.clone();
                let task = selected.as_ref().filter(|s| s.kind == "task").map(|s| s.id);
                let resource = selected
                    .as_ref()
                    .filter(|s| s.kind == "resource")
                    .map(|s| s.id);
                self.annotations()?
                    .bookmark(name, note, session, cursor, task, resource);
                self.save_annotations()?;
            }
            Action::EditBookmark { id, name, note } => {
                ensure!(!name.trim().is_empty(), "Bookmark needs a name");
                let b = self
                    .annotations()?
                    .bookmarks
                    .iter_mut()
                    .find(|b| b.id == id)
                    .context("Bookmark is unavailable")?;
                b.name = name;
                b.note = note;
                self.save_annotations()?;
            }
            Action::DeleteBookmark { id } => {
                self.annotations()?.bookmarks.retain(|b| b.id != id);
                self.save_annotations()?;
            }
            Action::GoBookmark { id } => {
                let b = self
                    .annotations()?
                    .bookmarks
                    .iter()
                    .find(|b| b.id == id)
                    .context("Bookmark is unavailable")?
                    .clone();
                self.seek(b.session, b.cursor)?;
                if let Some(id) = b.task {
                    self.select("task".into(), id)?;
                }
                if let Some(id) = b.resource {
                    self.select("resource".into(), id)?;
                }
            }
            Action::SaveView { name } => {
                ensure!(!name.trim().is_empty(), "View needs a name");
                let tab = self.current();
                let playback = tab.playback.as_ref();
                let view = InvestigationView {
                    name: name.clone(),
                    recording: tab.document.recording.clone(),
                    session: playback.map_or(0, |p| p.session),
                    cursor: playback.map_or(tab.worker.read().snapshot.now, |p| p.cursor),
                    range: tab.window_ns.map(|ns| {
                        [
                            tab.worker.read().snapshot.now.saturating_sub(ns),
                            tab.worker.read().snapshot.now,
                        ]
                    }),
                    filter: tab.document.filter.clone(),
                    tables: tab.document.tables.clone(),
                    selected: tab.selection.as_ref().map(|s| (s.kind.clone(), s.id)),
                };
                self.views.retain(|v| v.name != name);
                self.views.push(view.clone());
                self.preferences.saved_views = self.views.clone();
                self.persist()?;
                if let Some(i) = &mut self.current_mut().investigation {
                    i.views.retain(|v| v.name != name);
                    i.views.push(view);
                    self.save_annotations()?;
                }
            }
            Action::LoadView { name } => {
                let view = self
                    .views
                    .iter()
                    .chain(
                        self.current()
                            .investigation
                            .iter()
                            .flat_map(|i| i.views.iter()),
                    )
                    .find(|v| v.name == name)
                    .context("Saved view is unavailable")?
                    .clone();
                if let Some(path) = &view.recording {
                    self.open_recording(path.clone(), false, Some(view));
                } else {
                    self.current_mut().document.filter = view.filter;
                    self.current_mut().document.tables = view.tables;
                    self.current_mut().selection = None;
                    self.notice=Some("Live view restored; select a current entity to avoid reusing an earlier session's ID".into());
                }
            }
            Action::DeleteView { name } => {
                self.views.retain(|v| v.name != name);
                self.preferences.saved_views = self.views.clone();
                self.persist()?;
                if let Some(i) = &mut self.current_mut().investigation {
                    i.views.retain(|v| v.name != name);
                    self.save_annotations()?;
                }
            }
            Action::RenameView { name, new_name } | Action::DuplicateView { name, new_name } => {
                // Variant identity is captured before this match so rename can remove
                // the old entry while duplicate keeps the original document.
                self.rename_view(&name, &new_name, duplicate_view)?;
            }
            Action::SaveWorkspace { path, name } => {
                ensure!(!name.trim().is_empty(), "Workspace needs a name");
                self.name = name;
                save_json(
                    &path,
                    &WorkspaceDocument {
                        schema_version: 1,
                        name: self.name.clone(),
                        active: self.active,
                        tabs: self
                            .tabs
                            .iter()
                            .map(|t| {
                                let mut document = t.document.clone();
                                if let Some(p) = &t.playback {
                                    document.view = Some(InvestigationView {
                                        name: document.name.clone(),
                                        recording: document.recording.clone(),
                                        session: p.session,
                                        cursor: p.cursor,
                                        range: t
                                            .window_ns
                                            .map(|ns| [p.cursor.saturating_sub(ns), p.cursor]),
                                        filter: document.filter.clone(),
                                        tables: document.tables.clone(),
                                        selected: t
                                            .selection
                                            .as_ref()
                                            .map(|s| (s.kind.clone(), s.id)),
                                    });
                                }
                                document
                            })
                            .collect(),
                        policy: ProjectPolicy {
                            schema_version: 1,
                            settings: Some(self.settings.clone()),
                            presets: self.effective_preferences()?.filter_presets,
                        },
                        views: self.views.clone(),
                    },
                )?;
            }
            Action::OpenWorkspace { path } => {
                ensure!(
                    self.pending == 0,
                    "Wait for current file operations to finish before switching workspaces"
                );
                let document: WorkspaceDocument = load_json(&path)?;
                ensure!(
                    document.schema_version == 1 && !document.tabs.is_empty(),
                    "Unsupported or empty workspace"
                );
                let effective = document
                    .policy
                    .apply(&self.preferences, self.cli_settings.as_ref())?;
                let mut ids = std::collections::BTreeSet::new();
                for tab in &document.tabs {
                    ensure!(ids.insert(tab.id), "Workspace tab IDs must be unique");
                }
                for t in &document.tabs {
                    Query::parse(&t.filter)?;
                    for p in t.tables.values() {
                        p.validate()?;
                    }
                    if t.recording.is_none() {
                        crate::config::validate_target(&t.target)?;
                    }
                }
                self.shutdown();
                self.tabs.clear();
                self.views = document.views;
                self.name = document.name;
                self.project_policy = document.policy;
                self.settings = effective.settings;
                self.session_settings = self.cli_settings.clone();
                let wanted = document.active;
                let mut active = None;
                for t in document.tabs {
                    let old_id = t.id;
                    if let Some(path) = t.recording {
                        let restoring = t.view.is_some();
                        self.open_recording(path, false, t.view);
                        if !restoring {
                            let tab = self.current_mut();
                            tab.document.filter = t.filter;
                            tab.document.tables = t.tables;
                        }
                    } else {
                        self.add_target(t.target, t.name, None)?;
                        let tab = self.current_mut();
                        tab.document.filter = t.filter;
                        tab.document.tables = t.tables;
                        tab.policy = t.histogram_policy.clone();
                        if tab.policy.mode == "selected" {
                            tab.policy.tasks.clear();
                        }
                        tab.worker
                            .send(Command::HistogramPolicy(tab.policy.clone()));
                    }
                    if old_id == wanted {
                        active = Some(self.active);
                    }
                }
                if let Some(active) = active {
                    self.active = active;
                }
                if self.tabs.is_empty() {
                    self.add_target(self.settings.target.clone(), "Local runtime".into(), None)?;
                }
            }
            Action::NewWorkspace { name } => {
                ensure!(
                    self.pending == 0,
                    "Wait for current file operations to finish before switching workspaces"
                );
                ensure!(!name.trim().is_empty(), "Workspace needs a name");
                self.shutdown();
                self.tabs.clear();
                self.views.clear();
                self.name = name;
                self.project_policy = ProjectPolicy {
                    schema_version: 1,
                    settings: None,
                    presets: vec![],
                };
                self.session_settings = self.cli_settings.clone();
                self.settings = self
                    .cli_settings
                    .clone()
                    .unwrap_or_else(|| self.preferences.settings.clone());
                self.add_target(self.settings.target.clone(), "Local runtime".into(), None)?;
            }
            Action::LoadPolicy { path } => {
                let policy = ProjectPolicy::load(&path)?;
                let effective = policy.apply(&self.preferences, self.session_settings.as_ref())?;
                self.apply_settings(effective.settings)?;
                self.project_policy = policy;
            }
            Action::SavePolicy { path } => {
                save_json(
                    &path,
                    &ProjectPolicy {
                        schema_version: 1,
                        settings: Some(self.settings.clone()),
                        presets: self.effective_preferences()?.filter_presets,
                    },
                )?;
            }
            Action::Compare {
                path,
                session,
                policy,
            } => {
                let current = self.current();
                let source = current
                    .document
                    .recording
                    .clone()
                    .context("Open a recording before comparing captures")?;
                let selected = current
                    .playback
                    .as_ref()
                    .context("Recording is still loading")?
                    .session;
                let duration = current.window_ns;
                let cursor = current.playback.as_ref().unwrap().cursor;
                let settings = self.settings.clone();
                let sender = self.jobs.clone();
                self.pending += 1;
                self.notice = Some("Comparing captures…".into());
                std::thread::spawn(move || {
                    let result = (|| -> Result<Job> {
                        let range = if let Some(duration) = duration {
                            let baseline = Recording::open(&path)?;
                            let info = baseline
                                .sessions
                                .get(session)
                                .context("Baseline session is unavailable")?;
                            ensure!(
                                info.end.saturating_sub(info.start) >= duration,
                                "Baseline session is shorter than the selected window"
                            );
                            Some([info.end - duration, info.end])
                        } else {
                            None
                        };
                        let (a, sa) =
                            crate::automation::capture_range(&path, session, &settings, range)?;
                        let (b, sb) = crate::automation::capture_range(
                            &source,
                            selected,
                            &settings,
                            duration.map(|ns| [cursor.saturating_sub(ns), cursor]),
                        )?;
                        Ok(Job::Compared(Box::new(analysis::compare(
                            &a,
                            &b,
                            [sa, sb],
                            &policy,
                        )?)))
                    })();
                    let _ = sender.send(result);
                });
            }
            Action::Export {
                path,
                format,
                table,
                redaction,
            } => {
                let tab = self.current();
                if let Some(source) = &tab.document.recording {
                    crate::sharing::preserve_source(source, &path)?;
                }
                let live = tab.worker.read();
                let snapshot = Query::parse(&tab.document.filter)?.filter(&live.snapshot);
                let mut document = ExportDocument::new(live.session, Some(live.target), snapshot);
                document.sections = self.report_sections.clone();
                document.range = tab
                    .window_ns
                    .map(|ns| [live.snapshot.now.saturating_sub(ns), live.snapshot.now]);
                document.notes.push("Table and inspector counters are lifetime values at the cursor. The selected range scopes trend charts and window summaries; it does not change lifetime counters.".into());
                let document = redaction.document(&document);
                let points = tab
                    .history
                    .points()
                    .into_iter()
                    .filter(|p| {
                        p.now <= live.snapshot.now && document.range.is_none_or(|r| p.now >= r[0])
                    })
                    .collect::<Vec<_>>();
                let comparison = if redaction.fields.is_empty()
                    && !redaction.source_paths
                    && !redaction.endpoints
                {
                    self.comparison.clone()
                } else {
                    None
                };
                self.background(move || {
                    match format.as_str() {
                        "json" => document.json(&path)?,
                        "csv" => document.csv(&path, &table)?,
                        "html" => document.report(&path, &points, comparison.as_ref())?,
                        "svg" => preferences::atomic_write(
                            &path,
                            crate::sharing::chart_svg(&points, &table)?.as_bytes(),
                        )?,
                        _ => anyhow::bail!("Unknown export format"),
                    };
                    Ok(format!("Saved {}", path.display()))
                });
            }
            Action::RedactRecording { path, policy } => {
                let source = self
                    .current()
                    .document
                    .recording
                    .clone()
                    .context("Open a recording")?;
                self.background(move || {
                    policy.recording_copy(&source, &path)?;
                    Ok(format!("Saved redacted copy {}", path.display()))
                });
            }
        }
        Ok(())
    }
    pub fn view(&self) -> Result<View> {
        let tab = self.current();
        let live = tab.worker.read();
        let query = Query::parse(&tab.document.filter)?;
        let snapshot = query.filter(&live.snapshot);
        let selected = tab
            .selection
            .as_ref()
            .filter(|s| s.session == live.session)
            .cloned();
        let focus = selected.as_ref().map(|s| format!("{}:{}", s.kind, s.id));
        let mut inspection = (*live.snapshot).clone();
        inspection.tasks.retain(|t| {
            selected
                .as_ref()
                .is_some_and(|s| s.kind == "task" && s.id == t.id)
        });
        inspection.resources.retain(|r| {
            selected
                .as_ref()
                .is_some_and(|s| s.kind == "resource" && s.id == r.id)
        });
        inspection.async_ops.retain(|o| {
            selected.as_ref().is_some_and(|s| {
                (s.kind == "operation" && s.id == o.id)
                    || (s.kind == "resource" && s.id == o.resource_id)
            })
        });
        inspection
            .details
            .retain(|id, _| inspection.tasks.iter().any(|t| t.id == *id));
        let export_preview = self.preview_policy.as_ref().map(|policy| {
            let mut doc =
                ExportDocument::new(live.session, Some(live.target.clone()), snapshot.clone());
            doc.sections = self.report_sections.clone();
            policy.document(&doc)
        });
        let evidence = snapshot
            .tasks
            .iter()
            .filter(|t| {
                selected
                    .as_ref()
                    .is_none_or(|s| s.kind != "task" || s.id == t.id)
            })
            .flat_map(|t| analysis::evidence(t, &self.settings))
            .collect();
        let mut views = self.views.clone();
        if let Some(i) = &tab.investigation {
            for v in &i.views {
                if !views.iter().any(|old| old.name == v.name) {
                    views.push(v.clone());
                }
            }
        }
        Ok(View {
            name: self.name.clone(),
            inspection,
            lookup_tasks: self
                .lookup_query
                .as_ref()
                .map_or(vec![], |q| self.find_tasks(q)),
            export_preview,
            report_sections: self.report_sections.clone(),
            histogram_policy: tab.policy.clone(),
            histogram_notice: live.histogram_notice.clone(),
            project_presets: self
                .project_policy
                .presets
                .iter()
                .map(|p| p.name.clone())
                .collect(),
            active: self.active,
            tabs: self
                .tabs
                .iter()
                .map(|t| {
                    let state = t.worker.read();
                    TabSummary {
                        id: t.document.id,
                        name: t.document.name.clone(),
                        target: t.document.target.clone(),
                        status: state.status,
                        recording: state.recording,
                    }
                })
                .collect(),
            status: live.status,
            paused: live.paused,
            target: live.target,
            recording: live.recording,
            session: live.session,
            unfiltered_counts: [
                live.snapshot.tasks.len(),
                live.snapshot.resources.len(),
                live.snapshot.async_ops.len(),
            ],
            query: tab.document.filter.clone(),
            selection: selected,
            trends: tab
                .history
                .points_for(
                    tab.selection
                        .as_ref()
                        .filter(|s| s.kind == "task")
                        .map(|s| s.id),
                )
                .into_iter()
                .filter(|p| {
                    p.now <= live.snapshot.now
                        && tab
                            .window_ns
                            .is_none_or(|ns| p.now >= live.snapshot.now.saturating_sub(ns))
                })
                .collect(),
            window: tab.window_ns.map(|ns| {
                tab.history
                    .window(&live.snapshot, live.snapshot.now.saturating_sub(ns))
            }),
            graph: analysis::contention_graph(&snapshot, focus.as_deref(), 300),
            evidence,
            sessions: tab
                .recording
                .as_ref()
                .map_or(vec![], |r| r.sessions.clone()),
            cursor: tab.playback.as_ref().map(|p| p.cursor),
            playing: tab.playback.as_ref().is_some_and(|p| p.playing),
            speed: tab.playback.as_ref().map_or(1.0, |p| p.speed),
            bookmarks: tab
                .investigation
                .as_ref()
                .map_or(vec![], |i| i.bookmarks.clone()),
            views,
            preferences: self.effective_preferences()?,
            comparison: self.comparison.clone(),
            notice: self.notice.clone().or(live.error).or(live.notice),
            pending: self.pending,
            snapshot,
        })
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.shutdown();
    }
}
