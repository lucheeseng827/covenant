//! OpenLineage: a run's verdict, as a lineage service reads it.
//!
//! `check --openlineage <url>`, and the Kafka gate's, send the run to an
//! OpenLineage endpoint (Marquez, or any catalog that takes the standard's
//! HTTP events), so the verdict shows next to the dataset it was about. A run is two events, START and COMPLETE, with one `runId` made from
//! the run's report document: the same run sent twice is the same run.
//!
//! Each source is an input dataset, named as OpenLineage names it: a file is
//! namespace `file` and its absolute path, a topic is namespace
//! `kafka://host:port` and its name. It carries two facets of the standard:
//!
//! - `dataQualityAssertions`: one assertion per rule the run held the source
//!   to, by column: every rule the contract model declares, in its order, and
//!   any other rule the run reported (a missing column, a line that is not a
//!   record). An assertion is the rule's stable name; it succeeds when the
//!   rule was not broken; its severity is `error`, or `warn` under
//!   `on_violation: warn`.
//! - `dataQualityMetrics`: the rows read.
//!
//! Nothing else: no values and no samples. The events leave the machine as
//! the report document does, and say less.

use std::path::Path;

use serde_json::{json, Value};

use crate::compile::CompiledModel;
use crate::error::{CovenantError, Result};
use crate::protocol::{Body, CheckEntry, ReportDocument};
use crate::report::Rule;

/// The events' schema: OpenLineage 2-0-2.
pub const EVENT_SCHEMA: &str = "https://openlineage.io/spec/2-0-2/OpenLineage.json#/$defs/RunEvent";
const ASSERTIONS_SCHEMA: &str = "https://openlineage.io/spec/facets/1-0-2/DataQualityAssertionsDatasetFacet.json#/$defs/DataQualityAssertionsDatasetFacet";
const METRICS_SCHEMA: &str = "https://openlineage.io/spec/facets/1-0-3/DataQualityMetricsInputDatasetFacet.json#/$defs/DataQualityMetricsInputDatasetFacet";

/// Where the events' API key is read from, as OpenLineage's own clients read
/// it: sent as `Authorization: Bearer`, never read from the command line.
pub const API_KEY_ENV: &str = "OPENLINEAGE_API_KEY";

/// The job namespace every Covenant run is under.
pub const JOB_NAMESPACE: &str = "covenant";

fn producer() -> String {
    format!(
        "https://github.com/lucheeseng827/covenant/tree/v{}",
        env!("CARGO_PKG_VERSION")
    )
}

/// A rule a run held its source to: by field, or for the whole record.
pub type Declared = (Option<String>, Rule);

/// Every rule `model` holds a record to, in contract order: per field its
/// presence, null, type and each constraint it declares, then the strict
/// model's closed world.
pub fn declared_rules(model: &CompiledModel) -> Vec<Declared> {
    let mut rules = Vec::new();
    for f in &model.fields {
        let mut add = |rule| rules.push((Some(f.name.clone()), rule));
        if f.required {
            add(Rule::RequiredMissing);
        }
        if !f.nullable {
            add(Rule::NullNotAllowed);
        }
        add(Rule::TypeMismatch);
        let declares = [
            (f.min_length.is_some(), Rule::MinLength),
            (f.max_length.is_some(), Rule::MaxLength),
            (f.pattern.is_some(), Rule::Pattern),
            (f.format.is_some(), Rule::Format),
            (f.allowed.is_some(), Rule::Allowed),
            (f.min.is_some(), Rule::Min),
            (f.max.is_some(), Rule::Max),
            (f.unique, Rule::Unique),
        ];
        for (declared, rule) in declares {
            if declared {
                add(rule);
            }
        }
    }
    if model.strict {
        rules.push((None, Rule::UnexpectedField));
    }
    rules
}

/// A source as OpenLineage names a dataset: `(namespace, name)`.
pub fn dataset(source: &str) -> Result<(String, String)> {
    if let Some(rest) = source.strip_prefix("kafka://") {
        if let Some((address, topic)) = rest.rsplit_once('/') {
            return Ok((format!("kafka://{address}"), topic.to_string()));
        }
    }
    if source == "stdin" {
        return Err(CovenantError::Usage {
            message: "OpenLineage names a dataset, and a stream on stdin has no name: gate a \
                      topic (--brokers) or check a file"
                .to_string(),
        });
    }
    let absolute = std::path::absolute(Path::new(source)).map_err(|e| CovenantError::Io {
        path: source.to_string(),
        source: e,
    })?;
    Ok(("file".to_string(), absolute.display().to_string()))
}

/// The START and COMPLETE events of the run `doc` reports, each source held
/// to `declared`. `warn_only`: the contract reports and never fails.
pub fn run_events(
    doc: &ReportDocument,
    declared: &[Declared],
    warn_only: bool,
) -> Result<[Value; 2]> {
    let Body::Data { checks, .. } = &doc.body else {
        return Err(CovenantError::Usage {
            message: "a contract diff has no dataset for OpenLineage".to_string(),
        });
    };
    let started = chrono::DateTime::parse_from_rfc3339(&doc.run.started_at)
        .expect("a document's own start time")
        .with_timezone(&chrono::Utc);
    let ended = started
        + chrono::Duration::milliseconds(i64::try_from(doc.run.duration_ms).unwrap_or(i64::MAX));
    let time =
        |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let severity = if warn_only { "warn" } else { "error" };
    let producer = producer();
    let job = match checks.first() {
        Some(c) => format!("{}.{}.{}", doc.kind.name(), c.contract_id, c.model),
        None => "check".to_string(),
    };
    let run_id = run_id(doc, started.timestamp_millis());
    let mut named = Vec::with_capacity(checks.len());
    for check in checks {
        named.push((dataset(&check.source)?, check));
    }
    let event = |event_type: &str, at: String, inputs: Vec<Value>| {
        json!({
            "eventType": event_type,
            "eventTime": at,
            "run": { "runId": run_id },
            "job": { "namespace": JOB_NAMESPACE, "name": job },
            "inputs": inputs,
            "outputs": [],
            "producer": producer,
            "schemaURL": EVENT_SCHEMA,
        })
    };
    let start_inputs = named
        .iter()
        .map(|((namespace, name), _)| json!({ "namespace": namespace, "name": name }))
        .collect();
    let complete_inputs = named
        .iter()
        .map(|((namespace, name), check)| {
            json!({
                "namespace": namespace,
                "name": name,
                "inputFacets": {
                    "dataQualityAssertions": {
                        "_producer": producer,
                        "_schemaURL": ASSERTIONS_SCHEMA,
                        "assertions": assertions(check, declared, severity),
                    },
                    "dataQualityMetrics": {
                        "_producer": producer,
                        "_schemaURL": METRICS_SCHEMA,
                        "rowCount": check.rows,
                        "columnMetrics": {},
                    },
                },
            })
        })
        .collect();
    Ok([
        event("START", time(started), start_inputs),
        event("COMPLETE", time(ended), complete_inputs),
    ])
}

/// Every declared rule with whether it held, then any rule the run reported
/// that the model does not declare, failed.
fn assertions(check: &CheckEntry, declared: &[Declared], severity: &str) -> Vec<Value> {
    let assertion = |column: Option<&str>, rule: &str, success: bool| {
        let mut a = json!({ "assertion": rule, "success": success, "severity": severity });
        if let Some(column) = column {
            a["column"] = json!(column);
        }
        a
    };
    // The closed world is one rule for the whole dataset, whichever columns
    // broke it.
    let broken = |field: Option<&str>, rule: Rule| {
        check.per_rule.iter().any(|c| {
            c.rule == rule.name()
                && (rule == Rule::UnexpectedField || c.field == field.unwrap_or_default())
        })
    };
    let mut out: Vec<Value> = declared
        .iter()
        .map(|(field, rule)| {
            assertion(
                field.as_deref(),
                rule.name(),
                !broken(field.as_deref(), *rule),
            )
        })
        .collect();
    for c in &check.per_rule {
        let field = (!c.field.is_empty()).then_some(c.field.as_str());
        let is_declared = declared.iter().any(|(f, r)| {
            r.name() == c.rule && (*r == Rule::UnexpectedField || f.as_deref() == field)
        });
        if !is_declared {
            out.push(assertion(field, c.rule, false));
        }
    }
    out
}

/// A UUIDv7 for the run: its start in milliseconds, then bits of the
/// SHA-256 of its document, so the same document is the same run.
fn run_id(doc: &ReportDocument, started_ms: i64) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(doc.to_json());
    let ms = (started_ms.max(0) as u64).to_be_bytes();
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(&ms[2..]);
    b[6] = 0x70 | (hash[0] & 0x0f);
    b[7] = hash[1];
    b[8] = 0x80 | (hash[2] & 0x3f);
    b[9..].copy_from_slice(&hash[3..10]);
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_and_a_topic_are_named_as_openlineage_names_them() {
        let (ns, name) = dataset("kafka://b1:9092/orders").unwrap();
        assert_eq!((ns.as_str(), name.as_str()), ("kafka://b1:9092", "orders"));
        let (ns, name) = dataset("data/orders.ndjson").unwrap();
        assert_eq!(ns, "file");
        assert!(Path::new(&name).is_absolute(), "{name}");
        assert!(name.ends_with("orders.ndjson"));
        assert!(dataset("stdin").is_err());
    }

    #[test]
    fn a_model_declares_presence_null_type_and_each_constraint_then_its_closed_world() {
        let contract = crate::spec::Contract::parse(
            r#"
covenant: 1
id: t
version: 1.0.0
models:
  m:
    strict: true
    fields:
      id: { type: string, required: true, unique: true, pattern: "^a" }
      note: { type: string, nullable: true, max_length: 5 }
      n: { type: integer, min: 0, allowed: [1, 2] }
"#,
            "<test>",
        )
        .unwrap();
        let compiled = crate::compile::CompiledContract::compile(&contract).unwrap();
        let declared = declared_rules(compiled.resolve_model(None).unwrap());
        let named: Vec<(Option<&str>, &str)> = declared
            .iter()
            .map(|(field, rule)| (field.as_deref(), rule.name()))
            .collect();
        assert_eq!(
            named,
            [
                (Some("id"), "required_missing"),
                (Some("id"), "null_not_allowed"),
                (Some("id"), "type_mismatch"),
                (Some("id"), "pattern"),
                (Some("id"), "unique"),
                (Some("note"), "type_mismatch"),
                (Some("note"), "max_length"),
                (Some("n"), "null_not_allowed"),
                (Some("n"), "type_mismatch"),
                (Some("n"), "allowed"),
                (Some("n"), "min"),
                (None, "unexpected_field"),
            ]
        );
    }
}
