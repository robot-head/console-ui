use std::{collections::BTreeSet, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Lint {
    SelfWakes,
    LostWaker,
    NeverYielded,
    AutoBoxedFuture,
    LargeFuture,
}

impl Lint {
    pub const ALL: [Self; 5] = [
        Self::SelfWakes,
        Self::LostWaker,
        Self::NeverYielded,
        Self::AutoBoxedFuture,
        Self::LargeFuture,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::SelfWakes => "self-wakes",
            Self::LostWaker => "lost-waker",
            Self::NeverYielded => "never-yielded",
            Self::AutoBoxedFuture => "auto-boxed-future",
            Self::LargeFuture => "large-future",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Palette {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Native Windows 11 console for Tokio runtime telemetry"
)]
pub struct Cli {
    /// console-subscriber gRPC endpoint (HTTP or HTTPS over TCP).
    pub target: Option<String>,
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Time to keep completed tasks/resources; 'none' keeps them indefinitely.
    #[arg(long, global = true)]
    pub retain_for: Option<String>,
    #[arg(short = 'W', long = "warn", value_delimiter = ',', global = true)]
    pub warn: Vec<Lint>,
    #[arg(short = 'A', long = "allow", value_delimiter = ',', global = true)]
    pub allow: Vec<Lint>,
    #[arg(long, global = true)]
    pub palette: Option<Palette>,
    #[arg(long, global = true)]
    pub ascii_only: bool,
    #[arg(long, global = true)]
    pub no_colors: bool,
    /// Record the wire stream in Console UI's framed protobuf format.
    #[arg(long, conflicts_with = "replay")]
    pub record: Option<PathBuf>,
    /// Open a Console UI wire recording for offline inspection.
    #[arg(long, conflicts_with = "record")]
    pub replay: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Action>,
}

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Print the effective configuration as TOML.
    GenConfig,
    /// Generate shell completions (including PowerShell).
    GenCompletion { shell: clap_complete::Shell },
    /// Connect without a window and print a JSON snapshot after a telemetry update.
    Dump {
        #[arg(long, default_value = "10s")]
        timeout: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub target: String,
    pub retain_for: String,
    pub warnings: BTreeSet<Lint>,
    pub palette: Palette,
    pub ascii_only: bool,
    pub no_colors: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target: "http://127.0.0.1:6669".into(),
            retain_for: "60s".into(),
            warnings: Lint::ALL.into_iter().collect(),
            palette: Palette::System,
            ascii_only: false,
            no_colors: false,
        }
    }
}

impl Settings {
    pub fn load(cli: &Cli) -> Result<Self> {
        let path = cli.config.clone().or_else(|| {
            let local = PathBuf::from("console.toml");
            local.is_file().then_some(local)
        });
        let mut settings: Self = match path {
            Some(path) => toml::from_str(
                &std::fs::read_to_string(&path)
                    .with_context(|| format!("Read {}", path.display()))?,
            )
            .with_context(|| format!("Parse {}", path.display()))?,
            None => Self::default(),
        };
        if let Some(target) = &cli.target {
            settings.target.clone_from(target);
        }
        if let Some(retain) = &cli.retain_for {
            settings.retain_for.clone_from(retain);
        }
        if let Some(palette) = cli.palette {
            settings.palette = palette;
        }
        settings.ascii_only |= cli.ascii_only;
        settings.no_colors |= cli.no_colors;
        settings.warnings.extend(&cli.warn);
        for lint in &cli.allow {
            settings.warnings.remove(lint);
        }
        validate_target(&settings.target)?;
        settings.retention()?;
        Ok(settings)
    }

    pub fn retention(&self) -> Result<Option<Duration>> {
        if self.retain_for.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        Ok(Some(humantime::parse_duration(&self.retain_for).context(
            "--retain-for must be a duration such as '60s', '5min', or 'none'",
        )?))
    }
}

pub fn validate_target(target: &str) -> Result<()> {
    if !target.starts_with("http://") && !target.starts_with("https://") {
        bail!(
            "Use an http:// or https:// TCP endpoint. Unix sockets and vsock are not supported by this Windows client."
        );
    }
    let uri: tonic::codegen::http::Uri = target.parse().context("Invalid target URI")?;
    if uri.host().is_none_or(str::is_empty)
        || uri.authority().is_some_and(|a| a.as_str().contains('@'))
    {
        bail!("The target needs a hostname and must not contain embedded credentials");
    }
    if uri
        .path_and_query()
        .is_some_and(|p| p.as_str() != "/" && !p.as_str().is_empty())
    {
        bail!("The target must be a gRPC server origin, without a path or query");
    }
    tonic::transport::Endpoint::from_shared(target.to_owned()).context("Invalid gRPC endpoint")?;
    Ok(())
}
