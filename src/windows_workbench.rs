//! Native WinUI workbench for the shared product controller.
use crate::{
    analysis::RegressionPolicy,
    config::{LintThresholds, Palette, Settings},
    preferences::{self, Column, PathMapping, TablePreferences},
    sharing::RedactionPolicy,
    telemetry::HistogramPolicy,
    workspace::{Action, Selection, Workspace},
};
use anyhow::{Context, Result};
use std::{
    cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc, sync::mpsc, time::Duration,
};
use windows_reactor::*;

#[derive(Clone)]
pub struct Input(Rc<RefCell<Option<Workspace>>>);
impl PartialEq for Input {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Monitor,
    Connections,
    Filters,
    Tables,
    Source,
    Analysis,
    Recording,
    Compare,
    Export,
    Workspace,
}
#[derive(Clone)]
pub enum Message {
    Tick,
    Panel(u8),
    Action(Action),
    Text(&'static str, Rc<str>),
    Flag(&'static str, bool),
    Apply(&'static str),
    Pick(Action),
    Lookup,
    FocusLookup,
    EditBookmark(u64),
    Jump(u64, Selection),
    Timeline { tab: u64, session: u64, cursor: u64 },
    NativeSelection(Option<Selection>),
    Column { key: String, change: ColumnChange },
}
#[derive(Clone)]
pub enum ColumnChange {
    Width(f64),
    Visible(bool),
    Move(i32),
}
pub struct Workbench {
    app: Workspace,
    panel: Panel,
    fields: BTreeMap<&'static str, Rc<str>>,
    flags: BTreeMap<&'static str, bool>,
    timer: ComponentTimer,
    navigation: u64,
    dialog: bool,
    lookup: ElementRef<TextBox>,
    focus_timer: Option<ComponentTimer>,
    tx: mpsc::Sender<(u64, Option<Action>)>,
    rx: mpsc::Receiver<(u64, Option<Action>)>,
}
impl Component for Workbench {
    type Input = Input;
    type Message = Message;
    fn create(input: &Input, context: &ComponentContext<Self>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            app: input
                .0
                .borrow_mut()
                .take()
                .expect("workbench initialized once"),
            panel: Panel::Monitor,
            fields: BTreeMap::new(),
            flags: BTreeMap::new(),
            timer: context.set_local_timeout(Duration::from_millis(250), || Message::Tick),
            navigation: 0,
            dialog: false,
            lookup: ElementRef::new(),
            focus_timer: None,
            tx,
            rx,
        }
    }
    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        let result = (|| -> Result<()> {
            match message {
                Message::Tick => {
                    self.timer =
                        context.set_local_timeout(Duration::from_millis(250), || Message::Tick);
                    self.app.tick()?;
                    while let Ok((tab, action)) = self.rx.try_recv() {
                        self.dialog = false;
                        if let Some(action) = action {
                            if !self.app.tabs.iter().any(|t| t.document.id == tab) {
                                continue;
                            }
                            self.app.dispatch(Action::SelectTab { id: tab })?;
                            self.app.dispatch(action)?;
                        }
                    }
                }
                Message::Lookup => {
                    self.panel = Panel::Filters;
                    self.focus_timer = Some(
                        context
                            .set_local_timeout(Duration::from_millis(50), || Message::FocusLookup),
                    );
                }
                Message::FocusLookup => {
                    let _ = self.lookup.request_focus();
                }
                Message::EditBookmark(id) => {
                    if let Some(bookmark) = self
                        .app
                        .current()
                        .investigation
                        .as_ref()
                        .and_then(|i| i.bookmarks.iter().find(|b| b.id == id))
                    {
                        self.fields.insert("bookmark_id", Rc::from(id.to_string()));
                        self.fields
                            .insert("bookmark_name", Rc::from(bookmark.name.as_str()));
                        self.fields
                            .insert("bookmark_note", Rc::from(bookmark.note.as_str()));
                    }
                }
                Message::Panel(index) => {
                    self.panel = match index {
                        0 => Panel::Monitor,
                        1 => Panel::Connections,
                        2 => Panel::Filters,
                        3 => Panel::Tables,
                        4 => Panel::Source,
                        5 => Panel::Analysis,
                        6 => Panel::Recording,
                        7 => Panel::Compare,
                        8 => Panel::Export,
                        _ => Panel::Workspace,
                    };
                    if self.panel == Panel::Monitor {
                        self.navigation += 1;
                    }
                }
                Message::Text(key, value) => {
                    self.fields.insert(key, value);
                }
                Message::Flag(key, value) => {
                    self.flags.insert(key, value);
                }
                Message::Action(action) => self.app.dispatch(action)?,
                Message::Apply(form) => self.apply(form)?,
                Message::Pick(action) => {
                    let action = if matches!(&action, Action::Compare { .. }) {
                        self.comparison_request()?
                    } else {
                        action
                    };
                    if self.dialog {
                        return Ok(());
                    }
                    self.dialog = true;
                    let sender = self.tx.clone();
                    let tab = self.app.active;
                    std::thread::spawn(move || {
                        let selected = pick(action);
                        let _ = sender.send((tab, selected));
                    });
                }
                Message::Timeline {
                    tab,
                    session,
                    cursor,
                } => {
                    anyhow::ensure!(
                        self.app.active == tab
                            && self.app.current().worker.read().session == session,
                        "The active investigation changed"
                    );
                    self.app.dispatch(Action::Seek { cursor })?;
                }
                Message::Jump(tab, selection) => {
                    anyhow::ensure!(self.app.active == tab, "The active investigation changed");
                    anyhow::ensure!(
                        selection.session == self.app.current().worker.read().session,
                        "The runtime session changed; find the task again"
                    );
                    self.app.dispatch(Action::Select {
                        kind: selection.kind,
                        id: selection.id,
                    })?;
                    self.navigation += 1;
                    self.panel = Panel::Monitor;
                }
                Message::NativeSelection(selection) => {
                    if let Some(selection) = selection {
                        if selection.session == self.app.current().worker.read().session {
                            self.app.dispatch(Action::Select {
                                kind: selection.kind,
                                id: selection.id,
                            })?;
                        }
                    } else {
                        self.app.current_mut().selection = None;
                    }
                }
                Message::Column { key, change } => {
                    let table = self.value("table", "tasks");
                    let mut prefs = self
                        .app
                        .current()
                        .document
                        .tables
                        .get(&table)
                        .cloned()
                        .unwrap_or_default();
                    if prefs.columns.is_empty() {
                        prefs.columns = preferences::default_columns(&table);
                    }
                    let index = prefs
                        .columns
                        .iter()
                        .position(|c| c.key == key)
                        .context("Column is unavailable")?;
                    match change {
                        ColumnChange::Width(width) => prefs.columns[index].width = width,
                        ColumnChange::Visible(visible) => prefs.columns[index].visible = visible,
                        ColumnChange::Move(delta) => {
                            let target = (index as i32 + delta)
                                .clamp(0, prefs.columns.len() as i32 - 1)
                                as usize;
                            let column = prefs.columns.remove(index);
                            prefs.columns.insert(target, column);
                        }
                    }
                    self.app.dispatch(Action::Table {
                        table,
                        preferences: prefs,
                    })?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.app.notice = Some(format!("{error:#}"));
        }
    }
    fn view(&self, _: &Input, context: &mut ViewContext<Self>) -> View {
        context.window_visuals(WindowVisuals::new().client_size(1540.0, 960.0).theme(
            match self.app.settings.palette {
                Palette::System => WindowTheme::System,
                Palette::Dark => WindowTheme::Dark,
                Palette::Light => WindowTheme::Light,
            },
        ));
        let tab = self.app.current();
        let live = tab.worker.read();
        let mut settings = self.app.settings.clone();
        settings.target = tab.document.target.clone();
        let content: View = if self.panel == Panel::Monitor {
            component::<crate::ui::Console>(
                format!("monitor-{}", tab.document.id),
                crate::ui::Launch {
                    settings,
                    worker: tab.worker.clone(),
                    query: tab.document.filter.clone(),
                    tables: tab.document.tables.clone(),
                    navigation: tab.selection.clone().map(|s| (self.navigation, s)),
                    on_selection: context.callback(Message::NativeSelection),
                    on_target: context
                        .callback(|target| Message::Action(Action::Connect { target })),
                    on_query: context
                        .callback(|expression| Message::Action(Action::Filter { expression })),
                    on_settings: context
                        .callback(|settings| Message::Action(Action::Settings { settings })),
                    on_table: context.callback(|(table, preferences)| {
                        Message::Action(Action::Table { table, preferences })
                    }),
                },
            )
            .into()
        } else {
            ScrollViewer::new()
                .content(Border::new().padding(24.0).content(match self.panel {
                    Panel::Connections => self.connections(context),
                    Panel::Filters => self.filters(context),
                    Panel::Tables => self.tables(context),
                    Panel::Source => self.source(context),
                    Panel::Analysis => self.analysis(context),
                    Panel::Recording => self.recording(context),
                    Panel::Compare => self.comparison(context),
                    Panel::Export => self.export(context),
                    _ => self.workspace(context),
                }))
                .into()
        };
        let tabs = ScrollViewer::new()
            .horizontal_scroll_bar_visibility(ScrollBarVisibility::Auto)
            .vertical_scroll_bar_visibility(ScrollBarVisibility::Disabled)
            .content(
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(6.0)
                    .children(
                        self.app
                            .tabs
                            .iter()
                            .flat_map(|tab| {
                                [
                                    button(
                                        &format!(
                                            "{}{}",
                                            tab.document.name,
                                            if tab.worker.read().recording.is_some() {
                                                "  REC"
                                            } else {
                                                ""
                                            }
                                        ),
                                        context,
                                        Message::Action(Action::SelectTab {
                                            id: tab.document.id,
                                        }),
                                    ),
                                    Button::new()
                                        .content("×")
                                        .automation_name(format!("Close {}", tab.document.name))
                                        .is_enabled(self.app.tabs.len() > 1)
                                        .on_click(context.message(Message::Action(
                                            Action::CloseTab {
                                                id: tab.document.id,
                                            },
                                        )))
                                        .into(),
                                ]
                            })
                            .collect::<Vec<View>>(),
                    ),
            );
        let menu = StackPanel::new().width(160.0).spacing(8.0).children(
            [
                "Monitor",
                "Connections",
                "Filters & lookup",
                "Tables",
                "Source editor",
                "Analysis",
                "Recording",
                "Compare",
                "Export & share",
                "Workspace",
            ]
            .into_iter()
            .enumerate()
            .map(|(i, name)| button(name, context, Message::Panel(i as u8)))
            .collect::<Vec<View>>(),
        );
        let mut top = vec![tabs.into()];
        if let Some(notice) = &self.app.notice {
            top.push(text(notice).into());
        }
        if self.app.pending > 0 || self.dialog {
            top.push(
                text(if self.dialog {
                    "Choose a file in the native dialog…"
                } else {
                    "Working on your recording or export…"
                })
                .into(),
            );
        }
        if let Some(path) = live.recording {
            top.push(text(&format!("Recording to {}", path.display())).into());
        }
        let sender = context.sender();
        let keys =
            Border::new().on_preview_key_down(RoutedCallback::new(move |event: KeyEventInfo| {
                if event.key == VirtualKey::K && event.modifiers.contains(InputModifiers::CONTROL) {
                    let _ = sender.send(Message::Lookup);
                    true
                } else {
                    false
                }
            }));
        let layout = Grid::new()
            .rows([GridLength::Auto, GridLength::STAR])
            .row_spacing(8.0)
            .children((
                StackPanel::new()
                    .margin(Thickness::xy(12.0, 4.0))
                    .spacing(6.0)
                    .children(top),
                Grid::new()
                    .grid_row(1)
                    .columns([GridLength::Auto, GridLength::STAR])
                    .children((
                        Border::new().padding(12.0).content(menu),
                        Border::new().grid_column(1).content(content),
                    )),
            ));
        context.window_frame("Console UI  /  Tokio diagnostics", keys.content(layout))
    }
}

impl Workbench {
    fn value(&self, key: &'static str, fallback: &str) -> String {
        self.fields
            .get(key)
            .map_or_else(|| fallback.into(), |v| v.to_string())
    }
    fn flag(&self, key: &'static str) -> bool {
        self.flags.get(key).copied().unwrap_or(false)
    }
    fn entry(
        &self,
        key: &'static str,
        label: &str,
        fallback: &str,
        context: &ViewContext<Self>,
    ) -> View {
        StackPanel::new()
            .spacing(4.0)
            .children((
                text(label),
                TextBox::new()
                    .text(self.value(key, fallback))
                    .automation_name(label)
                    .on_text_changed(context.callback(move |value| Message::Text(key, value))),
            ))
            .into()
    }
    fn check(&self, key: &'static str, label: &str, context: &ViewContext<Self>) -> View {
        CheckBox::new()
            .content(label)
            .is_checked(self.flag(key))
            .on_is_checked_changed(
                context.callback(move |value: Option<bool>| {
                    Message::Flag(key, value.unwrap_or(false))
                }),
            )
            .into()
    }
    fn apply(&mut self, form: &str) -> Result<()> {
        let action = match form {
            "profile" => Action::SaveProfile {
                name: self.value("profile_name", ""),
                target: self.value("endpoint", &self.app.current().document.target),
            },
            "add_tab" => Action::AddTarget {
                target: self.value("endpoint", &self.app.current().document.target),
                name: self.value("profile_name", "Runtime"),
            },
            "connect" => Action::Connect {
                target: self.value("endpoint", &self.app.current().document.target),
            },
            "filter" => Action::Filter {
                expression: self.value("query", &self.app.current().document.filter),
            },
            "report_sections" => Action::ReportSections {
                sections: self
                    .value("report_sections", "charts,tasks,resources,comparison")
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            },
            "rename_view" => Action::RenameView {
                name: self.value("view_original", ""),
                new_name: self.value("view_name", ""),
            },
            "duplicate_view" => Action::DuplicateView {
                name: self.value("view_original", ""),
                new_name: self.value("view_name", ""),
            },
            "rename_preset" => Action::RenamePreset {
                name: self.value("preset_name", ""),
                new_name: self.value("preset_new_name", ""),
                project: self
                    .app
                    .project_policy
                    .presets
                    .iter()
                    .any(|p| p.name == self.value("preset_name", "")),
            },
            "project_preset" => Action::SaveProjectPreset {
                name: self.value("preset_name", ""),
            },
            "rename_profile" => Action::RenameProfile {
                name: self.value("profile_name", ""),
                new_name: self.value("profile_new_name", ""),
            },
            "preset" => Action::SavePreset {
                name: self.value("preset_name", ""),
            },
            "table_reset" => Action::Table {
                table: self.value("table", "tasks"),
                preferences: TablePreferences::default(),
            },
            "editor" => {
                let mut prefs = self.app.preferences.editor.clone();
                prefs.program = self.value("editor_program", &prefs.program);
                prefs.arguments = shlex::split(&self.value(
                    "editor_arguments",
                    &shlex::try_join(prefs.arguments.iter().map(String::as_str))?,
                ))
                .context("Editor arguments contain an unclosed quote")?;
                Action::Editor { preferences: prefs }
            }
            "mapping" => {
                let mut prefs = self.app.preferences.editor.clone();
                prefs.mappings.push(PathMapping {
                    remote_root: self.value("remote_root", ""),
                    local_root: self.value("local_root", "").into(),
                });
                Action::Editor { preferences: prefs }
            }
            "thresholds" => Action::Thresholds {
                thresholds: LintThresholds {
                    self_wake_percent: self
                        .value(
                            "self_wake",
                            &self
                                .app
                                .settings
                                .lint_thresholds
                                .self_wake_percent
                                .to_string(),
                        )
                        .parse()?,
                    never_yielded_ns: humantime::parse_duration(
                        &self.value(
                            "yield_duration",
                            &humantime::format_duration(Duration::from_nanos(
                                self.app.settings.lint_thresholds.never_yielded_ns,
                            ))
                            .to_string(),
                        ),
                    )?
                    .as_nanos()
                    .try_into()?,
                    large_future_bytes: self
                        .value(
                            "future_bytes",
                            &self
                                .app
                                .settings
                                .lint_thresholds
                                .large_future_bytes
                                .to_string(),
                        )
                        .parse()?,
                },
            },
            "window" => Action::Window {
                nanoseconds: if self.value("window", "60s") == "none" {
                    None
                } else {
                    Some(
                        humantime::parse_duration(&self.value("window", "60s"))?
                            .as_nanos()
                            .try_into()?,
                    )
                },
            },
            "seek" => {
                let p = self
                    .app
                    .current()
                    .playback
                    .as_ref()
                    .context("Open a recording")?;
                let recording = self.app.current().recording.as_ref().unwrap();
                let offset =
                    humantime::parse_duration(&self.value("seek_offset", "0s"))?.as_nanos() as u64;
                Action::Seek {
                    cursor: recording.sessions[p.session].start.saturating_add(offset),
                }
            }
            "speed" => Action::Speed {
                speed: self.value("speed", "1").parse()?,
            },
            "histogram" => Action::HistogramPolicy {
                policy: HistogramPolicy {
                    mode: self.value("histogram_mode", "inspected"),
                    tasks: self
                        .value("histogram_tasks", "")
                        .split(',')
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| s.trim().parse())
                        .collect::<std::result::Result<_, _>>()?,
                    query: self.value("histogram_query", ""),
                    max_streams: self.value("histogram_limit", "4").parse()?,
                    rotate_seconds: self.value("histogram_rotation", "30").parse()?,
                    collect_without_recording: false,
                },
            },
            "bookmark" => Action::Bookmark {
                name: self.value("bookmark_name", ""),
                note: self.value("bookmark_note", ""),
            },
            "edit_bookmark" => Action::EditBookmark {
                id: self.value("bookmark_id", "").parse()?,
                name: self.value("bookmark_name", ""),
                note: self.value("bookmark_note", ""),
            },
            "save_view" => Action::SaveView {
                name: self.value("view_name", ""),
            },
            "new_workspace" => Action::NewWorkspace {
                name: self.value("workspace_name", "New workspace"),
            },
            _ => anyhow::bail!("Unknown action"),
        };
        self.app.dispatch(action)
    }
    fn connections(&self, c: &ViewContext<Self>) -> View {
        let mut controls = vec![
            heading("Connections"),
            self.entry("profile_name", "Profile or tab name", "", c),
            self.entry("profile_new_name", "New profile name", "", c),
            self.entry(
                "endpoint",
                "Telemetry endpoint",
                &self.app.current().document.target,
                c,
            ),
            row(vec![
                button("Connect", c, Message::Apply("connect")),
                button(
                    "Disconnect / cancel",
                    c,
                    Message::Action(Action::Disconnect),
                ),
                button("Open target in new tab", c, Message::Apply("add_tab")),
                button("Save profile", c, Message::Apply("profile")),
                button("Rename profile", c, Message::Apply("rename_profile")),
            ]),
        ];
        for p in &self.app.preferences.profiles {
            controls.push(row(vec![
                text(&p.name).into(),
                button(
                    "Connect",
                    c,
                    Message::Action(Action::ApplyProfile {
                        name: p.name.clone(),
                    }),
                ),
                button(
                    "Delete",
                    c,
                    Message::Action(Action::DeleteProfile {
                        name: p.name.clone(),
                    }),
                ),
            ]));
        }
        controls.push(heading("Recent targets"));
        for target in &self.app.preferences.recent_targets {
            controls.push(button(
                target,
                c,
                Message::Action(Action::Connect {
                    target: target.clone(),
                }),
            ));
        }
        stack(controls)
    }
    fn filters(&self, c: &ViewContext<Self>) -> View {
        let mut controls=vec![heading("Filters and task lookup"),text("Combine clauses with AND (spaces), use | for OR, ! for negation, and quotes for values with spaces.").into(),self.entry("query","Filter expression",&self.app.current().document.filter,c),row(vec![button("Apply filter",c,Message::Apply("filter")),button("Clear",c,Message::Action(Action::Filter{expression:String::new()}))]),text("Examples: busy>5ms !state:completed   |   warning:self-wakes   |   field.request.id:123").into(),self.entry("preset_name","Preset name","",c),button("Save current filter as preset",c,Message::Apply("preset"))];
        controls.push(self.entry("preset_new_name", "New preset name", "", c));
        controls.push(button("Rename preset", c, Message::Apply("rename_preset")));
        controls.push(button(
            "Save preset in this project",
            c,
            Message::Apply("project_preset"),
        ));
        let effective = self
            .app
            .effective_preferences()
            .unwrap_or_else(|_| self.app.preferences.clone());
        if let Some(active) = &self.app.current().document.active_preset {
            let modified = effective
                .filter_presets
                .iter()
                .find(|p| &p.name == active)
                .is_none_or(|p| p.expression != self.app.current().document.filter);
            controls.push(
                text(&format!(
                    "Active preset: {active}{}",
                    if modified { " (query modified)" } else { "" }
                ))
                .into(),
            );
        }
        for p in &effective.filter_presets {
            controls.push(row(vec![
                text(&p.name).into(),
                button(
                    "Apply",
                    c,
                    Message::Action(Action::ApplyPreset {
                        name: p.name.clone(),
                    }),
                ),
                button(
                    "Delete",
                    c,
                    Message::Action(
                        if self
                            .app
                            .project_policy
                            .presets
                            .iter()
                            .any(|x| x.name == p.name)
                        {
                            Action::DeleteProjectPreset {
                                name: p.name.clone(),
                            }
                        } else {
                            Action::DeletePreset {
                                name: p.name.clone(),
                            }
                        },
                    ),
                ),
            ]));
        }
        controls.push(
            TextBox::new()
                .text(self.value("lookup", ""))
                .placeholder_text("Find task by name or ID · Ctrl+K")
                .automation_name("Find task by name or ID")
                .element_ref(&self.lookup)
                .on_text_changed(c.callback(|value| Message::Text("lookup", value)))
                .into(),
        );
        let session = self.app.current().worker.read().session;
        for task in self.app.find_tasks(&self.value("lookup", "")) {
            controls.push(button(
                &format!(
                    "{}  {}  {:?}  {}",
                    task.id, task.name, task.state, task.location
                ),
                c,
                Message::Jump(
                    self.app.active,
                    Selection {
                        kind: "task".into(),
                        id: task.id,
                        session,
                    },
                ),
            ));
        }
        stack(controls)
    }
    fn tables(&self, c: &ViewContext<Self>) -> View {
        let table = self.value("table", "tasks");
        let prefs = self
            .app
            .current()
            .document
            .tables
            .get(&table)
            .cloned()
            .unwrap_or_default();
        let columns = if prefs.columns.is_empty() {
            preferences::default_columns(&table)
        } else {
            prefs.columns
        };
        let mut controls = vec![
            heading("Table columns"),
            row(["tasks", "resources", "operations"]
                .into_iter()
                .map(|s| button(s, c, Message::Text("table", Rc::from(s))))
                .collect()),
            button("Restore default columns", c, Message::Apply("table_reset")),
        ];
        for Column {
            key,
            width,
            visible,
        } in columns
        {
            let width_key = key.clone();
            let visible_key = key.clone();
            controls.push(row(vec![
                text(&key).width(140.0).into(),
                NumberBox::new()
                    .width(140.0)
                    .automation_name(format!("{table} {key} column width"))
                    .minimum(32.0)
                    .maximum(2000.0)
                    .value(Some(width))
                    .on_value_changed(c.callback(move |value: Option<f64>| Message::Column {
                        key: width_key.clone(),
                        change: ColumnChange::Width(value.unwrap_or(90.0)),
                    }))
                    .into(),
                CheckBox::new()
                    .content("Visible")
                    .automation_name(format!("{table} {key} column visible"))
                    .is_checked(visible)
                    .on_is_checked_changed(c.callback(move |value: Option<bool>| Message::Column {
                        key: visible_key.clone(),
                        change: ColumnChange::Visible(value.unwrap_or(false)),
                    }))
                    .into(),
                button(
                    "Move left",
                    c,
                    Message::Column {
                        key: key.clone(),
                        change: ColumnChange::Move(-1),
                    },
                ),
                button(
                    "Move right",
                    c,
                    Message::Column {
                        key,
                        change: ColumnChange::Move(1),
                    },
                ),
            ]));
        }
        stack(controls)
    }
    fn source(&self, c: &ViewContext<Self>) -> View {
        let editor = &self.app.preferences.editor;
        let mut controls = vec![
            heading("Source navigation"),
            button(
                "Open selected task or resource in editor",
                c,
                Message::Action(Action::OpenSource),
            ),
            self.entry("editor_program", "Editor executable", &editor.program, c),
            self.entry(
                "editor_arguments",
                "Arguments ({file}, {line}, {column}); quote arguments containing spaces",
                &shlex::try_join(editor.arguments.iter().map(String::as_str)).unwrap_or_default(),
                c,
            ),
            button("Save editor integration", c, Message::Apply("editor")),
            self.entry("remote_root", "Telemetry source root", "", c),
            self.entry("local_root", "Local source root", "", c),
            button("Add root mapping", c, Message::Apply("mapping")),
        ];
        for mapping in &editor.mappings {
            let mut next = editor.clone();
            next.mappings.retain(|m| m != mapping);
            controls.push(row(vec![
                text(&format!(
                    "{} → {}",
                    mapping.remote_root,
                    mapping.local_root.display()
                ))
                .into(),
                button(
                    "Remove mapping",
                    c,
                    Message::Action(Action::Editor { preferences: next }),
                ),
            ]));
        }
        if let Some(selection) = &self.app.current().selection {
            let live = self.app.current().worker.read();
            let location = live
                .snapshot
                .tasks
                .iter()
                .find(|t| selection.kind == "task" && t.id == selection.id)
                .map(|t| &t.location)
                .or_else(|| {
                    live.snapshot
                        .resources
                        .iter()
                        .find(|r| selection.kind == "resource" && r.id == selection.id)
                        .map(|r| &r.location)
                });
            if let Some(location) = location {
                controls.push(
                    text(&match editor.resolve(location) {
                        Ok((path, line, column)) => {
                            format!("Resolved: {}:{line}:{column}", path.display())
                        }
                        Err(e) => e.to_string(),
                    })
                    .into(),
                );
            }
        }
        stack(controls)
    }
    fn analysis(&self, c: &ViewContext<Self>) -> View {
        let view = match self.app.view() {
            Ok(v) => v,
            Err(e) => return text(&e.to_string()).into(),
        };
        let mut controls = vec![
            heading("Runtime analysis"),
            self.entry(
                "window",
                "Analysis window (e.g. 60s, 5min, or none)",
                "60s",
                c,
            ),
            button("Apply time window", c, Message::Apply("window")),
        ];
        if let Some(window) = view.window {
            controls.push(
                text(&format!(
                    "Window {}–{} ns · {}",
                    window.start,
                    window.end,
                    if window.complete {
                        "complete"
                    } else {
                        "incomplete coverage"
                    }
                ))
                .into(),
            );
            if let Some(note) = window.note {
                controls.push(text(&note).into());
            }
            for task in window.tasks.iter().take(20) {
                controls.push(
                    text(&format!(
                        "Task {}: busy {}, scheduled {}, {} polls{}",
                        task.id,
                        crate::model::format_ns(task.busy_ns),
                        crate::model::format_ns(task.scheduled_ns),
                        task.polls,
                        if task.available { "" } else { " (unavailable)" }
                    ))
                    .into(),
                );
            }
        }
        controls.push(
            text(
                if view.selection.as_ref().is_some_and(|s| s.kind == "task") {
                    "Charts show the selected task. Activate a chart to open its details."
                } else {
                    "Charts sum retained tasks. Select a task for its individual time series."
                },
            )
            .into(),
        );
        for metric in ["busy", "scheduled", "polls"] {
            if let Ok(svg) = crate::sharing::chart_svg(&view.trends, metric) {
                let chart = Image::new()
                    .source_svg(EncodedImage::new(svg.into_bytes()))
                    .height(240.0)
                    .automation_name(format!(
                        "{metric} time series; producer timestamps, gaps and units shown"
                    ));
                if let Some(selection) = view.selection.as_ref().filter(|s| s.kind == "task") {
                    controls.push(
                        Button::new()
                            .content(chart)
                            .automation_name(format!("Open chart task {}", selection.id))
                            .on_click(c.message(Message::Jump(self.app.active, selection.clone())))
                            .into(),
                    );
                } else {
                    controls.push(chart.into());
                }
            }
        }
        controls.extend([
            heading("Warning thresholds"),
            self.entry(
                "self_wake",
                "Self-wake percentage",
                &self
                    .app
                    .settings
                    .lint_thresholds
                    .self_wake_percent
                    .to_string(),
                c,
            ),
            self.entry(
                "yield_duration",
                "Never-yielded duration",
                &humantime::format_duration(Duration::from_nanos(
                    self.app.settings.lint_thresholds.never_yielded_ns,
                ))
                .to_string(),
                c,
            ),
            self.entry(
                "future_bytes",
                "Large-future size in bytes",
                &self
                    .app
                    .settings
                    .lint_thresholds
                    .large_future_bytes
                    .to_string(),
                c,
            ),
            button("Apply thresholds", c, Message::Apply("thresholds")),
        ]);
        for evidence in view.evidence {
            controls.push(
                text(&format!(
                    "Task {} · {} · observed {} · threshold {}\n{}\n{}",
                    evidence.task,
                    evidence.lint.name(),
                    evidence.measured,
                    evidence.threshold,
                    evidence.explanation,
                    evidence.source
                ))
                .into(),
            );
        }
        controls.push(heading("Task/resource relationships"));
        controls.push(
            text("Readiness comes from async operations. It does not establish lock ownership.")
                .into(),
        );
        for edge in view.graph.edges {
            let mut cells =
                vec![text(&format!("{} → {} · {}", edge.from, edge.to, edge.label)).into()];
            for key in [edge.from, edge.to] {
                if let Some((kind, id)) = key.split_once(':')
                    && let Ok(id) = id.parse()
                {
                    cells.push(button(
                        &format!("Inspect {key}"),
                        c,
                        Message::Jump(
                            self.app.active,
                            Selection {
                                kind: kind.into(),
                                id,
                                session: view.session,
                            },
                        ),
                    ));
                }
            }
            controls.push(row(cells));
        }
        if view.graph.truncated {
            controls.push(text("Graph limited to 300 nodes; select a task or resource to focus its relationships.").into());
        }
        stack(controls)
    }
    fn recording(&self, c: &ViewContext<Self>) -> View {
        let view = match self.app.view() {
            Ok(v) => v,
            Err(e) => return text(&e.to_string()).into(),
        };
        let mut controls = vec![
            heading("Recording and replay"),
            text(view.histogram_notice.as_deref().unwrap_or(
                "Histogram capture uses the configured subscription and rotation limits",
            ))
            .into(),
            row(vec![
                button(
                    "Start recording…",
                    c,
                    Message::Pick(Action::StartRecording {
                        path: PathBuf::new(),
                    }),
                ),
                button("Stop recording", c, Message::Action(Action::StopRecording)),
                button(
                    "Open recording…",
                    c,
                    Message::Pick(Action::OpenRecording {
                        path: PathBuf::new(),
                        recover: false,
                    }),
                ),
                button(
                    "Recover partial recording…",
                    c,
                    Message::Pick(Action::OpenRecording {
                        path: PathBuf::new(),
                        recover: true,
                    }),
                ),
            ]),
        ];
        for session in &view.sessions {
            controls.push(button(
                &format!(
                    "Session {} · {}–{} ns · {} updates · {}{}",
                    session.index,
                    session.start,
                    session.end,
                    session.updates,
                    session.target.as_deref().unwrap_or("legacy target unknown"),
                    if session.incomplete {
                        " · incomplete"
                    } else {
                        ""
                    }
                ),
                c,
                Message::Action(Action::Session {
                    index: session.index,
                }),
            ));
        }
        if let Some(cursor) = view.cursor {
            if let Some(session) = view
                .sessions
                .iter()
                .find(|s| s.index as u64 + 1 == view.session)
            {
                let start = session.start;
                let end = session.end;
                let tab = self.app.active;
                let identity = view.session;
                controls.push(
                    Slider::new()
                        .minimum(0.0)
                        .maximum(((end - start) as f64 / 1e9).max(0.000001))
                        .step_frequency(0.001)
                        .value(cursor.saturating_sub(start) as f64 / 1e9)
                        .automation_name("Recording timeline offset in seconds")
                        .on_value_changed(c.callback(move |seconds: f64| {
                            Message::Timeline {
                                tab,
                                session: identity,
                                cursor: start
                                    .saturating_add((seconds * 1e9).max(0.0) as u64)
                                    .min(end),
                            }
                        }))
                        .into(),
                );
            }
            controls.push(
                text(&format!(
                    "Cursor {cursor} ns · {} · {:.1}x",
                    if view.playing { "playing" } else { "paused" },
                    view.speed
                ))
                .into(),
            );
            controls.push(self.entry("seek_offset", "Seek from session start (e.g. 2s)", "0s", c));
            controls.push(row(vec![
                button("Seek", c, Message::Apply("seek")),
                button(
                    if view.playing {
                        "Pause playback"
                    } else {
                        "Play recording"
                    },
                    c,
                    Message::Action(Action::Playback),
                ),
            ]));
            controls.push(self.entry("speed", "Playback speed (0.1–16)", "1", c));
            controls.push(button("Set speed", c, Message::Apply("speed")));
        }
        controls.extend([heading("Histogram capture policy"),self.entry("histogram_mode","Mode: inspected, selected, or matching",&self.app.current().policy.mode,c),self.entry("histogram_tasks","Selected task IDs, comma separated","",c),self.entry("histogram_query","Matching-task filter","",c),self.entry("histogram_limit","Maximum detail streams (1–32)","4",c),self.entry("histogram_rotation","Rotation duration in seconds","30",c),button("Apply capture policy",c,Message::Apply("histogram")),text("Policies apply bounded detail subscriptions. Matching tasks rotate; recordings identify histograms only when samples were captured.").into(),heading("Bookmarks and notes"),self.entry("bookmark_name","Bookmark name","",c),self.entry("bookmark_note","Investigation note","",c),row(vec![button("Add bookmark at cursor",c,Message::Apply("bookmark")),button("Update selected bookmark",c,Message::Apply("edit_bookmark"))])]);
        for b in view.bookmarks {
            controls.push(row(vec![
                text(&format!(
                    "{} · session {} · {} ns\n{}",
                    b.name, b.session, b.cursor, b.note
                ))
                .into(),
                button("Go", c, Message::Action(Action::GoBookmark { id: b.id })),
                button("Edit", c, Message::EditBookmark(b.id)),
                button(
                    "Delete",
                    c,
                    Message::Action(Action::DeleteBookmark { id: b.id }),
                ),
            ]));
        }
        stack(controls)
    }
    fn comparison(&self, c: &ViewContext<Self>) -> View {
        let mut controls=vec![heading("Compare diagnostic captures"),text("Open the current recording first, then choose a baseline. Metrics are normalized to each session's observation interval.").into(),self.entry("baseline_session","Baseline session index","0",c),self.entry("regression_percent","Relative regression threshold (%)","10",c),self.entry("regression_ms","Absolute busy/scheduling/latency threshold (ms)","1",c),self.entry("regression_polls","Polling-rate increase threshold (polls/s)","100",c)];
        controls.push(self.check("added_regression", "Treat added entities as regressions", c));
        controls.push(self.check(
            "removed_regression",
            "Treat removed entities as regressions",
            c,
        ));
        controls.push(button(
            "Choose baseline and compare…",
            c,
            Message::Pick(Action::Compare {
                path: PathBuf::new(),
                session: 0,
                policy: Default::default(),
            }),
        ));
        if let Some(comparison) = &self.app.comparison {
            for note in &comparison.notes {
                controls.push(text(note).into());
            }
            controls.push(row([
                ("severity", "Rank regressions"),
                ("name", "Sort by name"),
                ("busy", "Sort by busy delta"),
                ("scheduled", "Sort by scheduling delta"),
                ("polls", "Sort by polling delta"),
            ]
            .into_iter()
            .map(|(key, label)| button(label, c, Message::Text("comparison_sort", Rc::from(key))))
            .collect()));
            let mut rows = comparison.rows.iter().collect::<Vec<_>>();
            let sort = self.value("comparison_sort", "severity");
            if sort == "name" {
                rows.sort_by(|a, b| a.name.cmp(&b.name));
            } else if sort != "severity" {
                rows.sort_by(|a, b| {
                    let values = |r: &&crate::analysis::ComparisonRow| match sort.as_str() {
                        "busy" => r.busy_per_second,
                        "scheduled" => r.scheduled_per_second,
                        _ => r.polls_per_second,
                    };
                    let aa = values(a);
                    let bb = values(b);
                    (bb[1] - bb[0]).total_cmp(&(aa[1] - aa[0]))
                });
            }
            for r in rows {
                if let Some(id) = r.current {
                    controls.push(button(
                        "Inspect current entity",
                        c,
                        Message::Jump(
                            self.app.active,
                            Selection {
                                kind: r.kind.clone(),
                                id,
                                session: self.app.current().worker.read().session,
                            },
                        ),
                    ));
                }
                controls.push(text(&format!("{} · {}\nBusy {:.2} → {:.2} ns/s · scheduled {:.2} → {:.2} ns/s · polling {:.2} → {:.2} polls/s",r.name,r.status,r.busy_per_second[0],r.busy_per_second[1],r.scheduled_per_second[0],r.scheduled_per_second[1],r.polls_per_second[0],r.polls_per_second[1])).into());
            }
        }
        stack(controls)
    }
    fn comparison_request(&self) -> Result<Action> {
        let ms: f64 = self
            .value("regression_ms", "1")
            .parse()
            .context("Enter a valid absolute threshold")?;
        let percent: f64 = self
            .value("regression_percent", "10")
            .parse()
            .context("Enter a valid percentage")?;
        anyhow::ensure!(
            ms.is_finite()
                && ms >= 0.0
                && ms * 1e6 < u64::MAX as f64
                && percent.is_finite()
                && percent >= 0.0,
            "Regression thresholds must be finite and nonnegative"
        );
        Ok(Action::Compare {
            path: PathBuf::new(),
            session: self
                .value("baseline_session", "0")
                .parse()
                .context("Enter a session index")?,
            policy: RegressionPolicy {
                relative_percent: percent,
                absolute_ns: (ms * 1e6) as u64,
                polls_increase: self.value("regression_polls", "100").parse()?,
                added_is_regression: self.flag("added_regression"),
                removed_is_regression: self.flag("removed_regression"),
            },
        })
    }
    fn redaction(&self) -> RedactionPolicy {
        RedactionPolicy {
            fields: self
                .value("redact_fields", "")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            source_paths: self.flag("redact_paths"),
            endpoints: self.flag("redact_endpoint"),
        }
    }
    fn export(&self, c: &ViewContext<Self>) -> View {
        let policy = self.redaction();
        let mut controls = vec![
            heading("Export and sharing"),
            self.entry(
                "report_sections",
                "Report sections: charts,tasks,resources,comparison",
                &self.app.report_sections.join(","),
                c,
            ),
            button(
                "Apply report sections",
                c,
                Message::Apply("report_sections"),
            ),
            self.entry(
                "redact_fields",
                "Redact span/attribute field keys, comma separated",
                "",
                c,
            ),
            self.check("redact_paths", "Redact source paths", c),
            self.check("redact_endpoint", "Redact endpoint details", c),
            self.entry(
                "export_table",
                "Table: tasks, resources, or operations",
                "tasks",
                c,
            ),
        ];
        for (format, label) in [
            ("csv", "Export table CSV…"),
            ("json", "Export JSON…"),
            ("html", "Export self-contained report…"),
        ] {
            controls.push(button(
                label,
                c,
                Message::Pick(Action::Export {
                    path: PathBuf::new(),
                    format: format.into(),
                    table: self.value("export_table", "tasks"),
                    redaction: policy.clone(),
                }),
            ));
        }
        for (metric, label) in [
            ("polls", "Save polling chart SVG…"),
            ("busy", "Save busy-time chart SVG…"),
            ("scheduled", "Save scheduling chart SVG…"),
        ] {
            controls.push(button(
                label,
                c,
                Message::Pick(Action::Export {
                    path: PathBuf::new(),
                    format: "svg".into(),
                    table: metric.into(),
                    redaction: policy.clone(),
                }),
            ));
        }
        controls.push(button(
            "Save redacted recording copy…",
            c,
            Message::Pick(Action::RedactRecording {
                path: PathBuf::new(),
                policy: policy.clone(),
            }),
        ));
        controls.push(heading("Export preview"));
        if let Ok(view) = self.app.view() {
            let document = policy.document(&crate::sharing::ExportDocument::new(
                view.session,
                Some(view.target),
                view.snapshot,
            ));
            controls.push(
                text(&format!(
                    "{} tasks, {} resources, {} operations\nTarget: {}",
                    document.snapshot.tasks.len(),
                    document.snapshot.resources.len(),
                    document.snapshot.async_ops.len(),
                    document.target.as_deref().unwrap_or("unknown")
                ))
                .into(),
            );
            for task in document.snapshot.tasks.iter().take(12) {
                controls.push(
                    text(&format!(
                        "{} · {}\n{}\n{}",
                        task.id, task.name, task.location, task.fields
                    ))
                    .into(),
                );
            }
        }
        stack(controls)
    }
    fn workspace(&self, c: &ViewContext<Self>) -> View {
        let mut controls=vec![heading("Project workspace"),self.entry("workspace_name","Workspace name",&self.app.name,c),row(vec![button("New workspace",c,Message::Apply("new_workspace")),button("Save workspace…",c,Message::Pick(Action::SaveWorkspace{path:PathBuf::new(),name:self.value("workspace_name",&self.app.name)})),button("Open workspace…",c,Message::Pick(Action::OpenWorkspace{path:PathBuf::new()}))]),self.entry("view_name","Investigation view name","",c),button("Save current investigation view",c,Message::Apply("save_view")),text("Project policy precedence: application defaults → project policy → explicit session settings.").into(),row(vec![button("Import project policy…",c,Message::Pick(Action::LoadPolicy{path:PathBuf::new()})),button("Export current project policy…",c,Message::Pick(Action::SavePolicy{path:PathBuf::new()}))])];
        controls.push(self.entry(
            "view_original",
            "Existing view name to rename or duplicate",
            "",
            c,
        ));
        controls.push(row(vec![
            button("Rename saved view", c, Message::Apply("rename_view")),
            button("Duplicate saved view", c, Message::Apply("duplicate_view")),
        ]));
        if let Ok(view) = self.app.view() {
            for v in view.views {
                controls.push(row(vec![
                    text(&v.name).into(),
                    button(
                        "Restore",
                        c,
                        Message::Action(Action::LoadView {
                            name: v.name.clone(),
                        }),
                    ),
                    button(
                        "Delete",
                        c,
                        Message::Action(Action::DeleteView { name: v.name }),
                    ),
                ]));
            }
        }
        for (index, tab) in self.app.tabs.iter().enumerate() {
            controls.push(row(vec![
                text(&tab.document.name).into(),
                button(
                    "Move tab left",
                    c,
                    Message::Action(Action::MoveTab {
                        id: tab.document.id,
                        index: index.saturating_sub(1),
                    }),
                ),
                button(
                    "Move tab right",
                    c,
                    Message::Action(Action::MoveTab {
                        id: tab.document.id,
                        index: index + 1,
                    }),
                ),
            ]));
        }
        controls.push(heading("CLI automation"));
        controls.push(text("Use console-ui check --rules budgets.json --input capture.tcui, or omit --input for live observation. Add --baseline baseline.tcui for regression checks. Exit codes: 0 pass, 1 failed assertion, 2 invalid configuration, 3 incomplete/unavailable telemetry.").into());
        stack(controls)
    }
}
fn heading(label: &str) -> View {
    TextBlock::new()
        .text(label)
        .font_size(24.0)
        .font_weight(FontWeight::SEMI_BOLD)
        .into()
}
fn text(label: &str) -> TextBlock {
    TextBlock::new()
        .text(label)
        .text_wrapping(TextWrapping::Wrap)
        .is_text_selection_enabled(true)
}
fn row(views: Vec<View>) -> View {
    StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(10.0)
        .children(views)
        .into()
}
fn stack(views: Vec<View>) -> View {
    StackPanel::new().spacing(14.0).children(views).into()
}
fn button(label: &str, c: &ViewContext<Workbench>, message: Message) -> View {
    Button::new()
        .content(label)
        .on_click(c.message(message))
        .into()
}

fn pick(action: Action) -> Option<Action> {
    let save = matches!(
        action,
        Action::StartRecording { .. }
            | Action::Export { .. }
            | Action::RedactRecording { .. }
            | Action::SaveWorkspace { .. }
            | Action::SavePolicy { .. }
    );
    let extension = match &action {
        Action::StartRecording { .. }
        | Action::OpenRecording { .. }
        | Action::Compare { .. }
        | Action::RedactRecording { .. } => "tcui",
        Action::Export { format, .. } => format.as_str(),
        _ => "json",
    };
    let dialog = rfd::FileDialog::new()
        .set_title(if save {
            "Save Console UI artifact"
        } else {
            "Open Console UI artifact"
        })
        .add_filter("Console UI", &[extension]);
    let path = if save {
        dialog.save_file()
    } else {
        dialog.pick_file()
    }?;
    Some(match action {
        Action::StartRecording { .. } => Action::StartRecording { path },
        Action::OpenRecording { recover, .. } => Action::OpenRecording { path, recover },
        Action::Compare {
            session, policy, ..
        } => Action::Compare {
            path,
            session,
            policy,
        },
        Action::Export {
            format,
            table,
            redaction,
            ..
        } => Action::Export {
            path,
            format,
            table,
            redaction,
        },
        Action::RedactRecording { policy, .. } => Action::RedactRecording { path, policy },
        Action::SaveWorkspace { name, .. } => Action::SaveWorkspace { path, name },
        Action::OpenWorkspace { .. } => Action::OpenWorkspace { path },
        Action::LoadPolicy { .. } => Action::LoadPolicy { path },
        Action::SavePolicy { .. } => Action::SavePolicy { path },
        _ => return None,
    })
}
pub fn run(
    settings: Settings,
    record: Option<PathBuf>,
    replay: Option<PathBuf>,
    policy: Option<PathBuf>,
) -> Result<()> {
    let mut app = Workspace::new(settings, record, replay)?;
    if let Some(path) = policy {
        app.dispatch(Action::LoadPolicy { path })?;
    }
    App::run_component_with_policy::<Workbench>(
        Input(Rc::new(RefCell::new(Some(app)))),
        WindowPolicy::new().minimum_client_size(1120.0, 700.0),
    )?;
    Ok(())
}
