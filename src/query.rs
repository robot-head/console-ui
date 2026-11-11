//! AND-separated clauses; `|` separates OR groups; quotes preserve spaces; `!` negates a clause.
use crate::model::{AsyncOpRow, ResourceRow, Snapshot, TaskRow};
use anyhow::{Result, bail, ensure};

#[derive(Clone, Debug, Default)]
pub struct Query {
    groups: Vec<Vec<Clause>>,
}
#[derive(Clone, Debug)]
struct Clause {
    negate: bool,
    field: String,
    operation: Operation,
}
#[derive(Clone, Debug)]
enum Operation {
    Contains(String),
    Compare(char, bool, u64),
}

impl Query {
    pub fn parse(expression: &str) -> Result<Self> {
        let tokens =
            shlex::split(expression).ok_or_else(|| anyhow::anyhow!("Unclosed quote in filter"))?;
        if tokens.is_empty() {
            return Ok(Self::default());
        }
        let mut groups = vec![vec![]];
        for token in tokens {
            if token == "|" {
                ensure!(
                    !groups.last().unwrap().is_empty(),
                    "OR requires a query on both sides"
                );
                groups.push(vec![]);
                continue;
            }
            let (negate, token) = token
                .strip_prefix('!')
                .map_or((false, token.as_str()), |s| (true, s));
            ensure!(!token.is_empty(), "Negation needs a clause");
            let (field, operation) = if let Some(index) = token.find(['>', '<', '=']) {
                let field = &token[..index];
                ensure!(
                    [
                        "id",
                        "busy",
                        "scheduled",
                        "sched",
                        "idle",
                        "total",
                        "polls",
                        "warn",
                        "size"
                    ]
                    .contains(&field),
                    "Unknown numeric filter {field}"
                );
                let rest = &token[index..];
                let operator = rest.chars().next().unwrap();
                let (inclusive, value) = if let Some(rest) = rest[1..].strip_prefix('=') {
                    (true, rest)
                } else {
                    (operator == '=', &rest[1..])
                };
                ensure!(!value.is_empty(), "Filter {field} needs a value");
                let number = if ["busy", "scheduled", "sched", "idle", "total"].contains(&field) {
                    humantime::parse_duration(value)?.as_nanos().try_into()?
                } else if field == "size" {
                    let (digits, multiplier) = if let Some(s) = value.strip_suffix("KiB") {
                        (s, 1024)
                    } else if let Some(s) = value.strip_suffix("MiB") {
                        (s, 1024 * 1024)
                    } else {
                        (value, 1)
                    };
                    digits
                        .parse::<u64>()?
                        .checked_mul(multiplier)
                        .ok_or_else(|| anyhow::anyhow!("Size is too large"))?
                } else {
                    value.parse()?
                };
                (
                    field.into(),
                    Operation::Compare(operator, inclusive, number),
                )
            } else if let Some((field, value)) = token.split_once(':') {
                ensure!(
                    [
                        "name",
                        "id",
                        "state",
                        "warning",
                        "target",
                        "source",
                        "location",
                        "kind",
                        "type",
                        "visibility",
                        "fields",
                        "attributes"
                    ]
                    .contains(&field)
                        || field.starts_with("field."),
                    "Unknown filter {field}"
                );
                ensure!(!value.is_empty(), "Filter {field} needs a value");
                (field.into(), Operation::Contains(value.to_lowercase()))
            } else {
                ("text".into(), Operation::Contains(token.to_lowercase()))
            };
            groups.last_mut().unwrap().push(Clause {
                negate,
                field,
                operation,
            });
        }
        ensure!(
            !groups.last().unwrap().is_empty(),
            "OR requires a query on both sides"
        );
        Ok(Self { groups })
    }
    fn matches(
        &self,
        get: impl Fn(&str) -> Option<String>,
        number: impl Fn(&str) -> Option<u64>,
    ) -> bool {
        self.groups.is_empty()
            || self.groups.iter().any(|group| {
                group.iter().all(|clause| {
                    let matches = match &clause.operation {
                        Operation::Contains(needle) => {
                            get(&clause.field).is_some_and(|v| v.to_lowercase().contains(needle))
                        }
                        Operation::Compare(op, inclusive, threshold) => number(&clause.field)
                            .is_some_and(|v| match op {
                                '>' => v > *threshold || (*inclusive && v == *threshold),
                                '<' => v < *threshold || (*inclusive && v == *threshold),
                                '=' => v == *threshold,
                                _ => false,
                            }),
                    };
                    matches != clause.negate
                })
            })
    }
    pub fn task(&self, t: &TaskRow) -> bool {
        self.matches(
            |field| {
                Some(match field {
                    "name" => t.name.clone(),
                    "id" => t.id.to_string(),
                    "state" => format!("{:?}", t.state),
                    "warning" => t
                        .warnings
                        .iter()
                        .map(|w| w.name())
                        .collect::<Vec<_>>()
                        .join(" "),
                    "target" => t.target.clone(),
                    "source" | "location" => t.location.clone(),
                    "fields" => t.fields.clone(),
                    "kind" | "type" => if t.blocking { "blocking" } else { "async" }.into(),
                    "text" => format!(
                        "{} {} {} {} {} {:?}",
                        t.id, t.name, t.target, t.location, t.fields, t.state
                    ),
                    key if key.starts_with("field.") => {
                        return t.span_fields.get(&key[6..]).cloned();
                    }
                    _ => return None,
                })
            },
            |f| match f {
                "id" => Some(t.id),
                "warn" => Some(t.warnings.len() as u64),
                "size" => t.size_bytes,
                _ => metric(&t.timing, f),
            },
        )
    }
    pub fn resource(&self, r: &ResourceRow) -> bool {
        self.matches(
            |f| {
                Some(match f {
                    "name" | "type" => r.concrete_type.clone(),
                    "id" => r.id.to_string(),
                    "kind" => r.kind.clone(),
                    "target" => r.target.clone(),
                    "source" | "location" => r.location.clone(),
                    "visibility" => if r.internal { "INT" } else { "PUB" }.into(),
                    "state" => if r.completed { "Completed" } else { "Active" }.into(),
                    "attributes" => r.attributes.clone(),
                    "text" => format!(
                        "{} {} {} {} {}",
                        r.id, r.kind, r.concrete_type, r.location, r.attributes
                    ),
                    _ => return None,
                })
            },
            |f| match f {
                "id" => Some(r.id),
                "total" => Some(r.total),
                _ => None,
            },
        )
    }
    pub fn operation(&self, o: &AsyncOpRow) -> bool {
        self.matches(
            |f| {
                Some(match f {
                    "id" => o.id.to_string(),
                    "name" => o.task_name.clone(),
                    "source" => o.source.clone(),
                    "attributes" => o.attributes.clone(),
                    "state" => if o.completed {
                        "Completed"
                    } else {
                        match o.ready {
                            Some(true) => "Ready",
                            Some(false) => "Pending",
                            None => "Unknown",
                        }
                    }
                    .into(),
                    "text" => format!("{} {} {} {}", o.id, o.task_name, o.source, o.attributes),
                    _ => return None,
                })
            },
            |f| {
                if f == "id" {
                    Some(o.id)
                } else {
                    metric(&o.timing, f)
                }
            },
        )
    }
    pub fn filter(&self, snapshot: &Snapshot) -> Snapshot {
        let mut result = snapshot.clone();
        result.tasks.retain(|t| self.task(t));
        result.resources.retain(|r| self.resource(r));
        result.async_ops.retain(|o| self.operation(o));
        result
            .details
            .retain(|id, _| result.tasks.iter().any(|t| t.id == *id));
        result
    }
}
fn metric(t: &crate::model::Timing, name: &str) -> Option<u64> {
    match name {
        "busy" => Some(t.busy),
        "scheduled" | "sched" => Some(t.scheduled),
        "idle" => Some(t.idle),
        "total" => Some(t.total),
        "polls" => Some(t.polls),
        _ => None,
    }
}

pub fn validate_task_id(input: &str) -> Result<u64> {
    let id = input.parse::<u64>()?;
    if id == 0 {
        bail!("Task ID must be positive");
    }
    Ok(id)
}
