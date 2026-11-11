//! Versioned user/project preferences shared by all native frontends.
use crate::config::{Settings, validate_target};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

pub fn data_dir() -> Result<PathBuf> {
    Ok(
        directories::ProjectDirs::from("org", "ConsoleUI", "ConsoleUI")
            .context("Cannot determine the user's application settings directory")?
            .config_dir()
            .to_owned(),
    )
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("Save {}", path.display()))?;
    Ok(())
}

pub fn save_json<T: Serialize>(path: &Path, document: &T) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(document)?)
}

pub fn load_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T> {
    serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("Read {}", path.display()))?,
    )
    .with_context(|| format!("Parse {}", path.display()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub name: String,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterPreset {
    pub name: String,
    pub expression: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub key: String,
    pub width: f64,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TablePreferences {
    pub columns: Vec<Column>,
    pub sort: String,
    pub descending: bool,
}
impl Default for TablePreferences {
    fn default() -> Self {
        Self {
            columns: vec![],
            sort: "busy".into(),
            descending: true,
        }
    }
}
impl TablePreferences {
    pub fn validate(&self) -> Result<()> {
        let mut names = std::collections::BTreeSet::new();
        for column in &self.columns {
            ensure!(
                !column.key.is_empty() && names.insert(&column.key),
                "Column keys must be nonempty and unique"
            );
            ensure!(
                column.width.is_finite() && (32.0..=2000.0).contains(&column.width),
                "Column widths must be between 32 and 2000 DIPs"
            );
        }
        ensure!(
            self.columns.is_empty() || self.columns.iter().any(|c| c.visible),
            "Keep at least one visible column"
        );
        Ok(())
    }
}
pub fn default_columns(table: &str) -> Vec<Column> {
    let names: &[&str] = match table {
        "resources" => &[
            "id",
            "kind",
            "type",
            "total",
            "visibility",
            "target",
            "location",
            "attributes",
        ],
        "operations" => &[
            "id",
            "task",
            "source",
            "total",
            "busy",
            "idle",
            "polls",
            "state",
            "attributes",
        ],
        _ => &[
            "warn", "id", "name", "state", "total", "busy", "sched", "idle", "polls", "target",
            "location",
        ],
    };
    names
        .iter()
        .map(|&key| Column {
            key: key.into(),
            width: if [
                "name",
                "source",
                "location",
                "attributes",
                "target",
                "type",
                "task",
            ]
            .contains(&key)
            {
                200.0
            } else {
                90.0
            },
            visible: true,
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMapping {
    pub remote_root: String,
    pub local_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorPreferences {
    pub program: String,
    pub arguments: Vec<String>,
    pub mappings: Vec<PathMapping>,
}
impl Default for EditorPreferences {
    fn default() -> Self {
        Self {
            program: "code".into(),
            arguments: vec!["--goto".into(), "{file}:{line}:{column}".into()],
            mappings: vec![],
        }
    }
}
impl EditorPreferences {
    pub fn resolve(&self, location: &str) -> Result<(PathBuf, u32, u32)> {
        let mut parts = location.rsplitn(3, ':');
        let column: u32 = parts
            .next()
            .context("Missing source column")?
            .parse()
            .context("Invalid source column")?;
        let line: u32 = parts
            .next()
            .context("Missing source line")?
            .parse()
            .context("Invalid source line")?;
        let source = parts
            .next()
            .context("Missing source file")?
            .replace('\\', "/");
        let mut mappings = self.mappings.iter().collect::<Vec<_>>();
        mappings.sort_by_key(|m| std::cmp::Reverse(m.remote_root.len()));
        let path = mappings
            .into_iter()
            .find_map(|mapping| {
                let root = mapping.remote_root.replace('\\', "/");
                let root = root.trim_end_matches('/');
                let suffix = source.strip_prefix(root)?;
                if !suffix.is_empty() && !suffix.starts_with('/') {
                    return None;
                }
                let suffix = suffix.trim_start_matches('/');
                if suffix.split('/').any(|p| p == "..") {
                    return None;
                }
                Some(mapping.local_root.join(suffix))
            })
            .unwrap_or_else(|| PathBuf::from(&source));
        ensure!(
            path.is_file(),
            "Source does not exist: {}. Configure a source-root mapping.",
            path.display()
        );
        Ok((path, line, column))
    }
    pub fn command(&self, location: &str) -> Result<(String, Vec<String>)> {
        ensure!(
            !self.program.trim().is_empty(),
            "Configure an editor program"
        );
        let (file, line, column) = self.resolve(location)?;
        let args = self
            .arguments
            .iter()
            .map(|arg| {
                arg.replace("{file}", &file.to_string_lossy())
                    .replace("{line}", &line.to_string())
                    .replace("{column}", &column.to_string())
            })
            .collect();
        Ok((self.program.clone(), args))
    }
    pub fn open(&self, location: &str) -> Result<()> {
        let (program, args) = self.command(location)?;
        Command::new(program)
            .args(args)
            .spawn()
            .context("Launch configured source editor")?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub schema_version: u32,
    pub profiles: Vec<ConnectionProfile>,
    pub recent_targets: Vec<String>,
    pub filter_presets: Vec<FilterPreset>,
    pub tables: BTreeMap<String, TablePreferences>,
    pub editor: EditorPreferences,
    pub settings: Settings,
    pub saved_views: Vec<crate::investigation::InvestigationView>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            profiles: vec![],
            recent_targets: vec![],
            filter_presets: vec![],
            tables: BTreeMap::new(),
            editor: EditorPreferences::default(),
            settings: Settings::default(),
            saved_views: vec![],
        }
    }
}
impl Preferences {
    pub fn path() -> Result<PathBuf> {
        Ok(data_dir()?.join("preferences.json"))
    }
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let value: Self = load_json(path)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "Unsupported preferences version {}",
            self.schema_version
        );
        self.settings.retention()?;
        self.settings.lint_thresholds.validate()?;
        for profile in &self.profiles {
            ensure!(!profile.name.trim().is_empty(), "A profile needs a name");
            validate_target(&profile.target)?;
        }
        for preset in &self.filter_presets {
            crate::query::Query::parse(&preset.expression)?;
        }
        for table in self.tables.values() {
            table.validate()?;
        }
        Ok(())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        save_json(path, self)
    }
    pub fn remember(&mut self, target: &str) -> Result<()> {
        validate_target(target)?;
        self.recent_targets.retain(|t| t != target);
        self.recent_targets.insert(0, target.into());
        self.recent_targets.truncate(20);
        Ok(())
    }
    pub fn set_profile(&mut self, name: String, target: String) -> Result<()> {
        ensure!(!name.trim().is_empty(), "A profile needs a name");
        validate_target(&target)?;
        self.profiles.retain(|p| p.name != name);
        self.profiles.push(ConnectionProfile { name, target });
        Ok(())
    }
    pub fn set_preset(&mut self, name: String, expression: String) -> Result<()> {
        ensure!(!name.trim().is_empty(), "A preset needs a name");
        crate::query::Query::parse(&expression)?;
        self.filter_presets.retain(|p| p.name != name);
        self.filter_presets.push(FilterPreset { name, expression });
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectPolicy {
    pub schema_version: u32,
    pub settings: Option<Settings>,
    pub presets: Vec<FilterPreset>,
}
impl Default for ProjectPolicy {
    fn default() -> Self {
        Self {
            schema_version: 1,
            settings: None,
            presets: vec![],
        }
    }
}
impl ProjectPolicy {
    pub fn apply(&self, defaults: &Preferences, session: Option<&Settings>) -> Result<Preferences> {
        ensure!(
            self.schema_version == 1,
            "Unsupported project policy version"
        );
        let mut effective = defaults.clone();
        if let Some(settings) = &self.settings {
            effective.settings = settings.clone();
        }
        for p in &self.presets {
            effective.set_preset(p.name.clone(), p.expression.clone())?;
        }
        if let Some(settings) = session {
            effective.settings = settings.clone();
        }
        effective.validate()?;
        Ok(effective)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let policy: Self = load_json(path)?;
        if policy.schema_version != 1 {
            bail!("Unsupported project policy version");
        }
        Ok(policy)
    }
}
