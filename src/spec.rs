//! The contract document: what producers promise consumers.
//!
//! A Covenant contract is a small YAML/JSON document — one logical dataset
//! (`id`, semver `version`, `owner`) holding one or more `models` (a topic, a
//! table, a stream), each a map of typed fields with constraints, plus an
//! enforcement `policy`. The shape borrows vocabulary from the open
//! data-contract specs (datacontract-specification, ODCS) but keeps only what
//! the runtime can *enforce* — anything the engine can't check at the boundary
//! stays out of the schema on purpose.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::{CovenantError, Result};

/// Top-level contract document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    /// Spec revision this document is written against. Currently always `1`.
    pub covenant: u32,
    /// Stable machine identifier for the dataset (`orders`, `clickstream`).
    pub id: String,
    /// Semver version of the *contract* (not the data). `covenant diff`
    /// enforces that breaking changes bump major, risky changes bump minor.
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Who to page when the contract is violated — surfaced verbatim in
    /// reports so a failing CI job names the owning team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The datasets governed by this contract, in author order.
    pub models: IndexMap<String, Model>,
    #[serde(default)]
    pub policy: Policy,
}

/// One governed dataset (a Kafka topic, a warehouse table, a stream).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When true, fields not declared in the contract are violations
    /// (closed-world schema). When false, extra fields pass through.
    #[serde(default)]
    pub strict: bool,
    pub fields: IndexMap<String, Field>,
}

/// A single field's promise.
///
/// Presence and nullness are separate axes: `required` says the key must be
/// present in every record; `nullable` says an explicit null is acceptable.
/// Both default to the strict reading a consumer would want (`required:
/// false`, `nullable: false`) — an optional field may be absent, but if it
/// shows up it must carry a real value unless the contract says otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    #[serde(rename = "type")]
    pub ty: FieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub nullable: bool,
    /// Value must be unique across the checked file/stream. Enforced with an
    /// in-memory set — exact in CI checks; in `gate` mode it is exact but
    /// unbounded, so long-running gates should prefer keyed compaction
    /// upstream (documented trade-off).
    #[serde(default)]
    pub unique: bool,
    /// Anchored automatically? No — the regex is used as written; authors
    /// anchor with `^…$` when they mean whole-value match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Inclusive numeric bounds (integer and float fields).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Inclusive length bounds (string fields, measured in Unicode scalars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    /// Closed value set (enum). Values must be type-compatible with `type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<serde_json::Value>>,
    /// Extra string shape on top of `type: string`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<StringFormat>,
    /// Advisory deprecation marker: `deprecated: "use X instead"` (a
    /// migration note) or plain `deprecated: true`. Enforcement is
    /// UNCHANGED — deprecation is a migration signal for consumers
    /// (surfaced by `consumer-check` warnings, the diff classifier, and
    /// deprecation countdowns), never a validation rule.
    #[serde(
        default,
        deserialize_with = "de_deprecated",
        skip_serializing_if = "Option::is_none"
    )]
    pub deprecated: Option<String>,
}

/// Accept `deprecated: true|false` alongside `deprecated: "<note>"` —
/// authors write both, and rejecting the boolean form would be a needless
/// papercut. `true` normalizes to an empty note; `false` to absent.
fn de_deprecated<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Dep {
        Flag(bool),
        Note(String),
    }
    Ok(match Option::<Dep>::deserialize(deserializer)? {
        None | Some(Dep::Flag(false)) => None,
        Some(Dep::Flag(true)) => Some(String::new()),
        Some(Dep::Note(note)) => Some(note),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// The type a field promises. JSON and Arrow representations are mapped
/// per type by the engines.
pub enum FieldType {
    String,
    Integer,
    Float,
    Boolean,
    /// RFC 3339 timestamp string in JSON; any Arrow `Timestamp(..)` column.
    Timestamp,
    /// `YYYY-MM-DD` string in JSON; Arrow `Date32`/`Date64` column.
    Date,
    /// Canonical 8-4-4-4-12 hex UUID string.
    Uuid,
}

impl FieldType {
    /// The spec-facing lowercase type name (used in messages).
    pub fn name(self) -> &'static str {
        match self {
            FieldType::String => "string",
            FieldType::Integer => "integer",
            FieldType::Float => "float",
            FieldType::Boolean => "boolean",
            FieldType::Timestamp => "timestamp",
            FieldType::Date => "date",
            FieldType::Uuid => "uuid",
        }
    }

    /// Types whose JSON representation is a string.
    pub fn is_stringly(self) -> bool {
        matches!(
            self,
            FieldType::String | FieldType::Timestamp | FieldType::Date | FieldType::Uuid
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Extra shape on top of `type: string`.
pub enum StringFormat {
    Email,
    Uri,
}

impl StringFormat {
    /// The spec-facing lowercase format name (used in messages).
    pub fn name(self) -> &'static str {
        match self {
            StringFormat::Email => "email",
            StringFormat::Uri => "uri",
        }
    }
}

/// What enforcement does when data breaks the promise.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub on_violation: OnViolation,
    /// Violations tolerated before a `check` run fails (a budget for known
    /// dirt while a producer cleans up). 0 = any violation fails.
    #[serde(default)]
    pub max_violations: u64,
    /// Example violations retained per (field, rule) in reports; total counts
    /// are always exact regardless of this cap.
    #[serde(default = "default_sample_violations")]
    pub sample_violations: usize,
}

fn default_sample_violations() -> usize {
    10
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            on_violation: OnViolation::Block,
            max_violations: 0,
            sample_violations: default_sample_violations(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// What a violation does at an enforcement point.
pub enum OnViolation {
    /// Fail CI / withhold records from the gate's clean output.
    #[default]
    Block,
    /// Report everything, fail nothing, pass records through.
    Warn,
}

/// A `covenant validate` finding about the contract document itself.
#[derive(Debug, Clone, Serialize)]
pub struct LintFinding {
    /// `error` findings make the contract unusable for enforcement;
    /// `warning` findings are smells that still enforce deterministically.
    pub level: LintLevel,
    /// Dotted location, e.g. `models.orders.fields.amount`.
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
/// Severity of a contract lint finding.
pub enum LintLevel {
    Error,
    Warning,
}

impl Contract {
    /// Parse a contract from YAML (JSON is valid YAML, so `.json` contracts
    /// parse through the same path).
    pub fn parse(source: &str, origin: &str) -> Result<Contract> {
        serde_yaml::from_str(source).map_err(|e| CovenantError::ContractParse {
            path: origin.to_string(),
            message: e.to_string(),
        })
    }

    /// Resolve which model a run targets, without compiling: the requested
    /// name must exist, and "no name" is only unambiguous for a one-model
    /// contract. Mirrors `CompiledContract::resolve_model` for callers that
    /// need the name before deciding what to compile.
    pub fn resolve_model_name(&self, requested: Option<&str>) -> Result<&str> {
        let available = || self.models.keys().cloned().collect::<Vec<_>>().join(", ");
        match requested {
            Some(name) => match self.models.get_key_value(name) {
                Some((key, _)) => Ok(key.as_str()),
                None => Err(CovenantError::ModelNotFound {
                    contract_id: self.id.clone(),
                    model: name.to_string(),
                    available: available(),
                }),
            },
            None if self.models.len() == 1 => {
                Ok(self.models.keys().next().expect("len checked").as_str())
            }
            None => Err(CovenantError::ModelAmbiguous {
                contract_id: self.id.clone(),
                count: self.models.len(),
                available: available(),
            }),
        }
    }

    /// Read and parse a contract file.
    pub fn from_path(path: &std::path::Path) -> Result<Contract> {
        let text = std::fs::read_to_string(path).map_err(|e| CovenantError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        Contract::parse(&text, &path.display().to_string())
    }

    /// Lint the contract itself. Enforcement refuses to run against a
    /// contract with `error`-level findings — a gate that half-understands
    /// its contract is worse than no gate.
    pub fn lint(&self) -> Vec<LintFinding> {
        let mut out = Vec::new();
        let err = |path: &str, message: String| LintFinding {
            level: LintLevel::Error,
            path: path.to_string(),
            message,
        };
        let warn = |path: &str, message: String| LintFinding {
            level: LintLevel::Warning,
            path: path.to_string(),
            message,
        };

        if self.covenant != 1 {
            out.push(err(
                "covenant",
                format!("unsupported spec revision {} (this runtime speaks 1)", self.covenant),
            ));
        }
        if self.id.trim().is_empty() {
            out.push(err("id", "contract id must be non-empty".into()));
        }
        if semver::Version::parse(&self.version).is_err() {
            out.push(err(
                "version",
                format!("version {:?} is not valid semver (expected e.g. 1.2.0)", self.version),
            ));
        }
        if self.models.is_empty() {
            out.push(err("models", "contract declares no models".into()));
        }
        if self.owner.is_none() {
            out.push(warn(
                "owner",
                "no owner set — violation reports cannot name the responsible team".into(),
            ));
        }

        for (model_name, model) in &self.models {
            let mpath = format!("models.{model_name}");
            if model.fields.is_empty() {
                out.push(err(&mpath, "model declares no fields".into()));
            }
            for (field_name, field) in &model.fields {
                let fpath = format!("{mpath}.fields.{field_name}");
                lint_field(field, &fpath, &mut out, &err, &warn);
            }
        }
        out
    }

    /// True when no `error`-level lint findings exist.
    pub fn is_enforceable(&self) -> bool {
        self.lint().iter().all(|f| f.level != LintLevel::Error)
    }
}

fn lint_field(
    field: &Field,
    fpath: &str,
    out: &mut Vec<LintFinding>,
    err: &impl Fn(&str, String) -> LintFinding,
    warn: &impl Fn(&str, String) -> LintFinding,
) {
    let numeric = matches!(field.ty, FieldType::Integer | FieldType::Float);

    // pattern / length / format are DELIBERATELY restricted to `string`
    // even though timestamp/date/uuid are string-shaped on the wire: the
    // engines do not evaluate these constraints on those types, and a lint
    // that admits a constraint the runtime silently skips would recreate
    // the half-enforcement failure this tool exists to kill. Widen the lint
    // only together with the engines.
    if let Some(p) = &field.pattern {
        if field.ty != FieldType::String {
            out.push(err(
                fpath,
                format!("pattern applies only to string fields, not {}", field.ty.name()),
            ));
        }
        if let Err(e) = regex::Regex::new(p) {
            out.push(err(fpath, format!("pattern does not compile: {e}")));
        }
    }
    if field.format.is_some() && field.ty != FieldType::String {
        out.push(err(
            fpath,
            format!("format applies only to string fields, not {}", field.ty.name()),
        ));
    }
    if (field.min.is_some() || field.max.is_some()) && !numeric {
        out.push(err(
            fpath,
            format!("min/max apply only to numeric fields, not {}", field.ty.name()),
        ));
    }
    for (name, bound) in [("min", field.min), ("max", field.max)] {
        if let Some(b) = bound {
            if !b.is_finite() {
                out.push(err(fpath, format!("{name} must be a finite number, got {b}")));
            }
        }
    }
    if let (Some(min), Some(max)) = (field.min, field.max) {
        if min > max {
            out.push(err(fpath, format!("min ({min}) exceeds max ({max})")));
        }
    }
    if (field.min_length.is_some() || field.max_length.is_some()) && field.ty != FieldType::String {
        out.push(err(
            fpath,
            format!("min_length/max_length apply only to string fields, not {}", field.ty.name()),
        ));
    }
    if let (Some(lo), Some(hi)) = (field.min_length, field.max_length) {
        if lo > hi {
            out.push(err(fpath, format!("min_length ({lo}) exceeds max_length ({hi})")));
        }
    }
    match &field.allowed {
        Some(values) if values.is_empty() => {
            out.push(err(fpath, "allowed is empty — no value could ever pass".into()));
        }
        Some(values) => {
            for v in values {
                let compatible = match field.ty {
                    FieldType::String
                    | FieldType::Timestamp
                    | FieldType::Date
                    | FieldType::Uuid => v.is_string(),
                    FieldType::Integer => v.as_i64().is_some() || v.as_u64().is_some(),
                    FieldType::Float => v.is_number(),
                    FieldType::Boolean => v.is_boolean(),
                };
                if !compatible {
                    out.push(err(
                        fpath,
                        format!("allowed value {v} is not a {}", field.ty.name()),
                    ));
                    continue;
                }
                // A temporal/uuid allowed value the engines' shape check
                // would reject can never be matched by a conforming value —
                // that entry is unsatisfiable, the same class of authoring
                // bug as an empty allowed set.
                if let Some(s) = v.as_str() {
                    let shape_ok = match field.ty {
                        FieldType::Timestamp => crate::compile::shape::is_timestamp(s),
                        FieldType::Date => crate::compile::shape::is_date(s),
                        FieldType::Uuid => crate::compile::shape::is_uuid(s),
                        _ => true,
                    };
                    if !shape_ok {
                        out.push(err(
                            fpath,
                            format!(
                                "allowed value {v} is not a valid {} — no conforming value could ever match it",
                                field.ty.name()
                            ),
                        ));
                    }
                }
            }
        }
        None => {}
    }
    if field.unique && field.nullable {
        out.push(warn(
            fpath,
            "unique + nullable: nulls are exempt from uniqueness, which is usually surprising".into(),
        ));
    }
}
