use crate::{
    config::{Lint, Palette, Settings, validate_target},
    model::{Distribution, Snapshot, TaskState, Timing, format_ns, percent},
    telemetry::{Command, ConnectionStatus, LiveState, Worker},
};
use anyhow::Result;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    time::{Duration, Instant},
};
use windows_reactor::*;

static NEXT_CONSOLE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct Launch {
    pub settings: Settings,
    pub worker: Arc<Worker>,
    pub query: String,
    pub tables: BTreeMap<String, crate::preferences::TablePreferences>,
    pub navigation: Option<(u64, crate::workspace::Selection)>,
    pub on_selection: Callback<Option<crate::workspace::Selection>>,
    pub on_query: Callback<String>,
    pub on_target: Callback<String>,
    pub on_settings: Callback<Settings>,
    pub on_table: Callback<(String, crate::preferences::TablePreferences)>,
}
impl PartialEq for Launch {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.worker, &other.worker)
            && self.settings == other.settings
            && self.query == other.query
            && self.tables == other.tables
            && self.navigation == other.navigation
            && self.on_selection == other.on_selection
            && self.on_query == other.on_query
            && self.on_target == other.on_target
            && self.on_settings == other.on_settings
            && self.on_table == other.on_table
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Tasks,
    Resources,
    Task(u64),
    Resource(u64),
    Settings,
    Help,
}

#[derive(Clone)]
pub enum Message {
    Tick,
    Connect,
    Endpoint(Rc<str>),
    Search(Rc<str>),
    Select(Option<u64>),
    Tasks,
    Resources,
    Settings,
    Help,
    Back,
    Inspect,
    OpenTask(u64),
    Pause,
    Sort(usize),
    Invert,
    Key(KeyEventInfo),
    Editing(bool),
    Warn(Lint, bool),
    WarningsOnly(bool),
    Palette(Palette),
    Save,
    FocusSelection,
    SelectionScrolled,
}

pub struct Console {
    instance: u64,
    settings: Settings,
    worker: Arc<Worker>,
    live: LiveState,
    snapshot: Arc<Snapshot>,
    timer: ComponentTimer,
    screen: Screen,
    parent: Screen,
    endpoint: Rc<str>,
    search: Rc<str>,
    selected: Option<u64>,
    sort: usize,
    descending: bool,
    editing: bool,
    warnings_only: bool,
    last_g: Option<Instant>,
    notice: Option<String>,
    external_query: String,
    navigation: u64,
    notified_selection: Option<crate::workspace::Selection>,
    tables: BTreeMap<String, crate::preferences::TablePreferences>,
    selected_row: ElementRef<Button>,
    focus_timer: Option<ComponentTimer>,
}

impl Component for Console {
    type Input = Launch;
    type Message = Message;

    fn create(input: &Launch, context: &ComponentContext<Self>) -> Self {
        let live = input.worker.read();
        let mut result = Self {
            settings: input.settings.clone(),
            worker: Arc::clone(&input.worker),
            snapshot: Arc::clone(&live.snapshot),
            live,
            timer: context.set_local_timeout(Duration::from_millis(150), || Message::Tick),
            screen: Screen::Tasks,
            parent: Screen::Tasks,
            endpoint: Rc::from(input.settings.target.as_str()),
            search: Rc::from(input.query.as_str()),
            selected: None,
            sort: 5,
            descending: true,
            editing: false,
            warnings_only: false,
            last_g: None,
            notice: None,
            external_query: input.query.clone(),
            navigation: 0,
            notified_selection: None,
            tables: input.tables.clone(),
            instance: NEXT_CONSOLE.fetch_add(1, AtomicOrdering::Relaxed),
            selected_row: ElementRef::new(),
            focus_timer: None,
        };
        result.input_changed(input, context);
        result
    }

    fn input_changed(&mut self, input: &Launch, _: &ComponentContext<Self>) {
        self.settings = input.settings.clone();
        self.tables = input.tables.clone();
        self.restore_sort();
        if self.external_query != input.query {
            self.external_query = input.query.clone();
            self.search = Rc::from(input.query.as_str());
            self.selected = None;
        }
        if let Some((sequence, selection)) = &input.navigation
            && *sequence != self.navigation
            && selection.session == self.live.session
        {
            self.navigation = *sequence;
            match selection.kind.as_str() {
                "task" => {
                    self.screen = Screen::Task(selection.id);
                    self.parent = Screen::Tasks;
                    self.worker.send(Command::Inspect(Some(selection.id)));
                }
                "resource" => {
                    self.screen = Screen::Resource(selection.id);
                    self.parent = Screen::Resources;
                }
                _ => {}
            }
        }
    }

    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        let old_sort = (self.sort, self.descending);
        let focus_after = matches!(
            &message,
            Message::Key(_)
                | Message::Select(_)
                | Message::Sort(_)
                | Message::Invert
                | Message::Back
        );
        match message {
            Message::SelectionScrolled => {}
            Message::FocusSelection => {
                if let Some(id) = self.selected {
                    let row = self.row_automation_id(id);
                    let queued = context.run_window(move |window| {
                        crate::windows_scroll::request(window.as_raw() as usize, row);
                        Message::SelectionScrolled
                    });
                    if !queued {
                        eprintln!("Native scroll window request was not accepted");
                    }
                }
                let queued = self.selected_row.request_focus_result(|result| {
                    if !matches!(result, Ok(true)) {
                        eprintln!("Selected row focus: {result:?}");
                    }
                });
                if !queued {
                    eprintln!("Selected row is not realized for keyboard focus");
                }
            }
            Message::Tick => {
                let next = self.worker.read();
                if next.session != self.live.session {
                    self.selected = None;
                    if matches!(self.screen, Screen::Task(_) | Screen::Resource(_)) {
                        self.navigate(Screen::Tasks);
                    }
                }
                if !next.paused || !self.live.paused || next.session != self.live.session {
                    self.snapshot = Arc::clone(&next.snapshot);
                }
                self.live = next;
                if self.live.status == ConnectionStatus::Offline {
                    self.endpoint = Rc::from(self.live.target.as_str());
                }
                self.timer =
                    context.set_local_timeout(Duration::from_millis(150), || Message::Tick);
            }
            Message::Connect => match validate_target(&self.endpoint) {
                Ok(()) if self.live.status != ConnectionStatus::Offline => {
                    context.input().on_target.call(self.endpoint.to_string());
                    self.settings.target = self.endpoint.to_string();
                    self.notice = None;
                }
                Ok(()) => {}
                Err(error) => self.notice = Some(error.to_string()),
            },
            Message::Endpoint(value) => self.endpoint = value,
            Message::Search(value) => {
                self.search = value;
                self.selected = None;
                match crate::query::Query::parse(&self.search) {
                    Ok(_) => {
                        context.input().on_query.call(self.search.to_string());
                        self.notice = None;
                    }
                    Err(error) => self.notice = Some(error.to_string()),
                }
            }
            Message::Select(id) => self.selected = id,
            Message::Tasks => self.navigate(Screen::Tasks),
            Message::Resources => self.navigate(Screen::Resources),
            Message::Settings => self.navigate(Screen::Settings),
            Message::Help => self.navigate(Screen::Help),
            Message::Back => self.back(),
            Message::Inspect => match (self.screen, self.selected) {
                (Screen::Tasks, Some(id)) => self.open_task(id),
                (Screen::Resources, Some(id)) => {
                    self.screen = Screen::Resource(id);
                    self.parent = Screen::Resources;
                    self.selected = None;
                    self.sort = 3;
                }
                (Screen::Resource(_), Some(id)) => {
                    if let Some(task) = self
                        .snapshot
                        .async_ops
                        .iter()
                        .find(|o| o.id == id)
                        .and_then(|o| o.task_id)
                    {
                        self.open_task(task);
                    }
                }
                _ => {}
            },
            Message::OpenTask(id) => self.open_task(id),
            Message::Pause if self.live.status == ConnectionStatus::Connected => {
                self.worker.send(Command::Pause(!self.live.paused))
            }
            Message::Pause => {}
            Message::Sort(index) => {
                if self.sort == index {
                    self.descending = !self.descending;
                } else {
                    self.sort = index;
                    self.descending = true;
                }
            }
            Message::Invert => self.descending = !self.descending,
            Message::Editing(editing) => self.editing = editing,
            Message::WarningsOnly(value) => {
                self.warnings_only = value;
                self.selected = None;
            }
            Message::Warn(lint, enabled) => {
                if enabled {
                    self.settings.warnings.insert(lint);
                } else {
                    self.settings.warnings.remove(&lint);
                }
                self.worker
                    .send(Command::Warnings(self.settings.warnings.clone()));
                let mut snapshot = (*self.snapshot).clone();
                for task in &mut snapshot.tasks {
                    task.lint(&self.settings.warnings);
                }
                self.snapshot = Arc::new(snapshot);
                context.input().on_settings.call(self.settings.clone());
            }
            Message::Palette(palette) => {
                self.settings.palette = palette;
                context.input().on_settings.call(self.settings.clone());
            }
            Message::Save => {
                context.input().on_settings.call(self.settings.clone());
                self.notice = Some("Settings saved to your application preferences".into());
            }
            Message::Key(event) => self.key(event, context),
        }
        if focus_after
            && self.selected.is_some()
            && matches!(
                self.screen,
                Screen::Tasks | Screen::Resources | Screen::Resource(_)
            )
        {
            self.focus_timer = Some(
                context.set_local_timeout(Duration::from_millis(30), || Message::FocusSelection),
            );
        }
        let selection = match self.screen {
            Screen::Task(id) => Some(("task", id)),
            Screen::Resource(id) => Some(("resource", id)),
            Screen::Tasks => self.selected.map(|id| ("task", id)),
            Screen::Resources => self.selected.map(|id| ("resource", id)),
            _ => None,
        }
        .map(|(kind, id)| crate::workspace::Selection {
            kind: kind.into(),
            id,
            session: self.live.session,
        });
        if selection != self.notified_selection {
            self.notified_selection = selection.clone();
            context.input().on_selection.call(selection);
        }
        if old_sort != (self.sort, self.descending)
            && matches!(
                self.screen,
                Screen::Tasks | Screen::Resources | Screen::Resource(_)
            )
        {
            let key = self.table_key().to_owned();
            let mut preferences = self.tables.get(&key).cloned().unwrap_or_default();
            preferences.sort = self
                .headers()
                .get(self.sort)
                .unwrap_or(&"ID")
                .to_lowercase();
            preferences.descending = self.descending;
            context.input().on_table.call((key, preferences));
        }
    }

    fn view(&self, _: &Launch, context: &mut ViewContext<Self>) -> View {
        let sender = context.sender();
        let editing = self.editing;
        let keys = RoutedCallback::new(move |event: KeyEventInfo| {
            if editing {
                return false;
            }
            // Let the native list handle Up/Down and Home/End so selection,
            // container realization, and scrolling advance together.
            let recognized = matches!(
                event.key,
                VirtualKey::T
                    | VirtualKey::R
                    | VirtualKey::Q
                    | VirtualKey::J
                    | VirtualKey::K
                    | VirtualKey::H
                    | VirtualKey::L
                    | VirtualKey::I
                    | VirtualKey::G
                    | VirtualKey::SPACE
                    | VirtualKey::ESCAPE
                    | VirtualKey::ENTER
                    | VirtualKey::LEFT
                    | VirtualKey::RIGHT
            );
            if recognized
                && !event.modifiers.contains(InputModifiers::ALT)
                && !event.modifiers.contains(InputModifiers::CONTROL)
            {
                let _ = sender.send(Message::Key(event));
                true
            } else {
                false
            }
        });
        let content = match self.screen {
            Screen::Task(id) => self.task_details(id, context),
            Screen::Resource(id) => self.resource_details(id, context),
            Screen::Settings => self.settings_view(context),
            Screen::Help => help_view(),
            _ => self.list_view(context),
        };
        let frame = Border::new().background(ThemeBrush::SolidBackground).is_tab_stop(true)
            .on_preview_key_down(keys).padding(Thickness::new(24.0, 12.0, 24.0, 12.0))
            .content(Grid::new().rows([GridLength::Auto, GridLength::Auto, GridLength::STAR, GridLength::Auto])
                .row_spacing(14.0).children((
                    self.header(context),
                    Border::new().grid_row(1).content(self.navigation(context)),
                    Border::new().grid_row(2).content(content),
                    TextBlock::new().grid_row(3).font_size(12.0).opacity(0.75)
                        .text("t Tasks   r Resources   j/k Move   h/l Sort   i Invert   Enter Inspect   Esc Back   Space Pause   q Quit"),
                )));
        frame.into()
    }
}

impl Console {
    fn row_automation_id(&self, id: u64) -> String {
        format!(
            "console-{}-{}-{}-{id}",
            self.instance,
            self.live.session,
            self.table_key()
        )
    }
    fn table_key(&self) -> &'static str {
        match self.screen {
            Screen::Resources => "resources",
            Screen::Resource(_) => "operations",
            _ => "tasks",
        }
    }
    fn restore_sort(&mut self) {
        if let Some(p) = self.tables.get(self.table_key())
            && let Some(index) = self
                .headers()
                .iter()
                .position(|h| h.eq_ignore_ascii_case(&p.sort))
        {
            self.sort = index;
            self.descending = p.descending;
        }
    }
    fn navigate(&mut self, screen: Screen) {
        self.worker.send(Command::Inspect(None));
        self.screen = screen;
        self.parent = if matches!(screen, Screen::Resource(_)) {
            Screen::Resources
        } else {
            Screen::Tasks
        };
        self.selected = None;
        self.sort = if screen == Screen::Tasks { 5 } else { 0 };
        self.restore_sort();
    }
    fn open_task(&mut self, id: u64) {
        self.parent = if matches!(self.screen, Screen::Resource(_)) {
            self.screen
        } else {
            Screen::Tasks
        };
        self.screen = Screen::Task(id);
        self.worker.send(Command::Inspect(Some(id)));
    }
    fn back(&mut self) {
        let selected = match (self.screen, self.parent) {
            (Screen::Task(id), Screen::Tasks) | (Screen::Resource(id), Screen::Resources) => {
                Some(id)
            }
            (Screen::Task(_), Screen::Resource(_)) => self.selected,
            _ => None,
        };
        self.navigate(self.parent);
        self.selected = selected;
    }
    fn key(&mut self, event: KeyEventInfo, context: &ComponentContext<Self>) {
        match event.key {
            VirtualKey::T => self.navigate(Screen::Tasks),
            VirtualKey::R => self.navigate(Screen::Resources),
            VirtualKey::Q => {
                let _ = context.close_window();
            }
            VirtualKey::ESCAPE => self.back(),
            VirtualKey::SPACE => self.update(Message::Pause, context),
            VirtualKey::ENTER => self.update(Message::Inspect, context),
            VirtualKey::I => self.descending = !self.descending,
            VirtualKey::H | VirtualKey::LEFT => self.sort = self.sort.saturating_sub(1),
            VirtualKey::L | VirtualKey::RIGHT => {
                self.sort = (self.sort + 1).min(self.headers().len().saturating_sub(1))
            }
            key => {
                let rows = self.rows();
                if rows.is_empty() {
                    return;
                }
                let index = self
                    .selected
                    .and_then(|id| rows.iter().position(|r| r.id == id))
                    .unwrap_or(0);
                let next = match key {
                    VirtualKey::J | VirtualKey::DOWN => {
                        if self.selected.is_none() {
                            0
                        } else {
                            (index + 1).min(rows.len() - 1)
                        }
                    }
                    VirtualKey::K | VirtualKey::UP => index.saturating_sub(1),
                    VirtualKey::HOME => 0,
                    VirtualKey::END => rows.len() - 1,
                    VirtualKey::G if event.modifiers.contains(InputModifiers::SHIFT) => {
                        rows.len() - 1
                    }
                    VirtualKey::G => {
                        let now = Instant::now();
                        let double = self.last_g.is_some_and(|last| {
                            now.duration_since(last) < Duration::from_millis(600)
                        });
                        self.last_g = Some(now);
                        if double {
                            0
                        } else {
                            return;
                        }
                    }
                    _ => return,
                };
                self.selected = Some(rows[next].id);
            }
        }
    }
    fn header(&self, context: &ViewContext<Self>) -> View {
        let status = if self.live.status == ConnectionStatus::Offline {
            "OFFLINE"
        } else if self.live.status == ConnectionStatus::Connected && self.live.paused {
            "PAUSED"
        } else {
            match self.live.status {
                ConnectionStatus::Connecting => "CONNECTING",
                ConnectionStatus::Connected => "CONNECTED",
                _ => "DISCONNECTED",
            }
        };
        let mut children = vec![
            Grid::new()
                .columns([GridLength::STAR, GridLength::Auto])
                .column_spacing(16.0)
                .children((
                    StackPanel::new().spacing(4.0).children((
                        title("Runtime observatory", 28.0),
                        TextBlock::new()
                            .text("Understand what your async application is doing.")
                            .opacity(0.7),
                    )),
                    TextBlock::new()
                        .grid_column(1)
                        .text(status)
                        .font_weight(FontWeight::SEMI_BOLD)
                        .vertical_alignment(VerticalAlignment::Center),
                ))
                .into(),
            Grid::new()
                .columns([GridLength::STAR, GridLength::Auto, GridLength::Auto])
                .column_spacing(8.0)
                .children((
                    Border::new()
                        .content(
                            TextBox::new()
                                .text(self.endpoint.clone())
                                .placeholder_text("http://127.0.0.1:6669")
                                .automation_name("Telemetry endpoint")
                                .is_enabled(self.live.status != ConnectionStatus::Offline)
                                .on_text_changed(context.callback(Message::Endpoint)),
                        )
                        .on_got_focus(context.callback(|_| Message::Editing(true)))
                        .on_lost_focus(context.callback(|_| Message::Editing(false))),
                    Button::new()
                        .grid_column(1)
                        .content("Connect")
                        .on_click(context.message(Message::Connect))
                        .is_enabled(self.live.status != ConnectionStatus::Offline),
                    Button::new()
                        .grid_column(2)
                        .content(if self.live.paused { "Resume" } else { "Pause" })
                        .is_enabled(self.live.status == ConnectionStatus::Connected)
                        .on_click(context.message(Message::Pause)),
                ))
                .into(),
        ];
        for message in [&self.notice, &self.live.error, &self.live.notice]
            .into_iter()
            .flatten()
        {
            children.push(note(message));
        }
        if self.snapshot.dropped_events > 0 {
            children.push(note(&format!("{} events were dropped by the target. Increase console-subscriber's event buffer to avoid incomplete data.", self.snapshot.dropped_events)));
        }
        StackPanel::new().spacing(10.0).children(children).into()
    }
    fn navigation(&self, context: &ViewContext<Self>) -> View {
        let running = self
            .snapshot
            .tasks
            .iter()
            .filter(|t| t.state == TaskState::Running)
            .count();
        let scheduled = self
            .snapshot
            .tasks
            .iter()
            .filter(|t| t.state == TaskState::Scheduled)
            .count();
        let completed = self
            .snapshot
            .tasks
            .iter()
            .filter(|t| t.state == TaskState::Completed)
            .count();
        let idle = self.snapshot.tasks.len() - running - scheduled - completed;
        Grid::new().columns([GridLength::Auto, GridLength::STAR]).column_spacing(20.0).children((
            StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((
                Button::new().content("Tasks").style(if matches!(self.screen, Screen::Tasks | Screen::Task(_)) { ButtonStyle::Accent } else { ButtonStyle::Subtle }).on_click(context.message(Message::Tasks)),
                Button::new().content("Resources").style(if matches!(self.screen, Screen::Resources | Screen::Resource(_)) { ButtonStyle::Accent } else { ButtonStyle::Subtle }).on_click(context.message(Message::Resources)),
                Button::new().content("Settings").style(ButtonStyle::Subtle).on_click(context.message(Message::Settings)),
                Button::new().content("Help").style(ButtonStyle::Subtle).on_click(context.message(Message::Help)),
            )),
            TextBlock::new().grid_column(1).vertical_alignment(VerticalAlignment::Center).horizontal_alignment(HorizontalAlignment::Right)
                .text(format!("{running} running   {scheduled} scheduled   {idle} idle   {completed} completed")),
        )).into()
    }
    fn headers(&self) -> &'static [&'static str] {
        match self.screen {
            Screen::Resources => &[
                "ID",
                "Kind",
                "Type",
                "Total",
                "Visibility",
                "Target",
                "Location",
                "Attributes",
            ],
            Screen::Resource(_) => &[
                "ID",
                "Task",
                "Source",
                "Total",
                "Busy",
                "Idle",
                "Polls",
                "State",
                "Attributes",
            ],
            _ => &[
                "Warn", "ID", "Name", "State", "Total", "Busy", "Sched", "Idle", "Polls", "Target",
                "Location",
            ],
        }
    }
    fn rows(&self) -> Vec<Row> {
        let query = match crate::query::Query::parse(&self.search) {
            Ok(q) => q,
            Err(_) => return vec![],
        };
        let mut rows: Vec<Row> = match self.screen {
            Screen::Resources => self
                .snapshot
                .resources
                .iter()
                .filter(|r| query.resource(r))
                .map(|r| Row {
                    id: r.id,
                    cells: vec![
                        Cell::number(r.id),
                        Cell::text(&r.kind),
                        Cell::text(&r.concrete_type),
                        Cell::duration(r.total),
                        Cell::text(if r.internal { "INT" } else { "PUB" }),
                        Cell::text(&r.target),
                        Cell::text(&r.location),
                        Cell::text(&r.attributes),
                    ],
                })
                .collect(),
            Screen::Resource(id) => self
                .snapshot
                .async_ops
                .iter()
                .filter(|o| o.resource_id == id)
                .filter(|o| query.operation(o))
                .map(|o| Row {
                    id: o.id,
                    cells: vec![
                        Cell::number(o.id),
                        Cell::text(format!(
                            "{} {}",
                            o.task_id.map(|id| id.to_string()).unwrap_or_default(),
                            o.task_name
                        )),
                        Cell::text(&o.source),
                        Cell::duration(o.timing.total),
                        Cell::duration(o.timing.busy),
                        Cell::duration(o.timing.idle),
                        Cell::number(o.timing.polls),
                        Cell::text(if o.completed {
                            "Completed"
                        } else {
                            match o.ready {
                                Some(true) => "Ready",
                                Some(false) => "Pending",
                                None => "Unknown",
                            }
                        }),
                        Cell::text(&o.attributes),
                    ],
                })
                .collect(),
            _ => self
                .snapshot
                .tasks
                .iter()
                .filter(|t| !self.warnings_only || !t.warnings.is_empty())
                .filter(|t| query.task(t))
                .map(|t| Row {
                    id: t.id,
                    cells: vec![
                        Cell {
                            text: if t.warnings.is_empty() {
                                String::new()
                            } else if self.settings.ascii_only {
                                "!".into()
                            } else {
                                "⚠".into()
                            },
                            order: SortValue::Number(t.warnings.len() as u64),
                        },
                        Cell::number(t.id),
                        Cell::text(&t.name),
                        Cell::text(format!("{:?}", t.state)),
                        Cell::duration(t.timing.total),
                        Cell::duration(t.timing.busy),
                        Cell::duration(t.timing.scheduled),
                        Cell::duration(t.timing.idle),
                        Cell::number(t.timing.polls),
                        Cell::text(&t.target),
                        Cell::text(&t.location),
                    ],
                })
                .collect(),
        };
        rows.sort_by(|a, b| {
            let order = a
                .cells
                .get(self.sort)
                .zip(b.cells.get(self.sort))
                .map_or(Ordering::Equal, |(a, b)| a.order.cmp(&b.order));
            let order = if self.descending {
                order.reverse()
            } else {
                order
            };
            order.then(a.id.cmp(&b.id))
        });
        rows
    }
    fn list_view(&self, context: &ViewContext<Self>) -> View {
        let mut top = vec![];
        if self.screen == Screen::Tasks {
            let warnings = self
                .snapshot
                .tasks
                .iter()
                .filter(|t| !t.warnings.is_empty())
                .count();
            top.push(
                TextBlock::new()
                    .text(format!("{warnings} tasks with warnings"))
                    .font_weight(FontWeight::SEMI_BOLD)
                    .into(),
            );
        }
        top.push(
            Grid::new()
                .columns([GridLength::STAR, GridLength::Auto, GridLength::Auto])
                .column_spacing(12.0)
                .children((
                    self.search_box(context),
                    CheckBox::new()
                        .grid_column(1)
                        .content("Warnings only")
                        .is_checked(self.warnings_only)
                        .is_enabled(self.screen == Screen::Tasks)
                        .on_is_checked_changed(
                            context.callback(|v: Option<bool>| {
                                Message::WarningsOnly(v.unwrap_or(false))
                            }),
                        ),
                    Button::new()
                        .grid_column(2)
                        .content("Inspect selected")
                        .is_enabled(self.selected.is_some())
                        .on_click(context.message(Message::Inspect)),
                ))
                .into(),
        );
        Grid::new()
            .rows([GridLength::Auto, GridLength::STAR])
            .row_spacing(12.0)
            .children((
                StackPanel::new().spacing(10.0).children(top),
                Border::new().grid_row(1).content(self.table(context)),
            ))
            .into()
    }
    fn search_box(&self, context: &ViewContext<Self>) -> Border {
        Border::new()
            .content(
                TextBox::new()
                    .text(self.search.clone())
                    .placeholder_text(
                        "Filter: name:worker busy>5ms !state:completed | warning:self-wakes",
                    )
                    .automation_name("Filter telemetry")
                    .on_text_changed(context.callback(Message::Search)),
            )
            .on_got_focus(context.callback(|_| Message::Editing(true)))
            .on_lost_focus(context.callback(|_| Message::Editing(false)))
    }
    fn table(&self, context: &ViewContext<Self>) -> View {
        let rows = self.rows();
        let preferences = self
            .tables
            .get(self.table_key())
            .cloned()
            .unwrap_or_default();
        let configured = if preferences.columns.is_empty() {
            crate::preferences::default_columns(self.table_key())
        } else {
            preferences.columns
        };
        let visible: Vec<_> = configured
            .iter()
            .filter(|c| c.visible)
            .filter_map(|c| {
                self.headers()
                    .iter()
                    .position(|h| h.eq_ignore_ascii_case(&c.key))
                    .map(|index| (index, c.width))
            })
            .collect();
        let columns: Vec<_> = visible
            .iter()
            .map(|(_, width)| GridLength::Pixel(*width))
            .collect();
        let width =
            visible.iter().map(|(_, width)| *width).sum::<f64>() + 4.0 * visible.len() as f64;
        let headers = Grid::new()
            .width(width)
            .columns(columns.clone())
            .column_spacing(4.0)
            .margin(Thickness::new(12.0, 0.0, 12.0, 0.0))
            .children(
                visible
                    .iter()
                    .enumerate()
                    .map(|(display, (index, _))| {
                        Button::new()
                            .grid_column(display as i32)
                            .style(ButtonStyle::Subtle)
                            .content(format!(
                                "{}{}",
                                self.headers()[*index],
                                if *index == self.sort {
                                    if self.descending { " v" } else { " ^" }
                                } else {
                                    ""
                                }
                            ))
                            .on_click(context.message(Message::Sort(*index)))
                            .into()
                    })
                    .collect::<Vec<View>>(),
            );
        let table: View = if rows.is_empty() {
            card(
                "No matching telemetry",
                "Start an instrumented application, check the endpoint, or clear the filter.",
            )
        } else {
            ListBox::new()
                .automation_name("Live telemetry table")
                .on_selected_tag_changed(context.callback(|tag: Option<Rc<str>>| {
                    Message::Select(tag.and_then(|tag| tag.parse().ok()))
                }))
                .keyed_items(rows.into_iter().map(|row| {
                    let description = row
                        .cells
                        .iter()
                        .map(|c| c.text.as_str())
                        .collect::<Vec<_>>()
                        .join("  ");
                    let description = format!("ID {}, {description}", row.id);
                    let content = Grid::new()
                        .width(width)
                        .columns(columns.clone())
                        .column_spacing(4.0)
                        .children(
                            visible
                                .iter()
                                .enumerate()
                                .map(|(display, (index, _))| {
                                    TextBlock::new()
                                        .grid_column(display as i32)
                                        .text(row.cells[*index].text.clone())
                                        .font_size(12.0)
                                        .text_trimming(TextTrimming::CharacterEllipsis)
                                        .margin(Thickness::xy(2.0, 8.0))
                                        .into()
                                })
                                .collect::<Vec<View>>(),
                        );
                    // Use a native focusable control so WinUI can bring the
                    // keyboard selection into view, including offscreen rows.
                    let frame = Button::new()
                        .automation_name(description.clone())
                        .style(ButtonStyle::Subtle)
                        .width(width)
                        .content(content)
                        .on_click(context.message(Message::Select(Some(row.id))));
                    // Own the native item instead of a data-template wrapper:
                    // its accessible name and selection tag survive column content
                    // changes, and selection is identified by entity ID.
                    keyed(
                        row.id,
                        ListBoxItem::new()
                            .automation_id(self.row_automation_id(row.id))
                            .automation_name(description)
                            .tag(row.id.to_string())
                            .is_selected(self.selected == Some(row.id))
                            .content(if self.selected == Some(row.id) {
                                frame.element_ref(&self.selected_row)
                            } else {
                                frame
                            }),
                    )
                }))
                .into()
        };
        ScrollViewer::new()
            .horizontal_scroll_bar_visibility(ScrollBarVisibility::Auto)
            .vertical_scroll_bar_visibility(ScrollBarVisibility::Disabled)
            .content(
                Grid::new()
                    .width(width + 32.0)
                    .rows([GridLength::Auto, GridLength::STAR])
                    .row_spacing(6.0)
                    .children((headers, Border::new().grid_row(1).content(table))),
            )
            .into()
    }
    fn task_details(&self, id: u64, context: &ViewContext<Self>) -> View {
        let Some(task) = self.snapshot.tasks.iter().find(|t| t.id == id) else {
            return card("Task no longer retained", back(context));
        };
        let mut content = vec![
            back(context),
            title(&format!("{}  /  task {id}", task.name), 26.0).into(),
        ];
        if !task.stats_available {
            content.push(note("Task statistics are incomplete at this position; zero counters are not complete evidence."));
        }
        for evidence in crate::analysis::evidence(task, &self.settings) {
            content.push(note(&format!(
                "{}: observed {}; threshold {}. {}",
                evidence.lint.name(),
                evidence.measured,
                evidence.threshold,
                evidence.explanation
            )));
        }
        content.push(
            Grid::new()
                .columns([GridLength::STAR, GridLength::STAR])
                .column_spacing(16.0)
                .children((
                    card("Task timing", timing_view(&task.timing)),
                    Border::new().grid_column(1).content(card(
                        "Waker activity",
                        StackPanel::new().spacing(10.0).children((
                            metric("Active wakers", task.active_wakers().to_string()),
                            metric(
                                "Clones / drops",
                                format!("{} / {}", task.waker_clones, task.waker_drops),
                            ),
                            metric("Wakes", task.wakes.to_string()),
                            metric(
                                "Self wakes",
                                format!("{} ({:.1}%)", task.self_wakes, task.self_wake_percent()),
                            ),
                            metric(
                                "Last woken",
                                task.last_wake_ago
                                    .map(|n| format!("{} ago", format_ns(n)))
                                    .unwrap_or_else(|| "Never".into()),
                            ),
                        )),
                    )),
                ))
                .into(),
        );
        let details = self.live.snapshot.details.get(&id);
        content.push(
            Grid::new()
                .columns([GridLength::STAR, GridLength::STAR])
                .column_spacing(16.0)
                .children((
                    histogram(
                        "Poll times",
                        details.map(|d| &d.poll),
                        self.settings.no_colors,
                    ),
                    Border::new().grid_column(1).content(histogram(
                        "Scheduled times",
                        details.map(|d| &d.scheduled),
                        self.settings.no_colors,
                    )),
                ))
                .into(),
        );
        content.push(card(
            "Source and span fields",
            StackPanel::new().spacing(8.0).children((
                selectable(&format!(
                    "{}  /  {:?}  /  {}",
                    task.target,
                    task.state,
                    if task.blocking { "blocking" } else { "async" }
                )),
                selectable(&task.location),
                selectable(&task.fields),
                selectable(&format!(
                    "Future size: {} bytes  /  original: {} bytes",
                    task.size_bytes
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "unknown".into()),
                    task.original_size_bytes
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "unknown".into())
                )),
            )),
        ));
        ScrollViewer::new()
            .content(StackPanel::new().spacing(16.0).children(content))
            .into()
    }
    fn resource_details(&self, id: u64, context: &ViewContext<Self>) -> View {
        let Some(resource) = self.snapshot.resources.iter().find(|r| r.id == id) else {
            return card("Resource no longer retained", back(context));
        };
        Grid::new()
            .rows([GridLength::Auto, GridLength::Auto, GridLength::STAR])
            .row_spacing(12.0)
            .children((
                StackPanel::new().spacing(10.0).children((
                    back(context),
                    title(
                        &format!("{}  /  resource {id}", resource.concrete_type),
                        26.0,
                    ),
                    selectable(&format!(
                        "{}  /  {}  /  {}  /  {}  /  parent {}",
                        resource.kind,
                        resource.target,
                        if resource.internal { "INT" } else { "PUB" },
                        format_ns(resource.total),
                        resource
                            .parent
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "none".into())
                    )),
                    selectable(&resource.location),
                    selectable(&resource.attributes),
                )),
                Grid::new()
                    .grid_row(1)
                    .columns([GridLength::STAR, GridLength::Auto])
                    .column_spacing(12.0)
                    .children((
                        self.search_box(context),
                        Button::new()
                            .grid_column(1)
                            .content("Inspect operation's task")
                            .is_enabled(self.selected.is_some())
                            .on_click(context.message(Message::Inspect)),
                    )),
                Border::new().grid_row(2).content(self.table(context)),
            ))
            .into()
    }
    fn settings_view(&self, context: &ViewContext<Self>) -> View {
        let lints = Lint::ALL
            .into_iter()
            .map(|lint| {
                CheckBox::new()
                    .content(lint.name())
                    .is_checked(self.settings.warnings.contains(&lint))
                    .on_is_checked_changed(context.callback(move |value: Option<bool>| {
                        Message::Warn(lint, value.unwrap_or(false))
                    }))
                    .into()
            })
            .collect::<Vec<View>>();
        ScrollViewer::new().content(StackPanel::new().spacing(18.0).children((
            title("Settings", 28.0),
            card("Runtime warnings", StackPanel::new().spacing(10.0).children(lints)),
            card("Appearance", StackPanel::new().orientation(Orientation::Horizontal).spacing(10.0).children(
                [Palette::System, Palette::Dark, Palette::Light].into_iter().map(|palette| Button::new().content(format!("{palette:?}"))
                    .on_click(context.message(Message::Palette(palette))).into()).collect::<Vec<View>>())),
            card("Retention and recording", StackPanel::new().spacing(8.0).children((
                format!("Completed data retention: {}. Change it with --retain-for and restart.", self.settings.retain_for),
                "Use --record session.tcui to capture telemetry. Use --replay session.tcui for offline inspection.",
                "Task histograms are recorded while that task's details are open.",
            ))),
            Button::new().content("Save application preferences").on_click(context.message(Message::Save)),
        ))).into()
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SortValue {
    Number(u64),
    Text(String),
}
struct Cell {
    text: String,
    order: SortValue,
}
impl Cell {
    fn number(value: u64) -> Self {
        Self {
            text: value.to_string(),
            order: SortValue::Number(value),
        }
    }
    fn duration(value: u64) -> Self {
        Self {
            text: format_ns(value),
            order: SortValue::Number(value),
        }
    }
    fn text(value: impl AsRef<str>) -> Self {
        Self {
            text: value.as_ref().into(),
            order: SortValue::Text(value.as_ref().to_lowercase()),
        }
    }
}
struct Row {
    id: u64,
    cells: Vec<Cell>,
}

fn title(text: &str, size: f64) -> TextBlock {
    TextBlock::new()
        .text(text)
        .font_size(size)
        .font_weight(FontWeight::SEMI_BOLD)
}
fn selectable(text: &str) -> TextBlock {
    TextBlock::new()
        .text(text)
        .text_wrapping(TextWrapping::Wrap)
        .is_text_selection_enabled(true)
}
fn note(text: &str) -> View {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(6.0)
        .padding(12.0)
        .content(selectable(text))
        .into()
}
fn card(name: &str, content: impl Into<View>) -> View {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(8.0)
        .padding(18.0)
        .content(
            StackPanel::new()
                .spacing(14.0)
                .children((title(name, 16.0), content.into())),
        )
        .into()
}
fn metric(label: &str, value: String) -> View {
    Grid::new()
        .columns([GridLength::STAR, GridLength::Auto])
        .children((
            TextBlock::new().text(label).opacity(0.75),
            TextBlock::new()
                .grid_column(1)
                .text(value)
                .font_family("Consolas"),
        ))
        .into()
}
fn back(context: &ViewContext<Console>) -> View {
    Button::new()
        .content("Back  /  Esc")
        .on_click(context.message(Message::Back))
        .into()
}
fn timing_view(timing: &Timing) -> View {
    StackPanel::new()
        .spacing(10.0)
        .children((
            metric("Total", format_ns(timing.total)),
            metric(
                "Busy",
                format!(
                    "{}  ({:.1}%)",
                    format_ns(timing.busy),
                    percent(timing.busy, timing.total)
                ),
            ),
            metric(
                "Scheduled",
                format!(
                    "{}  ({:.1}%)",
                    format_ns(timing.scheduled),
                    percent(timing.scheduled, timing.total)
                ),
            ),
            metric(
                "Idle",
                format!(
                    "{}  ({:.1}%)",
                    format_ns(timing.idle),
                    percent(timing.idle, timing.total)
                ),
            ),
            metric("Polls", timing.polls.to_string()),
        ))
        .into()
}
fn histogram(name: &str, distribution: Option<&Distribution>, no_colors: bool) -> View {
    let Some(d) = distribution else {
        return card(
            name,
            "Waiting for task details. Offline recordings contain histograms only for inspected tasks.",
        );
    };
    if let Some(error) = &d.error {
        return card(name, selectable(error));
    }
    if d.count == 0 {
        return card(
            name,
            if d.high_outliers == 0 {
                "No latency samples have been recorded yet.".into()
            } else {
                format!(
                    "No samples within the histogram range. {} high outliers; highest {}.",
                    d.high_outliers,
                    d.highest_outlier
                        .map(format_ns)
                        .unwrap_or_else(|| "unknown".into())
                )
            },
        );
    }
    let mut children = vec![metric(
        "Samples / min / max",
        format!("{} / {} / {}", d.count, format_ns(d.min), format_ns(d.max)),
    )];
    children.push(
        selectable(
            &["p10", "p25", "p50", "p75", "p90", "p95", "p99"]
                .into_iter()
                .zip(&d.percentiles)
                .map(|(label, value)| format!("{label} {}", format_ns(*value)))
                .collect::<Vec<_>>()
                .join("    "),
        )
        .into(),
    );
    let peak = d.bins.iter().map(|b| b.count).max().unwrap_or(1).max(1);
    for bin in &d.bins {
        children.push(
            Grid::new()
                .columns([
                    GridLength::Pixel(90.0),
                    GridLength::STAR,
                    GridLength::Pixel(70.0),
                ])
                .column_spacing(8.0)
                .children((
                    TextBlock::new()
                        .text(format!("<= {}", format_ns(bin.upper_ns)))
                        .font_size(11.0),
                    Border::new()
                        .grid_column(1)
                        .background(ThemeBrush::CardStroke)
                        .height(12.0)
                        .vertical_alignment(VerticalAlignment::Center)
                        .content(
                            Border::new()
                                .background(if no_colors {
                                    ThemeBrush::PrimaryText
                                } else {
                                    ThemeBrush::Accent
                                })
                                .width((bin.count as f64 / peak as f64 * 180.0).max(1.0))
                                .height(12.0)
                                .horizontal_alignment(HorizontalAlignment::Left),
                        ),
                    TextBlock::new()
                        .grid_column(2)
                        .text(bin.count.to_string())
                        .font_size(11.0),
                ))
                .into(),
        );
    }
    if d.high_outliers > 0 {
        children.push(note(&format!(
            "{} high outliers; highest {}. Percentiles show samples within the histogram range.",
            d.high_outliers,
            d.highest_outlier
                .map(format_ns)
                .unwrap_or_else(|| "unknown".into())
        )));
    }
    card(name, StackPanel::new().spacing(8.0).children(children))
}
fn help_view() -> View {
    ScrollViewer::new().content(StackPanel::new().spacing(16.0).children((
        title("Connect an application", 28.0),
        card("1. Enable instrumentation", selectable("Add console-subscriber = \"0.5\" and tokio with features [\"full\", \"tracing\"].\nCompile with RUSTFLAGS=\"--cfg tokio_unstable\". This project's .cargo/config.toml already enables it.\nCall console_subscriber::init() before building the Tokio runtime.\nFor existing tracing registries, attach console_subscriber::spawn() as a layer.")),
        card("2. Start streaming", selectable("Run cargo run --example instrumented in a terminal, then connect to http://127.0.0.1:6669.\nTOKIO_CONSOLE_BIND changes the subscriber's bind address. TOKIO_CONSOLE_RETENTION changes server retention.\nThe subscriber and client retention settings are independent. HTTPS endpoints require a trusted server certificate.")),
        card("3. Inspect and diagnose", selectable("Tasks: sort by Busy for CPU work, Sched for executor saturation, or Idle for external waits.\nSelect a row and choose Inspect selected (or press Enter). The details page shows wakers, source fields, and latency distributions.\nResources: inspect a mutex, timer, or semaphore to see its async operations and waiting tasks.\nPause uses the server's Pause/Resume RPCs and can affect other observers of the same target.")),
        card("Keyboard", selectable("t Tasks / r Resources / j,k or arrows Move / gg or Home First / G or End Last\nh,l or left,right Sort column / i Invert / Enter Inspect / Esc Back / Space Pause or resume / q Quit\nShortcuts are suspended while editing a text field. Tab and Shift+Tab navigate native controls.")),
        card("Recordings and custom runtimes", selectable("--record capture.tcui stores console-api updates and inspected-task histograms. --replay capture.tcui opens the final retained snapshot.\nThis is a versioned protobuf recording, distinct from TOKIO_CONSOLE_RECORD_PATH raw tracing events.\nAny producer implementing console-api's Instrument service can connect; no Tokio dependency is required in the producer.")),
    ))).into()
}

pub fn run(
    settings: Settings,
    record: Option<PathBuf>,
    replay: Option<PathBuf>,
    policy: Option<PathBuf>,
) -> Result<()> {
    crate::windows_workbench::run(settings, record, replay, policy)
}
pub fn show_error(error: &str) {
    let text: Vec<u16> = error.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Console UI".encode_utf16().chain(Some(0)).collect();
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            0x10,
        );
    }
}
