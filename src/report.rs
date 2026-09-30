//! Violations and check reports — the evidence enforcement produces.
//!
//! Counts are always exact; *samples* (violations with row numbers and
//! offending values) are capped per (field, rule) by the contract's
//! `policy.sample_violations` so a million-row disaster still renders as a
//! readable report.

use indexmap::IndexMap;
use serde::Serialize;

/// One rule broken at one place.
#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub model: String,
    /// Absent for record-level findings (e.g. a non-object NDJSON line).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub rule: Rule,
    /// 0-based record index within the checked input, when known. Schema-level
    /// findings (missing column) have no row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<u64>,
    /// Truncated rendering of the offending value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub message: String,
    /// What the offending value was, without the value: its type, its length
    /// and a digest. Never serialized; a report document says the type and
    /// the length, and a keyed hash of the digest when it is asked for one.
    #[serde(skip)]
    pub observed: Option<Observed>,
}

/// The type of a value as its source held it: a JSON type for records, and
/// for Arrow columns the JSON type they correspond to, or `timestamp`, `date`
/// or `binary` for native temporal and binary columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Null,
    Boolean,
    Integer,
    Number,
    String,
    Array,
    Object,
    Timestamp,
    Date,
    Binary,
}

impl ValueKind {
    /// The name a report gives the type.
    pub fn name(self) -> &'static str {
        match self {
            ValueKind::Null => "null",
            ValueKind::Boolean => "boolean",
            ValueKind::Integer => "integer",
            ValueKind::Number => "number",
            ValueKind::String => "string",
            ValueKind::Array => "array",
            ValueKind::Object => "object",
            ValueKind::Timestamp => "timestamp",
            ValueKind::Date => "date",
            ValueKind::Binary => "binary",
        }
    }
}

/// An offending value, without the value: its type, its length, and the
/// SHA-256 of its canonical form. The digest is for keying a report's hash
/// with and is never written anywhere as it is: a plain hash of a low-entropy
/// value (a currency, a status) is the value, to anyone who tries the
/// candidates.
///
/// The canonical form is the type's name, a zero byte, then the value: a
/// string's own text, an integer in decimal, a number as JSON writes it (`-0`
/// as `0`), `true`/`false`, `null`, and an array or object as compact JSON. So
/// the same string, integer or boolean has the same digest in NDJSON, CSV and
/// Parquet.
#[derive(Clone, PartialEq, Eq)]
pub struct Observed {
    pub kind: ValueKind,
    /// Characters of a string, items of an array, keys of an object.
    pub length: Option<u64>,
    digest: [u8; 32],
}

impl std::fmt::Debug for Observed {
    // The digest stays out of debug output too.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Observed")
            .field("kind", &self.kind)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

impl Observed {
    fn of(kind: ValueKind, length: Option<u64>, canonical: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(kind.name().as_bytes());
        hasher.update([0u8]);
        hasher.update(canonical);
        Observed {
            kind,
            length,
            digest: hasher.finalize().into(),
        }
    }

    /// A JSON value, as a record holds it.
    pub fn json(value: &serde_json::Value) -> Self {
        use serde_json::Value;
        match value {
            Value::Null => Self::null(),
            Value::Bool(b) => Self::boolean(*b),
            Value::Number(n) => match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => Self::integer(i as i128),
                (None, Some(u)) => Self::integer(u as i128),
                _ => Self::number(n.as_f64().unwrap_or(f64::NAN)),
            },
            Value::String(s) => Self::string(s),
            Value::Array(items) => Self::of(
                ValueKind::Array,
                Some(items.len() as u64),
                value.to_string().as_bytes(),
            ),
            Value::Object(keys) => Self::of(
                ValueKind::Object,
                Some(keys.len() as u64),
                value.to_string().as_bytes(),
            ),
        }
    }

    pub fn null() -> Self {
        Self::of(ValueKind::Null, None, b"null")
    }

    pub fn boolean(b: bool) -> Self {
        Self::of(
            ValueKind::Boolean,
            None,
            if b { b"true".as_slice() } else { b"false" },
        )
    }

    pub fn integer(n: i128) -> Self {
        Self::of(ValueKind::Integer, None, n.to_string().as_bytes())
    }

    pub fn number(n: f64) -> Self {
        // As JSON writes it, so a float column's 1.5 is a record's 1.5; -0 is
        // 0, as uniqueness keys it.
        let text = if n == 0.0 {
            "0".to_string()
        } else {
            serde_json::Number::from_f64(n)
                .map(|n| n.to_string())
                .unwrap_or_else(|| n.to_string())
        };
        Self::of(ValueKind::Number, None, text.as_bytes())
    }

    pub fn string(s: &str) -> Self {
        Self::of(
            ValueKind::String,
            Some(s.chars().count() as u64),
            s.as_bytes(),
        )
    }

    /// A native temporal or binary value, by the text its column keys it by.
    pub fn native(kind: ValueKind, key: &str) -> Self {
        Self::of(kind, None, key.as_bytes())
    }

    /// The SHA-256 of the canonical form: for keying a hash with, never for
    /// writing down.
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

/// The closed set of enforced rules. The serialized snake_case name is the
/// stable machine identifier used in JSON reports and DLQ envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
// New rule kinds arrive in minor releases.
#[non_exhaustive]
pub enum Rule {
    /// NDJSON line that isn't a JSON object.
    RecordNotObject,
    RequiredMissing,
    NullNotAllowed,
    TypeMismatch,
    Pattern,
    Min,
    Max,
    MinLength,
    MaxLength,
    Allowed,
    Format,
    Unique,
    /// `strict: true` and the record/batch carries an undeclared field.
    UnexpectedField,
    /// Column declared by the contract absent from the batch schema.
    SchemaMissingField,
    /// Column present but its Arrow type can't satisfy the contract type.
    SchemaTypeMismatch,
}

impl Rule {
    /// The stable snake_case rule identifier (matches the serde rename;
    /// pinned together by a test).
    pub fn name(self) -> &'static str {
        match self {
            Rule::RecordNotObject => "record_not_object",
            Rule::RequiredMissing => "required_missing",
            Rule::NullNotAllowed => "null_not_allowed",
            Rule::TypeMismatch => "type_mismatch",
            Rule::Pattern => "pattern",
            Rule::Min => "min",
            Rule::Max => "max",
            Rule::MinLength => "min_length",
            Rule::MaxLength => "max_length",
            Rule::Allowed => "allowed",
            Rule::Format => "format",
            Rule::Unique => "unique",
            Rule::UnexpectedField => "unexpected_field",
            Rule::SchemaMissingField => "schema_missing_field",
            Rule::SchemaTypeMismatch => "schema_type_mismatch",
        }
    }
}

/// Accumulates violations with exact counts and capped samples.
#[derive(Debug)]
pub struct Collector {
    sample_cap: usize,
    total: u64,
    /// (field-or-"", rule) → exact count.
    counts: IndexMap<(String, &'static str), u64>,
    samples: Vec<Violation>,
}

impl Collector {
    /// A collector keeping at most `sample_cap` samples per (field, rule).
    pub fn new(sample_cap: usize) -> Self {
        Collector {
            sample_cap,
            total: 0,
            counts: IndexMap::new(),
            samples: Vec::new(),
        }
    }

    /// Count an already-built violation (row engines build messages eagerly
    /// because the gate needs them regardless). One cap rule lives in
    /// [`Collector::record`]; this just delegates.
    pub fn push(&mut self, v: Violation) {
        let field = v.field.clone();
        self.record(field.as_deref(), v.rule, move || v);
    }

    /// Count a violation, building the sample only while this (field, rule)
    /// is under the sample cap — columnar loops over millions of bad rows
    /// pay one counter bump, not one formatted string, per violation.
    pub fn record(&mut self, field: Option<&str>, rule: Rule, make: impl FnOnce() -> Violation) {
        self.total += 1;
        let key = (field.unwrap_or_default().to_string(), rule.name());
        let count = self.counts.entry(key).or_insert(0);
        *count += 1;
        if (*count as usize) <= self.sample_cap {
            self.samples.push(make());
        }
    }

    /// Exact violation count so far (never capped).
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Finish the run: fold counts and samples into a [`CheckReport`].
    pub fn into_report(self, header: ReportHeader, rows: u64) -> CheckReport {
        let per_rule = self
            .counts
            .into_iter()
            .map(|((field, rule), count)| RuleCount { field, rule, count })
            .collect();
        CheckReport {
            contract_id: header.contract_id,
            contract_version: header.contract_version,
            owner: header.owner,
            model: header.model,
            source: header.source,
            as_consumer: None,
            rows,
            violations: self.total,
            per_rule,
            samples: self.samples,
            unenforced: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
/// Identifying context stamped onto a [`CheckReport`].
pub struct ReportHeader {
    pub contract_id: String,
    pub contract_version: String,
    pub owner: Option<String>,
    pub model: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
/// Exact violation count for one (field, rule) pair.
pub struct RuleCount {
    /// Empty string for record-level rules.
    pub field: String,
    pub rule: &'static str,
    pub count: u64,
}

/// The result of one `covenant check` over one input.
#[derive(Debug, Serialize)]
pub struct CheckReport {
    pub contract_id: String,
    pub contract_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub model: String,
    pub source: String,
    /// Set when the run was scoped to a consumer manifest (`check
    /// --as-consumer`): the consumer id whose declared fields were the only
    /// ones validated, with strict mode off. Absent = full-contract check —
    /// without this marker a scoped report would be indistinguishable from
    /// full conformance to the named contract version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_consumer: Option<String>,
    pub rows: u64,
    pub violations: u64,
    pub per_rule: Vec<RuleCount>,
    pub samples: Vec<Violation>,
    /// Contract rules skipped because the run allowed unenforced rules
    /// (`--allow-unenforced`). Non-empty means the verdict covers only the
    /// enforceable part of the contract — a partial check, never a full one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unenforced: Vec<crate::spec::UnenforcedRule>,
}

impl CheckReport {
    /// Whether this report's violations fit within the given budget.
    pub fn passed(&self, max_violations: u64) -> bool {
        self.violations <= max_violations
    }

    /// Human rendering for terminals and CI logs.
    pub fn render_human(&self, max_violations: u64) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let verdict = if self.passed(max_violations) {
            "PASS"
        } else {
            "FAIL"
        };
        // A verdict over part of the contract must say so on the same line
        // a skimming reader (or a grep) looks at.
        let partial = if self.unenforced.is_empty() {
            ""
        } else {
            " (partial)"
        };
        let _ = writeln!(
            s,
            "{verdict}{partial}  {source}  [{id} v{version}, model {model}]",
            source = self.source,
            id = self.contract_id,
            version = self.contract_version,
            model = self.model,
        );
        if !self.unenforced.is_empty() {
            let _ = writeln!(
                s,
                "  not enforced (--allow-unenforced): {} contract rule(s)\n{}",
                self.unenforced.len(),
                crate::spec::render_unenforced(&self.unenforced)
                    .lines()
                    .map(|l| format!("  {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let _ = writeln!(
            s,
            "  rows checked: {}   violations: {}{}",
            self.rows,
            self.violations,
            if max_violations > 0 {
                format!(" (budget {max_violations})")
            } else {
                String::new()
            }
        );
        if let Some(owner) = &self.owner {
            if !self.passed(max_violations) {
                let _ = writeln!(s, "  contract owner: {owner}");
            }
        }
        if !self.per_rule.is_empty() {
            let _ = writeln!(s, "  by rule:");
            for rc in &self.per_rule {
                let field = if rc.field.is_empty() {
                    "<record>"
                } else {
                    &rc.field
                };
                let _ = writeln!(
                    s,
                    "    {field:<24} {rule:<22} × {count}",
                    rule = rc.rule,
                    count = rc.count
                );
            }
        }
        if !self.samples.is_empty() {
            let _ = writeln!(s, "  samples:");
            for v in &self.samples {
                let row = v
                    .row
                    .map(|r| format!("row {r}"))
                    .unwrap_or_else(|| "schema".into());
                let _ = writeln!(s, "    [{row}] {}", v.message);
            }
        }
        s
    }
}

/// Render a value for a violation message, truncated so a 10 MB blob in a bad
/// record can't flood the report.
pub fn preview(value: &serde_json::Value) -> String {
    let raw = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    truncate(&raw, 64)
}

/// Truncate a string to `max_chars` Unicode scalars, appending an ellipsis.
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::{Observed, Rule, ValueKind};

    #[test]
    fn a_value_is_observed_alike_from_a_record_and_from_a_column() {
        use serde_json::json;
        assert_eq!(Observed::json(&json!("BTC")), Observed::string("BTC"));
        assert_eq!(Observed::json(&json!(-3)), Observed::integer(-3));
        assert_eq!(
            Observed::json(&json!(u64::MAX)),
            Observed::integer(u64::MAX as i128)
        );
        assert_eq!(Observed::json(&json!(1.5)), Observed::number(1.5));
        assert_eq!(Observed::json(&json!(-0.0)), Observed::number(0.0));
        assert_eq!(Observed::json(&json!(true)), Observed::boolean(true));
        assert_eq!(Observed::json(&json!(null)), Observed::null());
        assert_eq!(Observed::string("héllo").length, Some(5));
        assert_eq!(Observed::json(&json!([1, 2])).length, Some(2));
        assert_eq!(Observed::json(&json!({"a": 1})).kind, ValueKind::Object);
        assert_eq!(Observed::integer(7).length, None);
        assert_ne!(Observed::string("1"), Observed::integer(1));
        // Debug output says the type and length; the digest stays out of it.
        let shown = format!("{:?}", Observed::string("BTC"));
        assert!(
            shown.contains("String") && shown.contains("Some(3)"),
            "{shown}"
        );
        assert!(!shown.contains("digest"), "{shown}");
    }

    const ALL_RULES: [Rule; 15] = [
        Rule::RecordNotObject,
        Rule::RequiredMissing,
        Rule::NullNotAllowed,
        Rule::TypeMismatch,
        Rule::Pattern,
        Rule::Min,
        Rule::Max,
        Rule::MinLength,
        Rule::MaxLength,
        Rule::Allowed,
        Rule::Format,
        Rule::Unique,
        Rule::UnexpectedField,
        Rule::SchemaMissingField,
        Rule::SchemaTypeMismatch,
    ];

    /// `Violation.rule` serializes via serde, `RuleCount.rule` via name() —
    /// the same rule must ship under one identifier in the same JSON report.
    #[test]
    fn serde_name_matches_manual_name() {
        for rule in ALL_RULES {
            let serialized = serde_json::to_value(rule).unwrap();
            assert_eq!(serialized, serde_json::json!(rule.name()), "{rule:?}");
        }
    }
}
