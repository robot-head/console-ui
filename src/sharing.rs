//! Offline exports and copy-only redaction. No sharing action uploads data.
use crate::{
    analysis::{Comparison, TrendPoint},
    model::Snapshot,
    preferences::atomic_write,
};
use anyhow::{Result, ensure};
use console_api::{self as api, instrument::Update};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportDocument {
    pub schema_version: u32,
    pub session: u64,
    pub target: Option<String>,
    pub range: Option<[u64; 2]>,
    pub snapshot: Snapshot,
    pub notes: Vec<String>,
    #[serde(default = "default_report_sections")]
    pub sections: Vec<String>,
}
pub fn default_report_sections() -> Vec<String> {
    ["charts", "tasks", "resources", "comparison"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}
impl ExportDocument {
    pub fn new(session: u64, target: Option<String>, snapshot: Snapshot) -> Self {
        Self {
            schema_version: 1,
            session,
            target,
            range: None,
            snapshot,
            notes: vec![],
            sections: default_report_sections(),
        }
    }
    pub fn json(&self, path: &Path) -> Result<()> {
        atomic_write(path, &serde_json::to_vec_pretty(self)?)
    }
    pub fn csv(&self, path: &Path, table: &str) -> Result<()> {
        let mut rows = vec![vec![
            "session".into(),
            "now_ns".into(),
            "entity".into(),
            "id".into(),
            "name_or_type".into(),
            "state".into(),
            "total_ns".into(),
            "busy_ns".into(),
            "scheduled_ns".into(),
            "idle_ns".into(),
            "polls".into(),
            "source".into(),
            "fields_or_attributes".into(),
        ]];
        if table == "tasks" {
            for t in &self.snapshot.tasks {
                rows.push(vec![
                    self.session.to_string(),
                    self.snapshot.now.to_string(),
                    "task".into(),
                    t.id.to_string(),
                    t.name.clone(),
                    format!("{:?}", t.state),
                    t.timing.total.to_string(),
                    t.timing.busy.to_string(),
                    t.timing.scheduled.to_string(),
                    t.timing.idle.to_string(),
                    t.timing.polls.to_string(),
                    t.location.clone(),
                    t.fields.clone(),
                ]);
            }
        } else if table == "resources" {
            for r in &self.snapshot.resources {
                rows.push(vec![
                    self.session.to_string(),
                    self.snapshot.now.to_string(),
                    "resource".into(),
                    r.id.to_string(),
                    r.concrete_type.clone(),
                    if r.completed { "completed" } else { "active" }.into(),
                    r.total.to_string(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    r.location.clone(),
                    r.attributes.clone(),
                ]);
            }
        } else if table == "operations" {
            for o in &self.snapshot.async_ops {
                rows.push(vec![
                    self.session.to_string(),
                    self.snapshot.now.to_string(),
                    "operation".into(),
                    o.id.to_string(),
                    o.task_name.clone(),
                    if o.completed {
                        "completed"
                    } else {
                        match o.ready {
                            Some(true) => "ready",
                            Some(false) => "pending",
                            None => "unknown",
                        }
                    }
                    .into(),
                    o.timing.total.to_string(),
                    o.timing.busy.to_string(),
                    o.timing.scheduled.to_string(),
                    o.timing.idle.to_string(),
                    o.timing.polls.to_string(),
                    o.source.clone(),
                    o.attributes.clone(),
                ]);
            }
        } else {
            anyhow::bail!("Unknown export table {table}");
        }
        let csv = rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|value| format!("\"{}\"", value.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("\r\n");
        atomic_write(path, format!("{csv}\r\n").as_bytes())
    }
    pub fn report(
        &self,
        path: &Path,
        points: &[TrendPoint],
        comparison: Option<&Comparison>,
    ) -> Result<()> {
        let mut html = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Console UI investigation</title><style>body{{font:16px system-ui;max-width:1100px;margin:40px auto;padding:20px}}table{{border-collapse:collapse;width:100%}}td,th{{padding:8px;border:1px solid #aaa;text-align:left}}pre{{white-space:pre-wrap}}svg{{width:100%;height:auto}}.note{{background:#eee;padding:12px}}</style><h1>Runtime investigation</h1><p>Session {} · Producer timestamp {} ns</p><p>Target: {}</p><p>Range: {}</p>",
            self.session,
            self.snapshot.now,
            escape(self.target.as_deref().unwrap_or("unspecified")),
            escape(&format!("{:?}", self.range))
        );
        for note in &self.notes {
            html.push_str(&format!("<p class=note>{}</p>", escape(note)));
        }
        if self.sections.iter().any(|s| s == "charts") {
            for metric in ["busy", "scheduled", "polls"] {
                html.push_str(&chart_svg(points, metric)?);
            }
        }
        if self.sections.iter().any(|s| s == "tasks") {
            html.push_str("<h2>Tasks</h2><table><tr><th>Task</th><th>Busy ns</th><th>Scheduled ns</th><th>Polls</th><th>Warnings</th><th>Source</th></tr>");
            for t in &self.snapshot.tasks {
                html.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape(&t.name),
                    t.timing.busy,
                    t.timing.scheduled,
                    t.timing.polls,
                    escape(
                        &t.warnings
                            .iter()
                            .map(|l| l.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    escape(&t.location)
                ));
            }
            html.push_str("</table><p>HDR percentiles cover recorded in-range samples; unavailable distributions are unknown. Reports contain the selected retained data.</p>");
        }
        if self.sections.iter().any(|s| s == "comparison")
            && let Some(comparison) = comparison
        {
            html.push_str("<h2>Comparison</h2><pre>");
            html.push_str(&escape(&serde_json::to_string_pretty(comparison)?));
            html.push_str("</pre>");
        }
        if self.sections.iter().any(|s| s == "resources") {
            html.push_str("<h2>Resources and operations</h2><pre>");
            html.push_str(&escape(&serde_json::to_string_pretty(&(
                &self.snapshot.resources,
                &self.snapshot.async_ops,
            ))?));
            html.push_str("</pre>");
        }
        atomic_write(path, html.as_bytes())
    }
}
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn chart_svg(points: &[TrendPoint], metric: &str) -> Result<String> {
    ensure!(
        ["polls", "busy", "scheduled"].contains(&metric),
        "Unknown chart metric"
    );
    let values: Vec<f64> = points
        .iter()
        .map(|p| match metric {
            "busy" => p.busy_ns as f64,
            "scheduled" => p.scheduled_ns as f64,
            _ => p.polls_per_second,
        })
        .collect();
    let max = values.iter().copied().fold(1.0, f64::max);
    let first = points.first().map_or(0, |p| p.now);
    let end = points.last().map_or(first, |p| p.now);
    let span = end.saturating_sub(first).max(1) as f64;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 800 240\" role=\"img\" aria-label=\"{metric} trend\"><rect width=\"800\" height=\"240\" fill=\"white\"/><text x=\"40\" y=\"20\" font-size=\"14\">{metric} {} · max {max:.2}</text><path d=\"M40 30V210H780\" stroke=\"black\" fill=\"none\"/>",
        if metric == "polls" {
            "polls/s"
        } else {
            "ns/interval"
        }
    );
    let mut segment = String::new();
    for (point, value) in points.iter().zip(values) {
        let x = 40.0 + (point.now.saturating_sub(first) as f64 / span) * 740.0;
        let y = 210.0 - value / max * 175.0;
        if point.gap {
            if !segment.is_empty() {
                svg.push_str(&format!("<polyline points=\"{segment}\" fill=\"none\" stroke=\"#3268cc\" stroke-width=\"2\"/>"));
                segment.clear();
            }
            svg.push_str(&format!(
                "<path d=\"M{x:.2} 30V210\" stroke=\"#b00020\" stroke-dasharray=\"3\"/>"
            ));
        } else {
            segment.push_str(&format!("{x:.2},{y:.2} "));
        }
    }
    if !segment.is_empty() {
        svg.push_str(&format!(
            "<polyline points=\"{segment}\" fill=\"none\" stroke=\"#3268cc\" stroke-width=\"2\"/>"
        ));
    }
    svg.push_str(&format!("<text x=\"40\" y=\"232\" font-size=\"11\">Producer time: {first}–{end} ns. Red marks indicate incomplete intervals.</text></svg>"));
    Ok(svg)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RedactionPolicy {
    pub fields: BTreeSet<String>,
    pub source_paths: bool,
    pub endpoints: bool,
}
impl RedactionPolicy {
    fn redacts_field(&self, name: &str) -> bool {
        self.fields.contains(name)
            || (self.source_paths
                && (name == "file"
                    || name == "loc.file"
                    || name.ends_with(".file")
                    || name.ends_with(".path")))
    }
    pub fn document(&self, source: &ExportDocument) -> ExportDocument {
        let mut result = source.clone();
        if self.endpoints {
            result.target = Some("[redacted]".into());
        }
        for t in &mut result.snapshot.tasks {
            for (key, value) in &mut t.span_fields {
                if self.redacts_field(key) {
                    *value = "[redacted]".into();
                }
            }
            if self.fields.contains("task.name") {
                t.name = "[redacted]".into();
            }
            if !self.fields.is_empty() || self.source_paths {
                t.fields = if t.span_fields.is_empty() {
                    "[legacy fields redacted]".into()
                } else {
                    t.span_fields
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("  ")
                };
            }
            if self.source_paths {
                t.location = "[redacted]".into();
            }
        }
        for resource in &mut result.snapshot.resources {
            if self.source_paths {
                resource.location = "[redacted]".into();
            }
        }
        // Attribute strings do not preserve their field structure in old snapshots. Remove them
        // conservatively whenever field redaction is requested.
        if !self.fields.is_empty() {
            for r in &mut result.snapshot.resources {
                r.attributes = "[redacted]".into();
            }
            for o in &mut result.snapshot.async_ops {
                o.attributes = "[redacted]".into();
                if self.fields.contains("task.name") {
                    o.task_name = "[redacted]".into();
                }
            }
        }
        if self.source_paths || self.endpoints || !self.fields.is_empty() {
            result.notes =
                vec!["Export redacted with an explicit field/path/endpoint policy.".into()];
        }
        result
    }
    pub fn recording_copy(&self, input: &Path, output: &Path) -> Result<()> {
        ensure!(
            input != output,
            "A redacted copy must use a new destination"
        );
        let mut reader = BufReader::new(File::open(input)?);
        let mut header = [0; 8];
        reader.read_exact(&mut header)?;
        ensure!(
            &header == b"TCUI\x01\0\0\0" || &header == b"TCUI\x02\0\0\0",
            "Unsupported recording format"
        );
        let mut writer = crate::recording::Recorder::create(output)?;
        let mut metadata = BTreeMap::<u64, api::Metadata>::new();
        while let Some((kind, bytes)) = crate::investigation::read_frame(&mut reader)? {
            match kind {
                1 => {
                    let mut u = Update::decode(bytes.as_slice())?;
                    if let Some(m) = &mut u.new_metadata {
                        for entry in &mut m.metadata {
                            if let (Some(id), Some(value)) = (entry.id, &mut entry.metadata) {
                                if self.source_paths {
                                    redact_location(&mut value.location);
                                }
                                metadata.insert(id.id, value.clone());
                            }
                        }
                    }
                    if let Some(tasks) = &mut u.task_update {
                        for task in &mut tasks.new_tasks {
                            for f in &mut task.fields {
                                self.field(f, task.metadata, &metadata);
                            }
                            if self.source_paths {
                                redact_location(&mut task.location);
                            }
                        }
                    }
                    if let Some(resources) = &mut u.resource_update {
                        for r in &mut resources.new_resources {
                            if self.source_paths {
                                redact_location(&mut r.location);
                            }
                        }
                        for s in resources.stats_update.values_mut() {
                            for a in &mut s.attributes {
                                if let Some(f) = &mut a.field {
                                    self.field(f, None, &metadata);
                                }
                            }
                        }
                    }
                    if let Some(ops) = &mut u.async_op_update {
                        for s in ops.stats_update.values_mut() {
                            for a in &mut s.attributes {
                                if let Some(f) = &mut a.field {
                                    self.field(f, None, &metadata);
                                }
                            }
                        }
                    }
                    writer.raw_frame(1, &u.encode_to_vec())?;
                }
                3 => {
                    metadata.clear();
                    writer.raw_frame(kind, &bytes)?;
                }
                4 => {
                    let mut value: crate::investigation::SessionMetadata =
                        serde_json::from_slice(&bytes)?;
                    if self.endpoints {
                        value.target = Some("[redacted]".into());
                    }
                    writer.raw_frame(4, &serde_json::to_vec(&value)?)?;
                }
                2 => writer.raw_frame(kind, &bytes)?,
                _ => anyhow::bail!("Unknown recording frame type {kind}"),
            }
        }
        writer.flush()
    }
    fn field(
        &self,
        field: &mut api::Field,
        fallback: Option<api::MetaId>,
        metadata: &BTreeMap<u64, api::Metadata>,
    ) {
        let name = match &field.name {
            Some(api::field::Name::StrName(s)) => Some(s.as_str()),
            Some(api::field::Name::NameIdx(i)) => field
                .metadata_id
                .or(fallback)
                .and_then(|id| metadata.get(&id.id))
                .and_then(|m| m.field_names.get(*i as usize))
                .map(String::as_str),
            None => None,
        };
        if name.is_some_and(|n| self.redacts_field(n))
            || ((!self.fields.is_empty() || self.source_paths) && name.is_none())
        {
            field.value = Some(api::field::Value::StrVal("[redacted]".into()));
        }
    }
}
fn redact_location(location: &mut Option<api::Location>) {
    if let Some(l) = location {
        l.file = Some("[redacted]".into());
        l.module_path = None;
    }
}
