//! Native GTK4 widgets driven by a Relm4 component and the shared Rust controller.
use clap::Parser;
use console_ui::{
    config::{LintThresholds, Palette},
    model::{ResourceRow, Snapshot, TaskRow},
    sharing::RedactionPolicy,
    workspace::{Action, Workspace},
};
use gtk::{glib, prelude::*};
use relm4::{ComponentParts, ComponentSender, RelmApp, RelmWidgetExt, SimpleComponent, gtk};
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

struct App {
    workspace: Workspace,
    revision: u64,
    smoke: bool,
    smoke_step: u8,
    started: Instant,
    done: bool,
    closing: bool,
}
#[derive(Debug)]
enum Msg {
    Tick,
    Shutdown,
    Action(Action),
    Select { kind: String, id: u64, session: u64 },
    InspectTask(u64),
    Error(String),
}
struct Widgets {
    window: gtk::ApplicationWindow,
    status: gtk::Label,
    error: gtk::Label,
    tabs: gtk::Box,
    tasks: gtk::ListBox,
    resources: gtk::ListBox,
    operations: gtk::ListBox,
    details: gtk::Box,
    analysis: gtk::Label,
    charts: gtk::Box,
    export_preview: gtk::Label,
    lookup: gtk::Box,
    sessions: gtk::Box,
    bookmarks: gtk::Box,
    profiles: gtk::Box,
    presets: gtk::Box,
    views: gtk::Box,
    endpoint: gtk::Entry,
    filter: gtk::Entry,
    sort: gtk::DropDown,
    updating: Rc<Cell<bool>>,
    last_key: String,
    last_revision: u64,
}

impl SimpleComponent for App {
    type Init = (Workspace, bool);
    type Input = Msg;
    type Output = ();
    type Root = gtk::ApplicationWindow;
    type Widgets = Widgets;
    fn init_root() -> Self::Root {
        gtk::ApplicationWindow::builder()
            .title("Console UI — GTK runtime diagnostics")
            .default_width(1440)
            .default_height(920)
            .build()
    }
    fn init(
        (workspace, smoke): Self::Init,
        window: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let updating = Rc::new(Cell::new(false));
        let outer = vertical();
        outer.set_margin_all(12);
        window.set_child(Some(&outer));
        {
            let sender = sender.clone();
            window.connect_close_request(move |_| {
                sender.input(Msg::Shutdown);
                glib::Propagation::Stop
            });
        }
        let title = gtk::Label::new(Some("Runtime observatory"));
        title.add_css_class("title-1");
        title.set_halign(gtk::Align::Start);
        outer.append(&title);
        let status = label("Connecting…");
        outer.append(&status);
        let error = label("");
        outer.append(&error);
        let tabs = horizontal();
        outer.append(&tabs);
        let endpoint = gtk::Entry::builder()
            .placeholder_text("http://127.0.0.1:6669")
            .hexpand(true)
            .build();
        endpoint.set_text(&workspace.settings.target);
        let connection = horizontal();
        connection.append(&endpoint);
        let source = endpoint.clone();
        connect_button(&connection, "Connect", sender.clone(), move || {
            Action::Connect {
                target: source.text().into(),
            }
        });
        connect_button(&connection, "Disconnect", sender.clone(), || {
            Action::Disconnect
        });
        connect_button(&connection, "Pause / resume", sender.clone(), || {
            Action::Pause
        });
        connect_button(&connection, "Open source", sender.clone(), || {
            Action::OpenSource
        });
        outer.append(&connection);
        let filter = gtk::Entry::builder()
            .placeholder_text("Filter: name:worker busy>5ms !state:completed | warning:self-wakes")
            .hexpand(true)
            .build();
        let bar = horizontal();
        bar.append(&filter);
        let query_entry = filter.clone();
        connect_button(&bar, "Apply filter", sender.clone(), move || {
            Action::Filter {
                expression: query_entry.text().into(),
            }
        });
        connect_button(&bar, "Clear", sender.clone(), || Action::Filter {
            expression: String::new(),
        });
        let sort = gtk::DropDown::from_strings(&[
            "Busy",
            "Scheduled",
            "Idle",
            "Polls",
            "Total",
            "Name",
            "ID",
        ]);
        bar.append(&sort);
        outer.append(&bar);
        {
            let sender = sender.clone();
            let guard = updating.clone();
            sort.connect_selected_notify(move |_| {
                if !guard.get() {
                    sender.input(Msg::Tick);
                }
            });
        }
        let notebook = gtk::Notebook::new();
        notebook.set_hexpand(true);
        notebook.set_vexpand(true);
        let tasks = list();
        let resources = list();
        let operations = list();
        notebook.append_page(&scrolled(&tasks), Some(&gtk::Label::new(Some("Tasks"))));
        notebook.append_page(
            &scrolled(&resources),
            Some(&gtk::Label::new(Some("Resources"))),
        );
        notebook.append_page(
            &scrolled(&operations),
            Some(&gtk::Label::new(Some("Async operations"))),
        );
        let details = vertical();
        details.set_size_request(410, -1);
        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_start_child(Some(&notebook));
        split.set_end_child(Some(&scrolled(&details)));
        split.set_position(880);
        outer.append(&split);
        for (kind, list) in [
            ("task", &tasks),
            ("resource", &resources),
            ("operation", &operations),
        ] {
            let sender = sender.clone();
            let guard = updating.clone();
            list.connect_row_selected(move |_, row| {
                if guard.get() {
                    return;
                }
                if let Some(row) = row {
                    let name = row.widget_name();
                    let parts: Vec<_> = name.split(':').collect();
                    if parts.len() == 3
                        && let (Ok(id), Ok(session)) = (parts[1].parse(), parts[2].parse())
                    {
                        sender.input(Msg::Select {
                            kind: kind.into(),
                            id,
                            session,
                        });
                    }
                }
            });
        }
        let analysis = label("");
        let analysis_page = vertical();
        analysis_page.append(&analysis);
        let charts = vertical();
        analysis_page.append(&charts);
        let range = entry("Analysis window (60s, 5min, none)", "60s");
        analysis_page.append(&range);
        {
            let sender = sender.clone();
            let button = gtk::Button::with_label("Apply analysis window");
            button.connect_clicked(move |_| {
                dispatch_result(
                    &sender,
                    (|| -> anyhow::Result<Action> {
                        let text = range.text();
                        Ok(Action::Window {
                            nanoseconds: if text == "none" {
                                None
                            } else {
                                Some(humantime::parse_duration(&text)?.as_nanos().try_into()?)
                            },
                        })
                    })(),
                );
            });
            analysis_page.append(&button);
        }

        let thresholds = horizontal();
        let wake = entry(
            "Self-wake %",
            &workspace
                .settings
                .lint_thresholds
                .self_wake_percent
                .to_string(),
        );
        let never = entry("Never-yielded duration", "1s");
        let size = entry("Large future bytes", "1024");
        thresholds.append(&wake);
        thresholds.append(&never);
        thresholds.append(&size);
        {
            let sender = sender.clone();
            let button = gtk::Button::with_label("Apply thresholds");
            button.connect_clicked(move |_| {
                let action = (|| -> anyhow::Result<Action> {
                    Ok(Action::Thresholds {
                        thresholds: LintThresholds {
                            self_wake_percent: wake.text().parse()?,
                            never_yielded_ns: humantime::parse_duration(&never.text())?
                                .as_nanos()
                                .try_into()?,
                            large_future_bytes: size.text().parse()?,
                        },
                    })
                })();
                dispatch_result(&sender, action);
            });
            thresholds.append(&button);
        }
        analysis_page.append(&thresholds);
        let lints = horizontal();
        for lint in console_ui::config::Lint::ALL {
            let check = gtk::CheckButton::with_label(lint.name());
            check.set_active(workspace.settings.warnings.contains(&lint));
            let sender = sender.clone();
            check.connect_toggled(move |check| {
                sender.input(Msg::Action(Action::LintEnabled {
                    lint,
                    enabled: check.is_active(),
                }))
            });
            lints.append(&check);
        }
        analysis_page.append(&lints);
        notebook.append_page(
            &scrolled(&analysis_page),
            Some(&gtk::Label::new(Some("Analysis"))),
        );
        let recording_page = vertical();
        let capture = horizontal();
        picker_button(
            &capture,
            "Start recording…",
            &window,
            sender.clone(),
            Action::StartRecording {
                path: PathBuf::new(),
            },
            true,
        );
        connect_button(&capture, "Stop recording", sender.clone(), || {
            Action::StopRecording
        });
        picker_button(
            &capture,
            "Open recording…",
            &window,
            sender.clone(),
            Action::OpenRecording {
                path: PathBuf::new(),
                recover: false,
            },
            false,
        );
        recording_page.append(&capture);
        let playback = horizontal();
        connect_button(&playback, "Play / pause recording", sender.clone(), || {
            Action::Playback
        });
        for speed in [0.5, 1.0, 2.0, 4.0] {
            connect_button(
                &playback,
                &format!("{speed}×"),
                sender.clone(),
                move || Action::Speed { speed },
            );
        }
        recording_page.append(&playback);
        let seek = horizontal();
        let offset = entry("Producer timestamp (ns)", "");
        seek.append(&offset);
        {
            let sender = sender.clone();
            let button = gtk::Button::with_label("Seek");
            button.connect_clicked(move |_| {
                dispatch_result(
                    &sender,
                    offset
                        .text()
                        .parse::<u64>()
                        .map(|cursor| Action::Seek { cursor })
                        .map_err(Into::into),
                )
            });
            seek.append(&button);
        }
        recording_page.append(&seek);
        let sessions = vertical();
        recording_page.append(&sessions);
        let bookmark_row = horizontal();
        let bookmark_name = entry("Bookmark name", "");
        let note = entry("Note", "");
        bookmark_row.append(&bookmark_name);
        bookmark_row.append(&note);
        connect_button(
            &bookmark_row,
            "Bookmark cursor",
            sender.clone(),
            move || Action::Bookmark {
                name: bookmark_name.text().into(),
                note: note.text().into(),
            },
        );
        recording_page.append(&bookmark_row);
        let bookmarks = vertical();
        recording_page.append(&bookmarks);
        let capture_policy = vertical();
        capture_policy.append(&label(
            "Histogram capture: inspected, selected IDs, or matching query (at most 32 streams)",
        ));
        let mode = entry("Capture mode", "inspected");
        let ids = entry("Task IDs, comma separated", "");
        let query = entry("Matching filter", "");
        let limit = entry("Maximum streams", "4");
        let rotation = entry("Rotation seconds", "30");
        for input in [&mode, &ids, &query, &limit, &rotation] {
            capture_policy.append(input);
        }
        {
            let sender = sender.clone();
            let button = gtk::Button::with_label("Apply histogram policy");
            button.connect_clicked(move |_| {
                dispatch_result(
                    &sender,
                    (|| -> anyhow::Result<Action> {
                        Ok(Action::HistogramPolicy {
                            policy: console_ui::telemetry::HistogramPolicy {
                                mode: mode.text().into(),
                                tasks: ids
                                    .text()
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::parse)
                                    .collect::<Result<_, _>>()?,
                                query: query.text().into(),
                                max_streams: limit.text().parse()?,
                                rotate_seconds: rotation.text().parse()?,
                                collect_without_recording: false,
                            },
                        })
                    })(),
                );
            });
            capture_policy.append(&button);
        }
        recording_page.append(&capture_policy);

        notebook.append_page(
            &scrolled(&recording_page),
            Some(&gtk::Label::new(Some("Recording"))),
        );
        let connections = vertical();
        let name = entry("Profile name", "");
        connections.append(&name);
        let target = endpoint.clone();
        let profile_name = name.clone();
        connect_button(
            &connections,
            "Save connection profile",
            sender.clone(),
            move || Action::SaveProfile {
                name: profile_name.text().into(),
                target: target.text().into(),
            },
        );
        let target = endpoint.clone();
        let profile_name = name.clone();
        connect_button(
            &connections,
            "Open target in new tab",
            sender.clone(),
            move || Action::AddTarget {
                name: profile_name.text().into(),
                target: target.text().into(),
            },
        );
        let profiles = vertical();
        connections.append(&profiles);
        let preset_name = name.clone();
        connect_button(
            &connections,
            "Save current filter preset",
            sender.clone(),
            move || Action::SavePreset {
                name: preset_name.text().into(),
            },
        );
        let preset_name = name.clone();
        connect_button(
            &connections,
            "Save filter preset in project",
            sender.clone(),
            move || Action::SaveProjectPreset {
                name: preset_name.text().into(),
            },
        );
        let rename = entry("New profile name", "");
        connections.append(&rename);
        {
            let old = name.clone();
            connect_button(&connections, "Rename profile", sender.clone(), move || {
                Action::RenameProfile {
                    name: old.text().into(),
                    new_name: rename.text().into(),
                }
            });
        }
        let lookup_entry = entry("Find task by name or ID", "");
        connections.append(&lookup_entry);
        {
            let sender = sender.clone();
            lookup_entry.connect_changed(move |entry| {
                sender.input(Msg::Action(Action::Lookup {
                    text: entry.text().into(),
                }))
            });
        }
        let lookup = vertical();
        connections.append(&lookup);
        let old_preset = entry("Preset to rename", "");
        let new_preset = entry("New preset name", "");
        connections.append(&old_preset);
        connections.append(&new_preset);
        connect_button(
            &connections,
            "Rename global preset",
            sender.clone(),
            move || Action::RenamePreset {
                name: old_preset.text().into(),
                new_name: new_preset.text().into(),
                project: false,
            },
        );
        let presets = vertical();
        connections.append(&presets);
        notebook.append_page(
            &scrolled(&connections),
            Some(&gtk::Label::new(Some("Connections"))),
        );
        let project = vertical();
        let appearance = gtk::DropDown::from_strings(&["System appearance", "Light", "Dark"]);
        appearance.set_selected(match workspace.settings.palette {
            Palette::System => 0,
            Palette::Light => 1,
            Palette::Dark => 2,
        });
        {
            let sender = sender.clone();
            appearance.connect_selected_notify(move |selection| {
                sender.input(Msg::Action(Action::Appearance {
                    palette: match selection.selected() {
                        1 => Palette::Light,
                        2 => Palette::Dark,
                        _ => Palette::System,
                    },
                }))
            });
        }
        project.append(&appearance);
        let name = entry("Workspace / view name", "My investigation");
        project.append(&name);
        let original = entry("Existing view to rename or duplicate", "");
        project.append(&original);
        {
            let old = original.clone();
            let next = name.clone();
            connect_button(&project, "Rename view", sender.clone(), move || {
                Action::RenameView {
                    name: old.text().into(),
                    new_name: next.text().into(),
                }
            });
        }
        {
            let old = original.clone();
            let next = name.clone();
            connect_button(&project, "Duplicate view", sender.clone(), move || {
                Action::DuplicateView {
                    name: old.text().into(),
                    new_name: next.text().into(),
                }
            });
        }
        let source = name.clone();
        connect_button(
            &project,
            "Save investigation view",
            sender.clone(),
            move || Action::SaveView {
                name: source.text().into(),
            },
        );
        picker_button(
            &project,
            "Save workspace…",
            &window,
            sender.clone(),
            Action::SaveWorkspace {
                path: PathBuf::new(),
                name: "Project workspace".into(),
            },
            true,
        );
        picker_button(
            &project,
            "Open workspace…",
            &window,
            sender.clone(),
            Action::OpenWorkspace {
                path: PathBuf::new(),
            },
            false,
        );
        picker_button(
            &project,
            "Import project policy…",
            &window,
            sender.clone(),
            Action::LoadPolicy {
                path: PathBuf::new(),
            },
            false,
        );
        picker_button(
            &project,
            "Export current project policy…",
            &window,
            sender.clone(),
            Action::SavePolicy {
                path: PathBuf::new(),
            },
            true,
        );
        let views = vertical();
        project.append(&views);
        let editor_program = entry("Editor executable", &workspace.preferences.editor.program);
        let editor_args = entry("Editor arguments", "--goto '{file}:{line}:{column}'");
        let remote = entry("Remote source root", "");
        let local = entry("Local source root", "");
        for e in [&editor_program, &editor_args, &remote, &local] {
            project.append(e);
        }
        let editor = workspace.preferences.editor.clone();
        {
            let sender = sender.clone();
            let button = gtk::Button::with_label("Save source-editor integration");
            button.connect_clicked(move |_| {
                let result = (|| -> anyhow::Result<Action> {
                    let mut preferences = editor.clone();
                    preferences.program = editor_program.text().into();
                    preferences.arguments = shlex_args(&editor_args.text())?;
                    if !remote.text().is_empty() {
                        preferences
                            .mappings
                            .push(console_ui::preferences::PathMapping {
                                remote_root: remote.text().into(),
                                local_root: local.text().as_str().into(),
                            });
                    }
                    Ok(Action::Editor { preferences })
                })();
                dispatch_result(&sender, result);
            });
            project.append(&button);
        }
        notebook.append_page(
            &scrolled(&project),
            Some(&gtk::Label::new(Some("Workspace"))),
        );
        let export = vertical();
        export.append(&label(
            "Exports use the current filtered snapshot. Redaction preserves the original capture.",
        ));
        let fields = entry("Redact field keys, comma separated", "");
        let paths = gtk::CheckButton::with_label("Redact source paths");
        let endpoints = gtk::CheckButton::with_label("Redact endpoint details");
        export.append(&fields);
        export.append(&paths);
        export.append(&endpoints);
        let sections = entry(
            "Report sections: charts,tasks,resources,comparison",
            "charts,tasks,resources,comparison",
        );
        export.append(&sections);
        {
            let source = sections.clone();
            connect_button(
                &export,
                "Apply report sections",
                sender.clone(),
                move || Action::ReportSections {
                    sections: source
                        .text()
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                },
            );
        }
        {
            let fields = fields.clone();
            let paths = paths.clone();
            let endpoints = endpoints.clone();
            connect_button(
                &export,
                "Preview redacted export",
                sender.clone(),
                move || Action::Preview {
                    policy: redaction(&fields, &paths, &endpoints),
                },
            );
        }
        let export_preview = label("Preview scope and fields before exporting.");
        export.append(&export_preview);
        for (format, table, title) in [
            ("csv", "tasks", "Export tasks CSV…"),
            ("csv", "resources", "Export resources CSV…"),
            ("csv", "operations", "Export operations CSV…"),
            ("json", "tasks", "Export JSON…"),
            ("html", "tasks", "Export report…"),
            ("svg", "polls", "Export polling chart…"),
            ("svg", "busy", "Export busy chart…"),
            ("svg", "scheduled", "Export scheduling chart…"),
            ("recording", "tasks", "Save redacted capture copy…"),
        ] {
            let fields = fields.clone();
            let paths = paths.clone();
            let endpoints = endpoints.clone();
            picker_dynamic(
                &export,
                title,
                &window,
                sender.clone(),
                move || {
                    let policy = redaction(&fields, &paths, &endpoints);
                    Ok(if format == "recording" {
                        Action::RedactRecording {
                            path: PathBuf::new(),
                            policy,
                        }
                    } else {
                        Action::Export {
                            path: PathBuf::new(),
                            format: format.into(),
                            table: table.into(),
                            redaction: policy,
                        }
                    })
                },
                true,
            );
        }
        picker_button(
            &export,
            "Compare with baseline…",
            &window,
            sender.clone(),
            Action::Compare {
                path: PathBuf::new(),
                session: 0,
                policy: Default::default(),
            },
            false,
        );
        notebook.append_page(
            &scrolled(&export),
            Some(&gtk::Label::new(Some("Compare / export"))),
        );
        let shortcuts = gtk::EventControllerKey::new();
        {
            let sender = sender.clone();
            let filter = filter.clone();
            let lookup_entry = lookup_entry.clone();
            let notebook = notebook.clone();
            let endpoint = endpoint.clone();
            shortcuts.connect_key_pressed(move |_, key, _, modifiers| {
                if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                    && (key == gtk::gdk::Key::f || key == gtk::gdk::Key::k)
                {
                    if key == gtk::gdk::Key::k {
                        notebook.set_current_page(Some(5));
                        lookup_entry.grab_focus();
                    } else {
                        filter.grab_focus();
                    }
                    return glib::Propagation::Stop;
                }
                if key == gtk::gdk::Key::F5 {
                    sender.input(Msg::Action(Action::Connect {
                        target: endpoint.text().into(),
                    }));
                }
                glib::Propagation::Proceed
            });
        }
        window.add_controller(shortcuts);
        {
            let sender = sender.clone();
            glib::timeout_add_local(Duration::from_millis(500), move || {
                sender.input(Msg::Tick);
                glib::ControlFlow::Continue
            });
        }
        ComponentParts {
            model: App {
                workspace,
                revision: 0,
                smoke,
                smoke_step: 0,
                started: Instant::now(),
                done: false,
                closing: false,
            },
            widgets: Widgets {
                window,
                status,
                error,
                tabs,
                tasks,
                resources,
                operations,
                details,
                analysis,
                charts,
                export_preview,
                lookup,
                sessions,
                bookmarks,
                profiles,
                presets,
                views,
                endpoint,
                filter,
                sort,
                updating,
                last_key: String::new(),
                last_revision: u64::MAX,
            },
        }
    }
    fn update(&mut self, message: Msg, _: ComponentSender<Self>) {
        let result = match message {
            Msg::Tick => self.workspace.tick(),
            Msg::Shutdown => {
                self.workspace.shutdown();
                self.closing = true;
                Ok(())
            }
            Msg::Action(action) => {
                self.revision += 1;
                self.workspace.dispatch(action)
            }
            Msg::Select { kind, id, session } => {
                self.revision += 1;
                if session == self.workspace.current().worker.read().session {
                    self.workspace.dispatch(Action::Select { kind, id })
                } else {
                    Ok(())
                }
            }
            Msg::InspectTask(id) => {
                self.revision += 1;
                self.workspace.dispatch(Action::Select {
                    kind: "task".into(),
                    id,
                })
            }
            Msg::Error(error) => {
                self.workspace.notice = Some(error);
                Ok(())
            }
        };
        if let Err(e) = result {
            self.workspace.notice = Some(format!("{e:#}"));
        }
        if self.smoke {
            let live = self.workspace.current().worker.read();
            match self.smoke_step {
                0 if !live.snapshot.tasks.is_empty() && !live.snapshot.resources.is_empty() => {
                    let id = live
                        .snapshot
                        .tasks
                        .iter()
                        .find(|t| t.name == "worker-0")
                        .unwrap_or(&live.snapshot.tasks[0])
                        .id;
                    let _ = self.workspace.dispatch(Action::Select {
                        kind: "task".into(),
                        id,
                    });
                    self.smoke_step = 1;
                }
                1 if !live.snapshot.details.is_empty() => {
                    let _ = self.workspace.dispatch(Action::Pause);
                    self.smoke_step = 2;
                }
                2 if live.paused => {
                    let _ = self.workspace.dispatch(Action::Pause);
                    self.smoke_step = 3;
                }
                3 if !live.paused => {
                    self.done = true;
                }
                _ => {}
            }
            if self.started.elapsed() > Duration::from_secs(40) {
                eprintln!("GTK smoke timed out: {:?}", live.error);
                std::process::exit(1);
            }
        }
    }
    fn update_view(&self, w: &mut Widgets, sender: ComponentSender<Self>) {
        if self.closing {
            if let Some(app) = w.window.application() {
                app.quit();
            }
            return;
        }
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(
                self.workspace.settings.palette == Palette::Dark,
            );
        }
        let view = match self.workspace.view() {
            Ok(v) => v,
            Err(e) => {
                w.error.set_text(&e.to_string());
                return;
            }
        };
        w.status.set_text(&format!(
            "{:?}{} · {} · {} tasks / {} resources / {} operations{}",
            view.status,
            if view.paused { " / PAUSED" } else { "" },
            view.target,
            view.unfiltered_counts[0],
            view.unfiltered_counts[1],
            view.unfiltered_counts[2],
            view.recording
                .as_ref()
                .map(|p| format!(" · Recording {}", p.display()))
                .unwrap_or_default()
        ));
        w.error.set_text(view.notice.as_deref().unwrap_or(""));
        let key = format!(
            "{}:{}:{}:{}:{}:{}:{:?}",
            view.active,
            view.session,
            view.snapshot.now,
            view.query,
            w.sort.selected(),
            view.target,
            view.status
        );
        if w.last_key != key || w.last_revision != self.revision {
            w.updating.set(true);
            w.last_key = key;
            w.last_revision = self.revision;
            if !w.endpoint.has_focus() {
                w.endpoint.set_text(&view.target);
            }
            if !w.filter.has_focus() {
                w.filter.set_text(&view.query);
            }
            clear(&w.tabs);
            for tab in &view.tabs {
                let row = horizontal();
                connect_button(
                    &row,
                    &format!(
                        "{}{}",
                        tab.name,
                        if tab.id == view.active { " ●" } else { "" }
                    ),
                    sender.clone(),
                    {
                        let id = tab.id;
                        move || Action::SelectTab { id }
                    },
                );
                if view.tabs.len() > 1 {
                    connect_button(&row, "×", sender.clone(), {
                        let id = tab.id;
                        move || Action::CloseTab { id }
                    });
                }
                w.tabs.append(&row);
            }
            clear_list(&w.tasks);
            let mut tasks = view.snapshot.tasks.clone();
            tasks.sort_by(|a, b| task_order(a, b, w.sort.selected()));
            for task in tasks.iter().take(1000) {
                add_row(
                    &w.tasks,
                    "task",
                    task.id,
                    view.session,
                    &format!(
                        "{}  {}  {:?}  | busy {}  sched {}  idle {}  | {} polls  | {}",
                        task.id,
                        task.name,
                        task.state,
                        ns(task.timing.busy),
                        ns(task.timing.scheduled),
                        ns(task.timing.idle),
                        task.timing.polls,
                        task.location
                    ),
                );
            }
            clear_list(&w.resources);
            let mut resources = view.snapshot.resources.clone();
            resources.sort_by_key(|r| std::cmp::Reverse(r.total));
            for r in resources.iter().take(1000) {
                add_row(
                    &w.resources,
                    "resource",
                    r.id,
                    view.session,
                    &format!(
                        "{}  {}  {}  | {}  | {} | {}",
                        r.id,
                        r.concrete_type,
                        r.kind,
                        ns(r.total),
                        if r.internal { "INT" } else { "PUB" },
                        r.location
                    ),
                );
            }
            clear_list(&w.operations);
            for op in view.snapshot.async_ops.iter().take(1000) {
                add_row(
                    &w.operations,
                    "operation",
                    op.id,
                    view.session,
                    &format!(
                        "{}  task {} {}  | {}  | busy {} idle {} | {} polls | {}",
                        op.id,
                        op.task_id.unwrap_or(0),
                        op.task_name,
                        op.source,
                        ns(op.timing.busy),
                        ns(op.timing.idle),
                        op.timing.polls,
                        op.attributes
                    ),
                );
            }
            for (kind, list) in [
                ("task", &w.tasks),
                ("resource", &w.resources),
                ("operation", &w.operations),
            ] {
                if let Some(selection) = &view.selection
                    && selection.kind == kind
                {
                    let mut child = list.first_child();
                    while let Some(widget) = child {
                        let next = widget.next_sibling();
                        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>()
                            && row.widget_name()
                                == format!("{kind}:{}:{}", selection.id, view.session)
                        {
                            list.select_row(Some(&row));
                            break;
                        }
                        child = next;
                    }
                }
            }
            clear(&w.profiles);
            for p in &view.preferences.profiles {
                let target = p.target.clone();
                let name = p.name.clone();
                let row = horizontal();
                connect_button(
                    &row,
                    &format!("{} · {}", p.name, p.target),
                    sender.clone(),
                    move || Action::Connect {
                        target: target.clone(),
                    },
                );
                connect_button(&row, "Delete", sender.clone(), move || {
                    Action::DeleteProfile { name: name.clone() }
                });
                w.profiles.append(&row);
            }
            clear(&w.lookup);
            for task in &view.lookup_tasks {
                let id = task.id;
                let session = view.session;
                connect_button(
                    &w.lookup,
                    &format!("{} · {} · {}", task.id, task.name, task.location),
                    sender.clone(),
                    move || Action::SelectInSession {
                        kind: "task".into(),
                        id,
                        session,
                    },
                );
            }
            clear(&w.presets);
            for p in &view.preferences.filter_presets {
                let name = p.name.clone();
                let row = horizontal();
                connect_button(&row, &p.name, sender.clone(), move || Action::ApplyPreset {
                    name: name.clone(),
                });
                let name = p.name.clone();
                let project = view.project_presets.contains(&name);
                connect_button(&row, "Delete", sender.clone(), move || {
                    if project {
                        Action::DeleteProjectPreset { name: name.clone() }
                    } else {
                        Action::DeletePreset { name: name.clone() }
                    }
                });
                w.presets.append(&row);
            }
            clear(&w.sessions);
            for s in &view.sessions {
                let index = s.index;
                connect_button(
                    &w.sessions,
                    &format!(
                        "Session {}: {}–{} ns · {} updates",
                        s.index, s.start, s.end, s.updates
                    ),
                    sender.clone(),
                    move || Action::Session { index },
                );
            }
            clear(&w.bookmarks);
            for b in &view.bookmarks {
                let id = b.id;
                connect_button(
                    &w.bookmarks,
                    &format!("{} · {} ns · {}", b.name, b.cursor, b.note),
                    sender.clone(),
                    move || Action::GoBookmark { id },
                );
            }
            clear(&w.views);
            for v in &view.views {
                let name = v.name.clone();
                connect_button(&w.views, &v.name, sender.clone(), move || {
                    Action::LoadView { name: name.clone() }
                });
            }
            let mut summary = format!(
                "Producer timestamp {} ns\n{}\n\n",
                view.snapshot.now,
                view.window
                    .as_ref()
                    .map(|w| format!("Window: {}–{} ns, complete={}", w.start, w.end, w.complete))
                    .unwrap_or_else(|| {
                        "Lifetime metrics; select an analysis interval in the shared workspace"
                            .into()
                    })
            );
            for evidence in &view.evidence {
                summary.push_str(&format!(
                    "Task {}: {} observed {} vs {}\n{}\n\n",
                    evidence.task,
                    evidence.lint.name(),
                    evidence.measured,
                    evidence.threshold,
                    evidence.explanation
                ));
            }
            for edge in &view.graph.edges {
                summary.push_str(&format!("{} → {} · {}\n", edge.from, edge.to, edge.label));
            }
            if let Some(comparison) = &view.comparison {
                summary.push_str("\nCapture comparison\n");
                for r in &comparison.rows {
                    summary.push_str(&format!(
                        "{}: {} · busy {:.2} → {:.2} ns/s\n",
                        r.name, r.status, r.busy_per_second[0], r.busy_per_second[1]
                    ));
                }
            }
            w.analysis.set_text(&summary);
            clear(&w.charts);
            for metric in ["busy", "scheduled", "polls"] {
                w.charts.append(&trend_chart(&view.trends, metric));
            }
            if let Some(preview) = &view.export_preview {
                w.export_preview
                    .set_text(&serde_json::to_string_pretty(preview).unwrap_or_default());
            }

            w.updating.set(false);
        }
        clear(&w.details);
        if let Some(selection) = &view.selection {
            match selection.kind.as_str() {
                "task" => {
                    if let Some(t) = view.inspection.tasks.iter().find(|t| t.id == selection.id) {
                        task_details(&w.details, t, &view.inspection);
                    }
                }
                "resource" => {
                    if let Some(r) = view
                        .snapshot
                        .resources
                        .iter()
                        .find(|r| r.id == selection.id)
                    {
                        resource_details(&w.details, r, &view.snapshot, sender.clone());
                    }
                }
                _ => {
                    if let Some(op) = view
                        .snapshot
                        .async_ops
                        .iter()
                        .find(|o| o.id == selection.id)
                    {
                        w.details
                            .append(&label(&format!("{}\n{}", op.source, op.attributes)));
                        if let Some(id) = op.task_id {
                            let button = gtk::Button::with_label("Inspect operation's task");
                            let sender = sender.clone();
                            button.connect_clicked(move |_| sender.input(Msg::InspectTask(id)));
                            w.details.append(&button);
                        }
                    }
                }
            }
        } else {
            w.details.append(&label("Select a task or resource to inspect timing, wakers, source fields, and latency histograms. Lists show up to 1,000 matching entities; narrow the filter for larger runtimes."));
        }
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(
                self.workspace.settings.palette == Palette::Dark,
            );
        }
        if self.done {
            // Let GTK allocate and paint the updated native tree before capturing it.
            let window = w.window.clone();
            glib::timeout_add_local_once(Duration::from_millis(200), move || {
                if let Err(error) = screenshot(&window) {
                    eprintln!("Capture native GTK smoke screenshot: {error:#}");
                    std::process::exit(1);
                }
                println!("NATIVE_GTK_SMOKE_OK");
                sender.input(Msg::Shutdown);
            });
        }
    }
}
fn screenshot(window: &gtk::ApplicationWindow) -> anyhow::Result<()> {
    use anyhow::Context;
    let child = window.child().context("The window has no content")?;
    let paintable = gtk::WidgetPaintable::new(Some(&child));
    let snapshot = gtk::Snapshot::new();
    // WidgetPaintable contains the child tree, so include the window background
    // explicitly to produce an opaque, readable artifact.
    snapshot.append_color(
        &gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0),
        &gtk::graphene::Rect::new(0.0, 0.0, window.width() as f32, window.height() as f32),
    );
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    let node = snapshot
        .to_node()
        .context("The native window has no rendered frame")?;
    let renderer = window
        .renderer()
        .context("The native renderer is unavailable")?;
    let texture = renderer.render_texture(&node, None);
    std::fs::create_dir_all("dist")?;
    let backend = std::env::var("GDK_BACKEND").unwrap_or_else(|_| "native".into());
    texture.save_to_png(format!("dist/smoke-linux-{backend}.png"))?;
    Ok(())
}
fn vertical() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, 10)
}
fn horizontal() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, 8)
}
fn label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .selectable(true)
        .wrap(true)
        .xalign(0.0)
        .build()
}
fn entry(hint: &str, value: &str) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .placeholder_text(hint)
        .hexpand(true)
        .build();
    entry.set_text(value);
    entry
}
fn list() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list
}
fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(child)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(220)
        .build()
}
fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}
fn add_row(list: &gtk::ListBox, kind: &str, id: u64, session: u64, value: &str) {
    let row = gtk::ListBoxRow::new();
    row.set_widget_name(&format!("{kind}:{id}:{session}"));
    let content = label(value);
    content.set_margin_all(8);
    row.set_child(Some(&content));
    list.append(&row);
}
fn connect_button(
    container: &gtk::Box,
    title: &str,
    sender: ComponentSender<App>,
    action: impl Fn() -> Action + 'static,
) {
    let button = gtk::Button::with_label(title);
    button.connect_clicked(move |_| sender.input(Msg::Action(action())));
    container.append(&button);
}
fn dispatch_result(sender: &ComponentSender<App>, result: anyhow::Result<Action>) {
    match result {
        Ok(action) => sender.input(Msg::Action(action)),
        Err(error) => sender.input(Msg::Error(error.to_string())),
    }
}
fn picker_button(
    container: &gtk::Box,
    title: &str,
    window: &gtk::ApplicationWindow,
    sender: ComponentSender<App>,
    action: Action,
    save: bool,
) {
    picker_dynamic(
        container,
        title,
        window,
        sender,
        move || Ok(action.clone()),
        save,
    );
}
fn picker_dynamic(
    container: &gtk::Box,
    title: &str,
    window: &gtk::ApplicationWindow,
    sender: ComponentSender<App>,
    action: impl Fn() -> anyhow::Result<Action> + 'static,
    save: bool,
) {
    let button = gtk::Button::with_label(title);
    let window = window.clone();
    let title = title.to_owned();
    button.connect_clicked(move |_| {
        let action = match action() {
            Ok(action) => action,
            Err(e) => {
                sender.input(Msg::Error(e.to_string()));
                return;
            }
        };
        let dialog = gtk::FileChooserNative::new(
            Some(&title),
            Some(&window),
            if save {
                gtk::FileChooserAction::Save
            } else {
                gtk::FileChooserAction::Open
            },
            Some(if save { "Save" } else { "Open" }),
            Some("Cancel"),
        );
        dialog.set_modal(true);
        let sender = sender.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(path) = dialog.file().and_then(|file| file.path())
            {
                let action = match &action {
                    Action::RedactRecording { policy, .. } => Action::RedactRecording {
                        path,
                        policy: policy.clone(),
                    },
                    Action::StartRecording { .. } => Action::StartRecording { path },
                    Action::OpenRecording { recover, .. } => Action::OpenRecording {
                        path,
                        recover: *recover,
                    },
                    Action::Export {
                        format,
                        table,
                        redaction,
                        ..
                    } => Action::Export {
                        path,
                        format: format.clone(),
                        table: table.clone(),
                        redaction: redaction.clone(),
                    },
                    Action::Compare {
                        session, policy, ..
                    } => Action::Compare {
                        path,
                        session: *session,
                        policy: policy.clone(),
                    },
                    Action::SaveWorkspace { name, .. } => Action::SaveWorkspace {
                        path,
                        name: name.clone(),
                    },
                    Action::OpenWorkspace { .. } => Action::OpenWorkspace { path },
                    Action::LoadPolicy { .. } => Action::LoadPolicy { path },
                    Action::SavePolicy { .. } => Action::SavePolicy { path },
                    _ => return,
                };
                sender.input(Msg::Action(action));
            }
            dialog.destroy();
        });
        dialog.show();
    });
    container.append(&button);
}
fn redaction(
    fields: &gtk::Entry,
    paths: &gtk::CheckButton,
    endpoints: &gtk::CheckButton,
) -> RedactionPolicy {
    RedactionPolicy {
        fields: fields
            .text()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        source_paths: paths.is_active(),
        endpoints: endpoints.is_active(),
    }
}
fn trend_chart(points: &[console_ui::analysis::TrendPoint], metric: &'static str) -> gtk::Box {
    let container = vertical();
    let units = if metric == "polls" {
        "polls/s"
    } else {
        "ns/interval"
    };
    container.append(&label(&format!(
        "{metric} · {units} · red marks indicate gaps"
    )));
    let area = gtk::DrawingArea::new();
    area.set_content_height(180);
    area.set_hexpand(true);
    let points = points.to_vec();
    area.set_draw_func(move |_, cr, width, height| {
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.paint();
        cr.set_source_rgb(0.1, 0.1, 0.1);
        cr.move_to(42.0, 12.0);
        cr.line_to(42.0, height as f64 - 26.0);
        cr.line_to(width as f64 - 8.0, height as f64 - 26.0);
        let _ = cr.stroke();
        let value = |p: &console_ui::analysis::TrendPoint| match metric {
            "busy" => p.busy_ns as f64,
            "scheduled" => p.scheduled_ns as f64,
            _ => p.polls_per_second,
        };
        let maximum = points.iter().map(&value).fold(1.0, f64::max);
        let first = points.first().map_or(0, |p| p.now);
        let end = points.last().map_or(first, |p| p.now);
        let span = end.saturating_sub(first).max(1) as f64;
        cr.set_font_size(10.0);
        cr.move_to(45.0, 12.0);
        let _ = cr.show_text(&format!("max {maximum:.2}"));
        cr.move_to(45.0, height as f64 - 8.0);
        let _ = cr.show_text(&format!("Producer time {first}–{end} ns"));
        let mut drawing = false;
        for p in &points {
            let x = 42.0 + (p.now.saturating_sub(first) as f64 / span) * (width as f64 - 54.0);
            let y = height as f64 - 26.0 - value(p) / maximum * (height as f64 - 45.0);
            if p.gap {
                let _ = cr.stroke();
                cr.set_source_rgb(0.8, 0.0, 0.0);
                cr.move_to(x, 20.0);
                cr.line_to(x, height as f64 - 26.0);
                let _ = cr.stroke();
                drawing = false;
            } else {
                cr.set_source_rgb(0.2, 0.4, 0.8);
                if drawing {
                    cr.line_to(x, y);
                } else {
                    cr.move_to(x, y);
                    drawing = true;
                }
            }
        }
        let _ = cr.stroke();
    });
    container.append(&area);
    container
}
fn ns(value: u64) -> String {
    console_ui::model::format_ns(value)
}
fn task_order(a: &TaskRow, b: &TaskRow, sort: u32) -> std::cmp::Ordering {
    match sort {
        0 => b.timing.busy.cmp(&a.timing.busy),
        1 => b.timing.scheduled.cmp(&a.timing.scheduled),
        2 => b.timing.idle.cmp(&a.timing.idle),
        3 => b.timing.polls.cmp(&a.timing.polls),
        4 => b.timing.total.cmp(&a.timing.total),
        5 => a.name.cmp(&b.name),
        _ => a.id.cmp(&b.id),
    }
    .then(a.id.cmp(&b.id))
}
fn task_details(container: &gtk::Box, t: &TaskRow, s: &Snapshot) {
    if !t.stats_available {
        container.append(&label("Task statistics are incomplete at this position."));
    }
    container.append(&label(&format!("{} / task {}\n{:?}\nTotal {}\nBusy {} ({:.1}%)\nScheduled {} ({:.1}%)\nIdle {}\nPolls {}\nActive wakers {}\nClones {} / drops {}\nWakes {} / self {} ({:.1}%)\nLast woken {}\n{}\n{}",t.name,t.id,t.state,ns(t.timing.total),ns(t.timing.busy),console_ui::model::percent(t.timing.busy,t.timing.total),ns(t.timing.scheduled),console_ui::model::percent(t.timing.scheduled,t.timing.total),ns(t.timing.idle),t.timing.polls,t.active_wakers(),t.waker_clones,t.waker_drops,t.wakes,t.self_wakes,t.self_wake_percent(),t.last_wake_ago.map(ns).unwrap_or_else(||"never".into()),t.location,t.fields)));
    for lint in &t.warnings {
        container.append(&label(&format!("Warning: {}", lint.name())));
    }
    if let Some(d) = s.details.get(&t.id) {
        for (name, hist) in [("Poll times", &d.poll), ("Scheduled times", &d.scheduled)] {
            container.append(&label(&format!(
                "{name}: {} samples · {} outliers\nPercentiles: {}",
                hist.count,
                hist.high_outliers,
                hist.percentiles
                    .iter()
                    .map(|&n| ns(n))
                    .collect::<Vec<_>>()
                    .join(" / ")
            )));
            let max = hist.bins.iter().map(|b| b.count).max().unwrap_or(1).max(1);
            for bin in &hist.bins {
                let bar = gtk::ProgressBar::new();
                bar.set_fraction(bin.count as f64 / max as f64);
                bar.set_show_text(true);
                bar.set_text(Some(&format!("≤ {}: {}", ns(bin.upper_ns), bin.count)));
                container.append(&bar);
            }
        }
    } else {
        container.append(&label("Waiting for task latency distributions…"));
    }
}
fn resource_details(
    container: &gtk::Box,
    r: &ResourceRow,
    s: &Snapshot,
    sender: ComponentSender<App>,
) {
    container.append(&label(&format!(
        "{} / resource {}\n{}\n{}\n{}",
        r.concrete_type, r.id, r.kind, r.location, r.attributes
    )));
    for op in s.async_ops.iter().filter(|o| o.resource_id == r.id) {
        container.append(&label(&format!(
            "{} · task {} {}\n{} polls · {} busy · {} idle\n{}",
            op.source,
            op.task_id.unwrap_or(0),
            op.task_name,
            op.timing.polls,
            ns(op.timing.busy),
            ns(op.timing.idle),
            op.attributes
        )));
        if let Some(id) = op.task_id {
            let button = gtk::Button::with_label("Inspect waiting task");
            let sender = sender.clone();
            button.connect_clicked(move |_| sender.input(Msg::InspectTask(id)));
            container.append(&button);
        }
    }
}
fn shlex_args(text: &str) -> anyhow::Result<Vec<String>> {
    shlex::split(text).ok_or_else(|| anyhow::anyhow!("Unclosed quote in editor arguments"))
}
fn main() -> anyhow::Result<()> {
    let smoke = std::env::args().any(|a| a == "--smoke");
    let cli = console_ui::config::Cli::parse_from(std::env::args().filter(|a| a != "--smoke"));
    anyhow::ensure!(
        cli.command.is_none(),
        "Use the console-ui executable for headless commands"
    );
    let settings = console_ui::config::Settings::load(&cli)?;
    let mut app = Workspace::new(settings, cli.record, cli.replay)?;
    if let Some(path) = cli.policy {
        app.dispatch(Action::LoadPolicy { path })?;
    }
    RelmApp::new("org.consoleui.Diagnostics")
        .with_args(vec!["console-ui-gtk".into()])
        .run::<App>((app, smoke));
    Ok(())
}
