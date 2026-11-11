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
        std::process::exit(1);
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
