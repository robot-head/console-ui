//! Seekable, session-isolated recordings and portable investigation documents.
use crate::{
    config::Settings,
    model::{Accumulator, Snapshot, timestamp},
    preferences::{FilterPreset, TablePreferences, load_json, save_json},
};
use anyhow::{Context, Result, bail, ensure};
use console_api::{instrument::Update, tasks::TaskDetails};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub target: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub index: usize,
    pub start: u64,
    pub end: u64,
    pub updates: u64,
    pub target: Option<String>,
    pub incomplete: bool,
}
#[derive(Clone, Copy)]
struct Frame {
    offset: u64,
    kind: u8,
    time: u64,
    session: usize,
}
struct Checkpoint {
    frame: usize,
    session: usize,
    time: u64,
    accumulator: Accumulator,
}
pub struct Recording {
    pub path: PathBuf,
    pub sessions: Vec<SessionInfo>,
    pub warnings: Vec<String>,
    frames: Vec<Frame>,
    checkpoints: VecDeque<Checkpoint>,
}
pub(crate) fn read_frame(reader: &mut (impl Read + Seek)) -> Result<Option<(u8, Vec<u8>)>> {
    let mut kind = [0];
    if reader.read(&mut kind)? == 0 {
        return Ok(None);
    }
    let mut length = [0; 4];
    reader
        .read_exact(&mut length)
        .context("Truncated recording frame length")?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length <= 16 * 1024 * 1024, "Recording frame exceeds 16 MiB");
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .context("Truncated recording frame payload")?;
    Ok(Some((kind[0], bytes)))
}
impl Recording {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_recovery(path, false)
    }
    pub fn open_with_recovery(path: &Path, recover: bool) -> Result<Self> {
        let mut reader = BufReader::new(File::open(path)?);
        let mut header = [0; 8];
        reader.read_exact(&mut header)?;
        ensure!(
            &header == crate::recording::MAGIC || &header == b"TCUI\x01\0\0\0",
            "Unsupported recording format/version"
        );
        let mut recording = Self {
            path: path.to_owned(),
            sessions: vec![],
            warnings: vec![],
            frames: vec![],
            checkpoints: VecDeque::new(),
        };
        let mut session = 0;
        let mut now = 0;
        let mut pending = true;
        loop {
            let offset = reader.stream_position()?;
            let frame = match read_frame(&mut reader) {
                Ok(frame) => frame,
                Err(error)
                    if recover
                        && error.chain().any(|e| {
                            e.downcast_ref::<std::io::Error>()
                                .is_some_and(|e| e.kind() == std::io::ErrorKind::UnexpectedEof)
                        }) =>
                {
                    recording
                        .warnings
                        .push(format!("Capture ends at an incomplete frame: {error:#}"));
                    if let Some(s) = recording.sessions.last_mut() {
                        s.incomplete = true;
                    }
                    break;
                }
                Err(error) => return Err(error),
            };
            let Some((kind, bytes)) = frame else {
                break;
            };
            if kind == 3 {
                ensure!(bytes.is_empty(), "Invalid reset frame");
                pending = true;
                now = 0;
                continue;
            }
            if pending {
                session = recording.sessions.len();
                recording.sessions.push(SessionInfo {
                    index: session,
                    start: 0,
                    end: 0,
                    updates: 0,
                    target: None,
                    incomplete: false,
                });
                pending = false;
            }
            let mut frame_time = now;
            match kind {
                1 => {
                    let update = Update::decode(bytes.as_slice())?;
                    if let Some(time) = update.now.as_ref().and_then(timestamp) {
                        ensure!(
                            now == 0 || time >= now,
                            "Recording time moved backwards within a session"
                        );
                        now = time;
                    }
                    frame_time = now;
                    let info = &mut recording.sessions[session];
                    if info.updates == 0 {
                        info.start = now;
                    }
                    info.end = info.end.max(now);
                    info.updates += 1;
                }
                2 => {
                    let details = TaskDetails::decode(bytes.as_slice())?;
                    frame_time = details.now.as_ref().and_then(timestamp).unwrap_or(now);
                    recording.sessions[session].end =
                        recording.sessions[session].end.max(frame_time);
                }
                4 => {
                    let metadata: SessionMetadata = serde_json::from_slice(&bytes)?;
                    recording.sessions[session].target = metadata.target;
                }
                _ => bail!("Unknown recording frame type {kind}"),
            }
            recording.frames.push(Frame {
                offset,
                kind,
                time: frame_time,
                session,
            });
            ensure!(
                recording.frames.len() <= 5_000_000,
                "Recording exceeds the supported frame index limit"
            );
        }
        Ok(recording)
    }
    pub fn seek(&mut self, session: usize, cursor: u64, settings: &Settings) -> Result<Snapshot> {
        let info = self
            .sessions
            .get(session)
            .context("Recording session is unavailable")?;
        ensure!(
            cursor >= info.start && cursor <= info.end,
            "Cursor lies outside the selected session"
        );
        let checkpoint = self
            .checkpoints
            .iter()
            .filter(|c| c.session == session && c.time <= cursor)
            .max_by_key(|c| c.frame);
        let (mut accumulator, start) = checkpoint
            .map(|c| (c.accumulator.clone(), c.frame + 1))
            .unwrap_or((Accumulator::default(), 0));
        let mut reader = BufReader::new(File::open(&self.path)?);
        let mut last: Option<(usize, u64)> = None;
        let mut prefix = true;
        let mut prefix_checkpoint = None;
        for (index, frame) in self
            .frames
            .iter()
            .enumerate()
            .skip(start)
            .filter(|(_, f)| f.session == session)
        {
            if frame.time > cursor {
                if prefix {
                    prefix_checkpoint =
                        last.map(|(frame, time)| (frame, time, accumulator.clone()));
                    prefix = false;
                }
                continue;
            }
            reader.seek(SeekFrom::Start(frame.offset))?;
            let (_, bytes) = read_frame(&mut reader)?.context("Recording changed while reading")?;
            match frame.kind {
                1 => accumulator.apply(Update::decode(bytes.as_slice())?),
                2 => accumulator.apply_details(TaskDetails::decode(bytes.as_slice())?),
                _ => {}
            }
            last = Some((
                index,
                last.map_or(frame.time, |(_, time)| time.max(frame.time)),
            ));
        }
        // Cache unpruned state so a later seek with different retention remains correct.
        let checkpoint = if prefix {
            last.map(|(frame, time)| (frame, time, accumulator.clone()))
        } else {
            prefix_checkpoint
        };
        if let Some((frame, time, accumulator)) = checkpoint
            && self.checkpoints.iter().all(|c| c.frame != frame)
        {
            self.checkpoints.push_back(Checkpoint {
                frame,
                session,
                time,
                accumulator,
            });
            while self.checkpoints.len() > 4 {
                self.checkpoints.pop_front();
            }
        }
        accumulator.prune(settings.retention()?);
        let mut result = accumulator.snapshot(&settings.warnings);
        for task in &mut result.tasks {
            task.lint_with(&settings.warnings, &settings.lint_thresholds);
        }
        Ok(result)
    }
    pub fn final_snapshot(&mut self, settings: &Settings) -> Result<Snapshot> {
        let session = self
            .sessions
            .last()
            .context("Recording contains no sessions")?;
        self.seek(session.index, session.end, settings)
    }
    pub fn snapshots(&mut self, session: usize, settings: &Settings) -> Result<Vec<Snapshot>> {
        let times: std::collections::BTreeSet<_> = self
            .frames
            .iter()
            .filter(|f| f.session == session && f.kind == 1)
            .map(|f| f.time)
            .collect();
        times
            .into_iter()
            .map(|t| self.seek(session, t, settings))
            .collect()
    }
    pub fn window(
        &mut self,
        session: usize,
        start: u64,
        end: u64,
        settings: &Settings,
    ) -> Result<Snapshot> {
        ensure!(
            end > start,
            "An analysis window must have positive duration"
        );
        let mut complete_settings = settings.clone();
        complete_settings.retain_for = "none".into();
        let settings = &complete_settings;
        let baseline = self.seek(session, start, settings)?;
        let current = self.seek(session, end, settings)?;
        let (interval, incomplete) = crate::analysis::interval(&baseline, &current);
        ensure!(
            !incomplete,
            "Recording interval contains missing telemetry or counter resets"
        );
        Ok(interval)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: u64,
    pub name: String,
    pub note: String,
    pub session: usize,
    pub cursor: u64,
    pub task: Option<u64>,
    pub resource: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvestigationView {
    pub name: String,
    pub recording: Option<PathBuf>,
    pub session: usize,
    pub cursor: u64,
    pub range: Option<[u64; 2]>,
    pub filter: String,
    pub tables: std::collections::BTreeMap<String, TablePreferences>,
    pub selected: Option<(String, u64)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Investigation {
    pub schema_version: u32,
    pub recording: PathBuf,
    pub bookmarks: Vec<Bookmark>,
    pub views: Vec<InvestigationView>,
    pub presets: Vec<FilterPreset>,
}
impl Investigation {
    pub fn new(recording: PathBuf) -> Self {
        Self {
            schema_version: 1,
            recording,
            bookmarks: vec![],
            views: vec![],
            presets: vec![],
        }
    }
    pub fn sidecar(path: &Path) -> PathBuf {
        path.with_extension("tcui.investigation.json")
    }
    pub fn load(path: &Path) -> Result<Self> {
        let result: Self = load_json(path)?;
        ensure!(
            result.schema_version == 1,
            "Unsupported investigation version"
        );
        for view in &result.views {
            crate::query::Query::parse(&view.filter)?;
        }
        Ok(result)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "Unsupported investigation version"
        );
        save_json(path, self)
    }
    pub fn bookmark(
        &mut self,
        name: String,
        note: String,
        session: usize,
        cursor: u64,
        task: Option<u64>,
        resource: Option<u64>,
    ) -> u64 {
        let id = self.bookmarks.iter().map(|b| b.id).max().unwrap_or(0) + 1;
        self.bookmarks.push(Bookmark {
            id,
            name,
            note,
            session,
            cursor,
            task,
            resource,
        });
        id
    }
}

pub struct Playback {
    pub session: usize,
    pub cursor: u64,
    pub playing: bool,
    pub speed: f64,
    last: Instant,
}
impl Playback {
    pub fn new(session: usize, cursor: u64) -> Self {
        Self {
            session,
            cursor,
            playing: false,
            speed: 1.0,
            last: Instant::now(),
        }
    }
    pub fn speed(&mut self, speed: f64) -> Result<()> {
        ensure!(
            speed.is_finite() && (0.1..=16.0).contains(&speed),
            "Playback speed must be 0.1–16x"
        );
        self.speed = speed;
        self.last = Instant::now();
        Ok(())
    }
    pub fn toggle(&mut self) {
        self.playing = !self.playing;
        self.last = Instant::now();
    }
    pub fn seek(&mut self, cursor: u64) {
        self.cursor = cursor;
        self.last = Instant::now();
    }
    pub fn advance(&mut self, elapsed: Duration, end: u64) {
        if self.playing {
            self.cursor = self
                .cursor
                .saturating_add(
                    (elapsed.as_nanos() as f64 * self.speed).min(u64::MAX as f64) as u64,
                )
                .min(end);
            if self.cursor == end {
                self.playing = false;
            }
        }
    }
    pub fn tick(&mut self, end: u64) -> bool {
        let now = Instant::now();
        let old = self.cursor;
        self.advance(now.duration_since(self.last), end);
        self.last = now;
        old != self.cursor
    }
}
