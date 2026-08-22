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
}

/// The closed set of enforced rules. The serialized snake_case name is the
/// stable machine identifier used in JSON reports and DLQ envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
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
        let verdict = if self.passed(max_violations) { "PASS" } else { "FAIL" };
        let _ = writeln!(
            s,
            "{verdict}  {source}  [{id} v{version}, model {model}]",
            source = self.source,
            id = self.contract_id,
            version = self.contract_version,
            model = self.model,
        );
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
                let field = if rc.field.is_empty() { "<record>" } else { &rc.field };
                let _ = writeln!(s, "    {field:<24} {rule:<22} × {count}", rule = rc.rule, count = rc.count);
            }
        }
        if !self.samples.is_empty() {
            let _ = writeln!(s, "  samples:");
            for v in &self.samples {
                let row = v.row.map(|r| format!("row {r}")).unwrap_or_else(|| "schema".into());
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
    use super::Rule;

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
