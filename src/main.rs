#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{Result, bail};
use clap::{CommandFactory, Parser};
use console_ui::{
    config::{Action, Cli, Settings},
    telemetry::{ConnectionStatus, Worker},
};
use std::{
    io::Write,
    time::{Duration, Instant},
};

fn main() {
    #[cfg(all(windows, not(debug_assertions)))]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
    }
    let cli = Cli::parse();
    #[cfg(windows)]
    let gui = cli.command.is_none();
    if let Err(error) = run(cli) {
        eprintln!("{error:#}");
        #[cfg(windows)]
        if gui {
            console_ui::ui::show_error(&format!("{error:#}"));
        }
        std::process::exit(
            error
                .downcast_ref::<console_ui::automation::CliFailure>()
                .map_or(1, |e| e.code),
        );
    }
}

fn run(cli: Cli) -> Result<()> {
    if let Some(Action::GenCompletion { shell }) = cli.command {
        clap_complete::generate(
            shell,
            &mut Cli::command(),
            "console-ui",
            &mut std::io::stdout(),
        );
        return Ok(());
    }
    let settings = Settings::load(&cli)?;
    match &cli.command {
        Some(Action::Check { options }) => {
            let code = console_ui::automation::check(options, &settings)?;
            if code != 0 {
                return Err(console_ui::automation::CliFailure {
                    code,
                    message: "Diagnostic assertions did not pass; see the result report".into(),
                }
                .into());
            }
            Ok(())
        }
        Some(Action::Sessions { input }) => {
            let recording = console_ui::investigation::Recording::open(input)?;
            println!("{}", serde_json::to_string_pretty(&recording.sessions)?);
            Ok(())
        }
        Some(Action::Compare {
            baseline,
            current,
            baseline_session,
            current_session,
            output,
        }) => {
            let (a, sa) =
                console_ui::automation::capture_window(baseline, *baseline_session, &settings)?;
            let (b, sb) =
                console_ui::automation::capture_window(current, *current_session, &settings)?;
            let comparison = console_ui::analysis::compare(&a, &b, [sa, sb], &Default::default())?;
            let bytes = serde_json::to_vec_pretty(&comparison)?;
            if let Some(path) = output {
                console_ui::preferences::atomic_write(path, &bytes)?;
            } else {
                println!("{}", String::from_utf8(bytes)?);
            }
            Ok(())
        }
        Some(Action::Export {
            input,
            output,
            format,
            table,
            query,
            session,
            at_ns,
            redact_fields,
            redact_paths,
            redact_endpoint,
        }) => {
            let mut recording = console_ui::investigation::Recording::open(input)?;
            let info = recording
                .sessions
                .get(*session)
                .ok_or_else(|| anyhow::anyhow!("Session is unavailable"))?
                .clone();
            let snapshot = recording.seek(*session, at_ns.unwrap_or(info.end), &settings)?;
            let snapshot = console_ui::query::Query::parse(query)?.filter(&snapshot);
            let mut document =
                console_ui::sharing::ExportDocument::new(*session as u64, info.target, snapshot);
            document.range = Some([info.start, info.end]);
            let policy = console_ui::sharing::RedactionPolicy {
                fields: redact_fields.iter().cloned().collect(),
                source_paths: *redact_paths,
                endpoints: *redact_endpoint,
            };
            let document = policy.document(&document);
            match format.as_str() {
                "json" => document.json(output)?,
                "csv" => document.csv(output, table)?,
                "html" => document.report(output, &[], None)?,
                _ => anyhow::bail!("Choose json, csv, or html export format"),
            };
            Ok(())
        }
        Some(Action::GenConfig) => {
            print!("{}", toml::to_string_pretty(&settings)?);
            Ok(())
        }
        Some(Action::Dump { timeout, output }) => {
            let timeout = humantime::parse_duration(timeout)?;
            let worker = Worker::start(settings, cli.record, cli.replay)?;
            let deadline = Instant::now() + timeout;
            loop {
                let state = worker.read();
                if state.snapshot.updates > 0 || state.status == ConnectionStatus::Offline {
                    if let Some(error) = state.error {
                        bail!("{error}");
                    }
                    let json = serde_json::to_vec_pretty(&state.snapshot)?;
                    if let Some(path) = output {
                        std::fs::write(path, json)?;
                    } else {
                        std::io::stdout().write_all(&json)?;
                        println!();
                    }
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    bail!(
                        "No telemetry received within {timeout:?}: {}",
                        state
                            .error
                            .as_deref()
                            .unwrap_or("target is not publishing updates")
                    );
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        Some(Action::GenCompletion { .. }) => unreachable!(),
        None => {
            #[cfg(windows)]
            {
                console_ui::ui::run(settings, cli.record, cli.replay)
            }
            #[cfg(not(windows))]
            {
                bail!(
                    "The native interface requires Windows 11. Use 'dump', 'gen-config', or 'gen-completion' on this platform."
                );
            }
        }
    }
}
