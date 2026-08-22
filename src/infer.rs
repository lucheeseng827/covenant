//! `covenant infer` — bootstrap a DRAFT contract from a sample of real
//! data. Every other command assumes a contract already exists; this is the
//! on-ramp that writes the first one.
//!
//! The discipline here is the inverse of enforcement: a checker must be
//! certain before it fails a record, and an inferrer must be HONEST about
//! what a sample can and cannot prove. A sample can prove a value appeared;
//! it can never prove one never will. So:
//!
//! - Types, nullability, and presence come straight from what was observed,
//!   and are emitted as rules.
//! - Ranges, enums, patterns, and uniqueness are *guesses from a window* —
//!   they are emitted only where the sample is wide enough to be suggestive,
//!   and every one of them is marked in the draft with a `# confirm:` note
//!   saying what it was inferred from.
//! - Anything ambiguous (mixed types, empty columns) widens to the loosest
//!   promise that still holds, and says so.
//!
//! The draft always parses and compiles — `covenant validate` on the output
//! of `covenant infer` is clean by construction (enforced by tests), and the
//! version starts at `0.1.0` because a drafted contract is not yet a promise
//! anyone should rely on.
//!
//! Shape detection (uuid/date/timestamp/email/uri) calls the same
//! [`crate::compile::shape`] predicates enforcement uses, so a type this
//! module drafts is a type the engines will accept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::compile::shape;
use crate::error::{CovenantError, Result};
use crate::spec::{Contract, Field, FieldType, Model, Policy, StringFormat};

/// Records scanned per run unless `--sample` says otherwise. Big enough to
/// see the tail of most enums, small enough to stay instant on a huge file.
pub const DEFAULT_SAMPLE: u64 = 10_000;
/// Distinct string values at or below which a column may become `allowed:`.
pub const DEFAULT_MAX_ENUM: usize = 12;
/// Distinct values tracked per field before enum/unique detection gives up.
const DISTINCT_CAP: usize = 512;
/// Samples required before a stable observation is suggestive at all — below
/// this, "every value was distinct" (etc.) is noise, not signal.
const MIN_SAMPLES_FOR_HINTS: u64 = 20;

/// How much the sample actually supports a line in the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Observed directly and emitted as a rule (types, nullability).
    Observed,
    /// A guess from a finite window — emitted, but flagged for a human.
    Confirm,
}

/// One thing the draft wants its reader to know, anchored at a spec path.
#[derive(Debug, Clone)]
pub struct Note {
    pub path: String,
    pub message: String,
    pub confidence: Confidence,
}

/// A drafted contract plus everything the sample could not settle.
#[derive(Debug)]
pub struct Draft {
    pub contract: Contract,
    pub notes: Vec<Note>,
    /// Records actually scanned, and whether the input was exhausted.
    pub sampled: u64,
    pub truncated: bool,
    pub sources: Vec<String>,
}

/// Knobs for [`infer_paths`].
#[derive(Debug, Clone)]
pub struct InferOptions {
    /// Contract id (defaults to the first file's stem, sanitized).
    pub id: Option<String>,
    /// Model name (defaults to the contract id).
    pub model: Option<String>,
    /// Records to scan; 0 means the whole input.
    pub sample: u64,
    /// Distinct-value ceiling for `allowed:` suggestions; 0 disables enums.
    pub max_enum: usize,
    /// Emit observed numeric ranges / string lengths as `min`/`max` rules.
    pub ranges: bool,
}

impl Default for InferOptions {
    /// Sensible sampling defaults: a wide-enough window, enums on, ranges on.
    fn default() -> Self {
        InferOptions {
            id: None,
            model: None,
            sample: DEFAULT_SAMPLE,
            max_enum: DEFAULT_MAX_ENUM,
            ranges: true,
        }
    }
}

/// One observed value, normalized across every reader (JSON records, Arrow
/// arrays) so the accumulator below is written once.
#[derive(Debug, Clone, Copy)]
enum Obs<'a> {
    Null,
    Bool,
    Int(i64),
    Float(f64),
    Str(&'a str),
    /// A JSON object/array or an Arrow type with no contract equivalent.
    Nested(&'static str),
}

/// Everything one field's values said about themselves.
#[derive(Debug, Default)]
struct FieldObs {
    present: u64,
    nulls: u64,
    ints: u64,
    floats: u64,
    bools: u64,
    strings: u64,
    nested: u64,
    nested_kind: Option<&'static str>,
    num_min: Option<f64>,
    num_max: Option<f64>,
    /// Set once a value rules `integer` out (a fraction, or a
    /// non-numeric type). Inverted so the derived Default is correct.
    non_integral: bool,
    len_min: Option<usize>,
    len_max: Option<usize>,
    uuids: u64,
    dates: u64,
    timestamps: u64,
    emails: u64,
    uris: u64,
    empty_strings: u64,
    distinct: BTreeMap<String, u64>,
    distinct_overflow: bool,
}

impl FieldObs {
    /// Fold one value into the running picture of this field.
    fn observe(&mut self, obs: Obs<'_>) {
        self.present += 1;
        match obs {
            Obs::Null => self.nulls += 1,
            Obs::Bool => {
                self.bools += 1;
                self.non_integral = true;
            }
            Obs::Int(i) => {
                self.ints += 1;
                self.note_number(i as f64);
            }
            Obs::Float(f) => {
                self.floats += 1;
                // A JSON `2.0` is a float token even though it is integral;
                // only a fractional part rules `integer` out.
                if f.fract() != 0.0 || !f.is_finite() {
                    self.non_integral = true;
                }
                self.note_number(f);
            }
            Obs::Str(s) => {
                self.strings += 1;
                self.non_integral = true;
                self.note_string(s);
            }
            Obs::Nested(kind) => {
                self.nested += 1;
                self.non_integral = true;
                self.nested_kind.get_or_insert(kind);
            }
        }
    }

    /// Track the numeric range. Non-finite values carry no bound.
    fn note_number(&mut self, v: f64) {
        if !v.is_finite() {
            return;
        }
        self.num_min = Some(self.num_min.map_or(v, |m: f64| m.min(v)));
        self.num_max = Some(self.num_max.map_or(v, |m: f64| m.max(v)));
    }

    /// Track everything a string value says: length, shape (uuid/date/
    /// timestamp/email/uri, via the enforcement predicates), and the
    /// distinct set that enum/uniqueness hints are drawn from.
    fn note_string(&mut self, s: &str) {
        let len = s.chars().count();
        self.len_min = Some(self.len_min.map_or(len, |m: usize| m.min(len)));
        self.len_max = Some(self.len_max.map_or(len, |m: usize| m.max(len)));
        if s.is_empty() {
            self.empty_strings += 1;
        }
        if shape::is_uuid(s) {
            self.uuids += 1;
        }
        if shape::is_date(s) {
            self.dates += 1;
        }
        if shape::is_timestamp(s) {
            self.timestamps += 1;
        }
        if shape::matches_format(StringFormat::Email, s) {
            self.emails += 1;
        }
        if shape::matches_format(StringFormat::Uri, s) {
            self.uris += 1;
        }
        if !self.distinct_overflow {
            if self.distinct.len() >= DISTINCT_CAP && !self.distinct.contains_key(s) {
                self.distinct_overflow = true;
                self.distinct.clear();
            } else {
                *self.distinct.entry(s.to_string()).or_insert(0) += 1;
            }
        }
    }

    /// Values that carried actual data (present, not null).
    fn valued(&self) -> u64 {
        self.present - self.nulls
    }

    /// The single type every non-null value agreed on, if there is one.
    fn resolve_type(&self, path: &str, notes: &mut Vec<Note>) -> FieldType {
        let valued = self.valued();
        if valued == 0 {
            notes.push(Note {
                path: path.to_string(),
                message: "every sampled value was null or absent — type is a guess (string)".into(),
                confidence: Confidence::Confirm,
            });
            return FieldType::String;
        }
        if self.nested > 0 {
            notes.push(Note {
                path: path.to_string(),
                message: format!(
                    "{} of {valued} values were {} — nested data has no contract type yet, \
                     so this is drafted as string; split it into its own model or drop it",
                    self.nested,
                    self.nested_kind.unwrap_or("nested"),
                ),
                confidence: Confidence::Confirm,
            });
            return FieldType::String;
        }
        // Mixed families widen to string, which is the only promise that
        // still holds — and say so, because a mixed column is usually a bug
        // in the producer, not a contract decision.
        let families = [
            self.bools > 0,
            self.ints + self.floats > 0,
            self.strings > 0,
        ]
        .iter()
        .filter(|f| **f)
        .count();
        if families > 1 {
            let mut seen = Vec::new();
            if self.bools > 0 {
                seen.push(format!("boolean×{}", self.bools));
            }
            if self.ints + self.floats > 0 {
                seen.push(format!("number×{}", self.ints + self.floats));
            }
            if self.strings > 0 {
                seen.push(format!("string×{}", self.strings));
            }
            notes.push(Note {
                path: path.to_string(),
                message: format!(
                    "mixed value types in the sample ({}) — drafted as string, the only \
                     type that covers them all; a mixed column is usually a producer bug",
                    seen.join(", "),
                ),
                confidence: Confidence::Confirm,
            });
            return FieldType::String;
        }
        if self.bools > 0 {
            return FieldType::Boolean;
        }
        if self.ints + self.floats > 0 {
            // Every value integral (including `2.0`) → integer, but say so
            // when the float token appeared, since the producer may widen.
            if !self.non_integral {
                if self.floats > 0 {
                    notes.push(Note {
                        path: path.to_string(),
                        message: format!(
                            "{} value(s) were written as floats but all were integral — \
                             drafted as integer; use float if fractions are possible",
                            self.floats,
                        ),
                        confidence: Confidence::Confirm,
                    });
                }
                return FieldType::Integer;
            }
            return FieldType::Float;
        }
        // All strings: promote to the narrowest shape EVERY value satisfies.
        let all = self.strings;
        if all > 0 && self.uuids == all {
            return FieldType::Uuid;
        }
        if all > 0 && self.dates == all {
            return FieldType::Date;
        }
        if all > 0 && self.timestamps == all {
            return FieldType::Timestamp;
        }
        FieldType::String
    }

    /// Build the drafted field, appending everything the sample could not
    /// settle to `notes`.
    fn finish(
        &self,
        name: &str,
        model: &str,
        records: u64,
        opts: &InferOptions,
        notes: &mut Vec<Note>,
    ) -> Field {
        let path = format!("models.{model}.fields.{name}");
        let ty = self.resolve_type(&path, notes);
        let valued = self.valued();

        let mut field = Field {
            ty,
            description: None,
            // `required` (the key is present) and `nullable` (the value may
            // be null) are orthogonal in the spec: a field that always
            // appears but is sometimes null is required AND nullable.
            required: self.present == records,
            nullable: self.nulls > 0,
            unique: false,
            pattern: None,
            min: None,
            max: None,
            min_length: None,
            max_length: None,
            allowed: None,
            format: None,
            deprecated: None,
        };

        if self.present < records {
            notes.push(Note {
                path: path.clone(),
                message: format!(
                    "absent from {} of {records} sampled records — drafted optional; make it \
                     required if the producer is supposed to always send it",
                    records - self.present,
                ),
                confidence: Confidence::Confirm,
            });
        }
        if self.nulls > 0 {
            notes.push(Note {
                path: path.clone(),
                message: format!(
                    "{} of {} present values were null",
                    self.nulls, self.present
                ),
                confidence: Confidence::Observed,
            });
        }

        // A string format is a promise about EVERY value, so only draft one
        // when every value in the sample kept it.
        if ty == FieldType::String && valued > 0 {
            if self.emails == self.strings && self.strings > 0 {
                field.format = Some(StringFormat::Email);
            } else if self.uris == self.strings && self.strings > 0 {
                field.format = Some(StringFormat::Uri);
            }
        }

        // Enum: few distinct values, seen often enough that the tail is
        // unlikely to hold surprises.
        if ty == FieldType::String
            && field.format.is_none()
            && opts.max_enum > 0
            && !self.distinct_overflow
            && !self.distinct.is_empty()
            && self.distinct.len() <= opts.max_enum
            && valued >= MIN_SAMPLES_FOR_HINTS
            && (self.distinct.len() as u64) * 4 <= valued
        {
            field.allowed = Some(
                self.distinct
                    .keys()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            );
            notes.push(Note {
                path: format!("{path}.allowed"),
                message: format!(
                    "{} distinct values across {valued} samples — a sample cannot prove the \
                     set is closed; drop this if new values are legal",
                    self.distinct.len(),
                ),
                confidence: Confidence::Confirm,
            });
        }

        // A pattern is only worth drafting when the whole sample shares one.
        if ty == FieldType::String && field.format.is_none() && field.allowed.is_none() {
            if let Some(pattern) = self.suggest_pattern() {
                notes.push(Note {
                    path: format!("{path}.pattern"),
                    message: format!(
                        "every sampled value matched {pattern} — verify it holds for values \
                         the sample never saw",
                    ),
                    confidence: Confidence::Confirm,
                });
                field.pattern = Some(pattern);
            }
        }

        if opts.ranges && valued > 0 {
            match ty {
                FieldType::Integer | FieldType::Float => {
                    if let (Some(lo), Some(hi)) = (self.num_min, self.num_max) {
                        // Ranges from a window are the classic false gate:
                        // emit them, but never silently.
                        field.min = Some(lo);
                        field.max = Some(hi);
                        notes.push(Note {
                            path: format!("{path}.min/max"),
                            message: format!(
                                "observed range [{}, {}] over {valued} values — widen or delete \
                                 these before enforcing, a sample is not a bound",
                                render_number(lo),
                                render_number(hi),
                            ),
                            confidence: Confidence::Confirm,
                        });
                    }
                }
                FieldType::String => {
                    if let (Some(lo), Some(hi)) = (self.len_min, self.len_max) {
                        if field.allowed.is_none() && field.pattern.is_none() && lo != hi {
                            notes.push(Note {
                                path: format!("{path}.max_length"),
                                message: format!(
                                    "observed lengths {lo}–{hi}; no max_length drafted — add one \
                                     if the sink has a column width"
                                ),
                                confidence: Confidence::Confirm,
                            });
                        } else if lo == hi
                            && lo > 0
                            && field.allowed.is_none()
                            // "every value was exactly N characters" reads as
                            // a strong claim, so it needs more than a couple
                            // of values behind it — the same floor the enum
                            // and pattern guesses use.
                            && valued >= MIN_SAMPLES_FOR_HINTS
                        {
                            field.min_length = Some(lo);
                            field.max_length = Some(hi);
                            notes.push(Note {
                                path: format!("{path}.min_length/max_length"),
                                message: format!(
                                    "every sampled value was exactly {lo} characters",
                                ),
                                confidence: Confidence::Confirm,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        if self.empty_strings > 0 && ty == FieldType::String {
            notes.push(Note {
                path: path.clone(),
                message: format!(
                    "{} value(s) were the empty string — often a null in disguise; consider \
                     min_length: 1",
                    self.empty_strings,
                ),
                confidence: Confidence::Confirm,
            });
        }

        // Uniqueness is NEVER drafted as a rule: a sample cannot prove it,
        // and a wrong `unique: true` fails clean data in production.
        if !self.distinct_overflow
            && valued >= MIN_SAMPLES_FOR_HINTS
            && self.strings > 0
            && self.distinct.len() as u64 == valued
        {
            notes.push(Note {
                path: format!("{path}.unique"),
                message: format!(
                    "all {valued} sampled values were distinct — consider `unique: true` if \
                     this is a key (not drafted: a sample cannot prove uniqueness)",
                ),
                confidence: Confidence::Confirm,
            });
        }

        field
    }

    /// A conservative `^prefix[class]{n}$` when every value shares a literal
    /// prefix and a fixed-width tail from one character class.
    fn suggest_pattern(&self) -> Option<String> {
        if self.distinct_overflow
            || self.distinct.len() < 2
            || self.valued() < MIN_SAMPLES_FOR_HINTS
        {
            return None;
        }
        let values: Vec<&str> = self.distinct.keys().map(String::as_str).collect();
        if values.iter().any(|v| !v.is_ascii() || v.is_empty()) {
            return None;
        }
        // Longest common prefix, trimmed to end at a separator so we do not
        // "learn" a shared first letter of ordinary words.
        let first = values[0];
        let mut prefix_len = first.len();
        for v in &values[1..] {
            prefix_len = prefix_len.min(
                first
                    .bytes()
                    .zip(v.bytes())
                    .take_while(|(a, b)| a == b)
                    .count(),
            );
        }
        let prefix = &first[..prefix_len];
        let cut = prefix.rfind(['_', '-', ':', '/'])?;
        let prefix = &prefix[..=cut];
        if prefix.is_empty() {
            return None;
        }
        let tails: Vec<&str> = values.iter().map(|v| &v[prefix.len()..]).collect();
        let width = tails[0].len();
        if width == 0 || tails.iter().any(|t| t.len() != width) {
            return None;
        }
        let class = if tails.iter().all(|t| t.bytes().all(|b| b.is_ascii_digit())) {
            "0-9"
        } else if tails.iter().all(|t| {
            t.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        }) {
            "a-z0-9"
        } else if tails
            .iter()
            .all(|t| t.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            "A-Za-z0-9"
        } else {
            return None;
        };
        Some(format!("^{}[{class}]{{{width}}}$", regex_escape(prefix)))
    }
}

/// Escape a literal for use inside a regex (the prefix is data-derived).
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Render a float the way a contract author would write it (no `1` → `1.0`
/// surprises for integers, no exponent noise).
fn render_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The whole-sample accumulator: field observations in first-seen order,
/// plus the bookkeeping the model-level draft needs.
#[derive(Debug, Default)]
struct Sampler {
    fields: IndexMap<String, FieldObs>,
    records: u64,
    /// Distinct field-name sets seen (capped). Counting "differs from the
    /// first record" would be meaningless when the first record happens to
    /// be the minority shape — what matters is whether the shape is stable.
    shapes: std::collections::BTreeSet<Vec<String>>,
    shapes_overflow: bool,
    not_objects: u64,
}

impl Sampler {
    /// Fold one record in, remembering its field set so the model-level
    /// `strict` call can tell a settled shape from a moving one.
    fn record<'a>(&mut self, values: impl Iterator<Item = (&'a str, Obs<'a>)>) {
        self.records += 1;
        let mut keys: Vec<String> = Vec::new();
        for (name, obs) in values {
            keys.push(name.to_string());
            self.fields
                .entry(name.to_string())
                .or_default()
                .observe(obs);
        }
        keys.sort();
        if !self.shapes_overflow {
            if self.shapes.len() >= 64 && !self.shapes.contains(&keys) {
                self.shapes_overflow = true;
            } else {
                self.shapes.insert(keys);
            }
        }
    }
}

/// Infer a draft contract from one or more data files.
pub fn infer_paths(paths: &[PathBuf], opts: &InferOptions) -> Result<Draft> {
    if paths.is_empty() {
        return Err(CovenantError::Usage {
            message: "no data files given to infer from".into(),
        });
    }
    let mut sampler = Sampler::default();
    let limit = if opts.sample == 0 {
        u64::MAX
    } else {
        opts.sample
    };
    let mut truncated = false;
    for path in paths {
        if sampler.records >= limit {
            truncated = true;
            break;
        }
        let format = crate::sources::DataFormat::infer(path)?;
        let before = sampler.records;
        match format {
            crate::sources::DataFormat::Ndjson => sample_ndjson(path, limit, &mut sampler)?,
            crate::sources::DataFormat::Csv => sample_csv(path, limit, &mut sampler)?,
            crate::sources::DataFormat::Parquet => sample_parquet(path, limit, &mut sampler)?,
        }
        let _ = before;
    }
    if sampler.records >= limit && opts.sample != 0 {
        truncated = true;
    }
    if sampler.records == 0 {
        return Err(CovenantError::Usage {
            message: format!(
                "no records found in {} — cannot infer a contract from nothing",
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        });
    }

    let id = match &opts.id {
        Some(id) => id.clone(),
        None => stem_id(&paths[0]),
    };
    let model_name = opts.model.clone().unwrap_or_else(|| id.clone());
    let mut notes = Vec::new();

    if sampler.not_objects > 0 {
        notes.push(Note {
            path: "models".into(),
            message: format!(
                "{} line(s) were not JSON objects and were skipped",
                sampler.not_objects
            ),
            confidence: Confidence::Confirm,
        });
    }

    let mut fields: IndexMap<String, Field> = IndexMap::new();
    for (name, obs) in &sampler.fields {
        fields.insert(
            name.clone(),
            obs.finish(name, &model_name, sampler.records, opts, &mut notes),
        );
    }

    // `strict` is the single highest-value default in a data contract (it is
    // what catches a producer silently adding a field), so draft it on — but
    // only claim the sample supports it when the shape never moved.
    let strict = !sampler.shapes_overflow && sampler.shapes.len() <= 1;
    if !strict {
        let shapes = if sampler.shapes_overflow {
            "many".to_string()
        } else {
            sampler.shapes.len().to_string()
        };
        notes.push(Note {
            path: format!("models.{model_name}.strict"),
            message: format!(
                "the {} records carried {shapes} different field sets (optional fields come \
                 and go) — drafted `strict: false`; turn it on once the shape is settled, it \
                 is what catches silent additions",
                sampler.records,
            ),
            confidence: Confidence::Confirm,
        });
    }

    let mut models = IndexMap::new();
    models.insert(
        model_name.clone(),
        Model {
            description: None,
            strict,
            fields,
        },
    );

    let contract = Contract {
        covenant: 1,
        id: id.clone(),
        name: None,
        // 0.x says out loud that this is a draft nobody should pin to yet.
        version: "0.1.0".into(),
        owner: None,
        description: Some(format!(
            "DRAFT inferred from {} sampled record(s). Review every `confirm:` note below \
             before enforcing.",
            sampler.records
        )),
        models,
        policy: Policy::default(),
    };

    Ok(Draft {
        contract,
        notes,
        sampled: sampler.records,
        truncated,
        sources: paths.iter().map(|p| p.display().to_string()).collect(),
    })
}

/// A file stem turned into a legal contract id (the spec's id rules are
/// checked by `validate`; this keeps the default from tripping them).
fn stem_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("contract")
        .to_ascii_lowercase();
    let mut id: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if id.is_empty() || !id.starts_with(|c: char| c.is_ascii_alphabetic()) {
        id.insert_str(0, "c_");
    }
    id
}

/// Stream NDJSON records into the sampler, skipping (and counting) lines
/// that are not JSON objects — dirty input is the normal case here.
fn sample_ndjson(path: &Path, limit: u64, sampler: &mut Sampler) -> Result<()> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).map_err(|e| CovenantError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    for line in std::io::BufReader::new(file).lines() {
        if sampler.records >= limit {
            break;
        }
        let line = line.map_err(|e| CovenantError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        // A malformed line is data to learn nothing from, not a run failure
        // — inference over dirty input is the normal case.
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            sampler.not_objects += 1;
            continue;
        };
        let serde_json::Value::Object(map) = value else {
            sampler.not_objects += 1;
            continue;
        };
        let pairs: Vec<(String, Obs)> = map.iter().map(|(k, v)| (k.clone(), json_obs(v))).collect();
        sampler.record(pairs.iter().map(|(k, v)| (k.as_str(), *v)));
    }
    Ok(())
}

/// Map one JSON value onto the reader-agnostic observation type.
fn json_obs(v: &serde_json::Value) -> Obs<'_> {
    match v {
        serde_json::Value::Null => Obs::Null,
        serde_json::Value::Bool(_) => Obs::Bool,
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Obs::Int(i),
            None => Obs::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => Obs::Str(s),
        serde_json::Value::Array(_) => Obs::Nested("arrays"),
        serde_json::Value::Object(_) => Obs::Nested("objects"),
    }
}

/// Read CSV through arrow-csv's own type sniffing, then let every text
/// column fall through to string-shape analysis.
fn sample_csv(path: &Path, limit: u64, sampler: &mut Sampler) -> Result<()> {
    use arrow_csv::reader::Format;
    let io_err = |e: std::io::Error| CovenantError::Io {
        path: path.display().to_string(),
        source: e,
    };
    let data_err = |e: arrow_schema::ArrowError| CovenantError::DataRead {
        path: path.display().to_string(),
        message: e.to_string(),
    };
    let mut file = std::fs::File::open(path).map_err(io_err)?;
    let format = Format::default().with_header(true);
    // Unlike `check`, inference has no contract to read the schema from, so
    // arrow-csv's own sniffing is the starting point — and every Utf8 column
    // still goes through the string-shape analysis below, which is where
    // uuid/date/timestamp/enum/pattern actually come from.
    let (schema, _) = format
        .infer_schema(&mut file, Some(limit.min(1000) as usize))
        .map_err(data_err)?;
    let file = std::fs::File::open(path).map_err(io_err)?;
    let reader = arrow_csv::ReaderBuilder::new(std::sync::Arc::new(schema))
        .with_format(format)
        .with_batch_size(1024)
        .build(file)
        .map_err(data_err)?;
    for batch in reader {
        let batch = batch.map_err(data_err)?;
        sample_batch(&batch, limit, sampler);
        if sampler.records >= limit {
            break;
        }
    }
    Ok(())
}

/// Read Parquet, whose embedded schema is authoritative for column types.
fn sample_parquet(path: &Path, limit: u64, sampler: &mut Sampler) -> Result<()> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let file = std::fs::File::open(path).map_err(|e| CovenantError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let data_err = |m: String| CovenantError::DataRead {
        path: path.display().to_string(),
        message: m,
    };
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| data_err(e.to_string()))?;
    let reader = builder
        .with_batch_size(1024)
        .build()
        .map_err(|e| data_err(e.to_string()))?;
    for batch in reader {
        let batch = batch.map_err(|e| data_err(e.to_string()))?;
        sample_batch(&batch, limit, sampler);
        if sampler.records >= limit {
            break;
        }
    }
    Ok(())
}

/// Feed one Arrow batch through the same accumulator the JSON path uses.
fn sample_batch(batch: &arrow_array::RecordBatch, limit: u64, sampler: &mut Sampler) {
    use arrow_array::{cast::AsArray, types::*, Array};
    use arrow_schema::DataType;

    let names: Vec<&str> = batch
        .schema_ref()
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .collect();
    let rows = batch.num_rows();
    for row in 0..rows {
        if sampler.records >= limit {
            return;
        }
        // Values are read per cell; String/Timestamp/Date renderings match
        // what the contract's stringly types promise, so shape detection is
        // the same as the JSON path.
        //
        // Temporal cells render to owned Strings that the borrowed
        // `Obs::Str` must outlive, so they are rendered BEFORE the pass
        // below rather than deferred to the end of it — deferring reordered
        // them after every other column, and `Sampler::fields` is keyed by
        // first-seen order, so a Parquet file with `created_at` first
        // drafted a contract with `created_at` last.
        let rendered: Vec<Option<String>> = (0..names.len())
            .map(|col| {
                let array = batch.column(col);
                match (array.is_null(row), array.data_type()) {
                    (false, DataType::Date32 | DataType::Date64 | DataType::Timestamp(_, _)) => {
                        Some(render_temporal(array, row))
                    }
                    _ => None,
                }
            })
            .collect();
        let mut obs: Vec<(&str, Obs)> = Vec::with_capacity(names.len());
        for (col, name) in names.iter().enumerate() {
            let array = batch.column(col);
            if array.is_null(row) {
                obs.push((name, Obs::Null));
                continue;
            }
            if let Some(text) = &rendered[col] {
                obs.push((name, Obs::Str(text)));
                continue;
            }
            let o = match array.data_type() {
                DataType::Boolean => Obs::Bool,
                DataType::Int8 => Obs::Int(array.as_primitive::<Int8Type>().value(row) as i64),
                DataType::Int16 => Obs::Int(array.as_primitive::<Int16Type>().value(row) as i64),
                DataType::Int32 => Obs::Int(array.as_primitive::<Int32Type>().value(row) as i64),
                DataType::Int64 => Obs::Int(array.as_primitive::<Int64Type>().value(row)),
                DataType::UInt8 => Obs::Int(array.as_primitive::<UInt8Type>().value(row) as i64),
                DataType::UInt16 => Obs::Int(array.as_primitive::<UInt16Type>().value(row) as i64),
                DataType::UInt32 => Obs::Int(array.as_primitive::<UInt32Type>().value(row) as i64),
                DataType::UInt64 => {
                    Obs::Float(array.as_primitive::<UInt64Type>().value(row) as f64)
                }
                DataType::Float32 => {
                    Obs::Float(array.as_primitive::<Float32Type>().value(row) as f64)
                }
                DataType::Float64 => Obs::Float(array.as_primitive::<Float64Type>().value(row)),
                DataType::Utf8 => Obs::Str(array.as_string::<i32>().value(row)),
                DataType::LargeUtf8 => Obs::Str(array.as_string::<i64>().value(row)),
                other => Obs::Nested(arrow_kind(other)),
            };
            obs.push((name, o));
        }
        sampler.record(obs.into_iter());
    }
}

/// Name an Arrow type that has no contract equivalent, for the note that
/// tells the author what was flagged.
fn arrow_kind(dt: &arrow_schema::DataType) -> &'static str {
    use arrow_schema::DataType;
    match dt {
        DataType::List(_) | DataType::LargeList(_) | DataType::FixedSizeList(_, _) => "arrays",
        DataType::Struct(_) => "objects",
        DataType::Map(_, _) => "maps",
        _ => "values of a type with no contract equivalent",
    }
}

/// Render a temporal Arrow cell as the string the contract type promises.
fn render_temporal(array: &dyn arrow_array::Array, row: usize) -> String {
    use arrow_array::cast::AsArray;
    use arrow_schema::DataType;
    match array.data_type() {
        DataType::Date32 => arrow_array::temporal_conversions::date32_to_datetime(
            array
                .as_primitive::<arrow_array::types::Date32Type>()
                .value(row),
        )
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default(),
        DataType::Date64 => arrow_array::temporal_conversions::date64_to_datetime(
            array
                .as_primitive::<arrow_array::types::Date64Type>()
                .value(row),
        )
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default(),
        _ => {
            // Timestamps: any unit, rendered RFC 3339 with a Z offset.
            // CHECKED: inference runs on untrusted files, and a
            // second-unit timestamp past ~year 2262 overflows i64 on the way
            // to nanoseconds — that panics in debug and wraps to a nonsense
            // date in release, which would then train the wrong shape into
            // the draft. Overflow renders empty, like any unconvertible cell.
            let v = match array.data_type() {
                DataType::Timestamp(arrow_schema::TimeUnit::Second, _) => array
                    .as_primitive::<arrow_array::types::TimestampSecondType>()
                    .value(row)
                    .checked_mul(1_000_000_000),
                DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, _) => array
                    .as_primitive::<arrow_array::types::TimestampMillisecondType>()
                    .value(row)
                    .checked_mul(1_000_000),
                DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, _) => array
                    .as_primitive::<arrow_array::types::TimestampMicrosecondType>()
                    .value(row)
                    .checked_mul(1_000),
                _ => Some(
                    array
                        .as_primitive::<arrow_array::types::TimestampNanosecondType>()
                        .value(row),
                ),
            };
            v.and_then(arrow_array::temporal_conversions::timestamp_ns_to_datetime)
                .map(|d| {
                    d.and_utc()
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                })
                .unwrap_or_default()
        }
    }
}

impl Draft {
    /// Render the draft as a commented YAML contract: the document a human
    /// edits, with every uncertain call marked inline at the line it affects.
    /// Round-trips through [`Contract::parse`] by construction.
    pub fn to_yaml(&self) -> String {
        use std::fmt::Write;
        let c = &self.contract;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "# DRAFT contract inferred by `covenant infer` from {} sampled record(s){}.",
            self.sampled,
            if self.truncated {
                " (sample truncated)"
            } else {
                ""
            },
        );
        for source in &self.sources {
            let _ = writeln!(out, "#   source: {source}");
        }
        let _ = writeln!(
            out,
            "#\n\
             # A sample proves what appeared; it can never prove what will not.\n\
             # Every `confirm:` line below is a guess from this window — read it,\n\
             # then keep, widen, or delete the rule above it. Fill in `owner:` and\n\
             # bump the version to 1.0.0 when it is a promise you will keep.\n\
             # Next: `covenant validate <this file>`, then `covenant check <data> -c <this file>`.",
        );
        let _ = writeln!(out, "covenant: {}", c.covenant);
        let _ = writeln!(out, "id: {}", yaml_scalar(&c.id));
        let _ = writeln!(
            out,
            "version: {}   # 0.x: a draft, not yet a promise",
            c.version
        );
        let _ = writeln!(
            out,
            "# owner: data-platform@example.com   # TODO: who answers for this?"
        );
        if let Some(d) = &c.description {
            let _ = writeln!(out, "description: {}", yaml_scalar(d));
        }
        let _ = writeln!(out, "\nmodels:");
        for (model_name, model) in &c.models {
            let _ = writeln!(out, "  {}:", yaml_scalar(model_name));
            for note in self.notes_for(&format!("models.{model_name}.strict")) {
                let _ = writeln!(out, "    # confirm: {}", note.message);
            }
            let _ = writeln!(
                out,
                "    strict: {}          # undeclared fields are violations",
                model.strict
            );
            let _ = writeln!(out, "    fields:");
            for (field_name, field) in &model.fields {
                let base = format!("models.{model_name}.fields.{field_name}");
                for note in self.notes_for(&base) {
                    let _ = writeln!(out, "      {} {}", note.marker(), note.message);
                }
                let _ = writeln!(out, "      {}:", yaml_scalar(field_name));
                let _ = writeln!(out, "        type: {}", field.ty.name());
                if field.required {
                    let _ = writeln!(out, "        required: true");
                }
                if field.nullable {
                    let _ = writeln!(out, "        nullable: true");
                }
                if let Some(f) = field.format {
                    let _ = writeln!(out, "        format: {}", f.name());
                }
                self.write_rule(&mut out, &base, ".pattern", field.pattern.as_ref(), |p| {
                    format!("        pattern: {}", yaml_scalar(p))
                });
                if let (Some(lo), Some(hi)) = (field.min, field.max) {
                    for note in self.notes_for(&format!("{base}.min/max")) {
                        let _ = writeln!(out, "        {} {}", note.marker(), note.message);
                    }
                    let _ = writeln!(out, "        min: {}", render_number(lo));
                    let _ = writeln!(out, "        max: {}", render_number(hi));
                }
                if let (Some(lo), Some(hi)) = (field.min_length, field.max_length) {
                    for note in self.notes_for(&format!("{base}.min_length/max_length")) {
                        let _ = writeln!(out, "        {} {}", note.marker(), note.message);
                    }
                    let _ = writeln!(out, "        min_length: {lo}");
                    let _ = writeln!(out, "        max_length: {hi}");
                }
                for note in self.notes_for(&format!("{base}.max_length")) {
                    let _ = writeln!(out, "        {} {}", note.marker(), note.message);
                }
                if let Some(allowed) = &field.allowed {
                    for note in self.notes_for(&format!("{base}.allowed")) {
                        let _ = writeln!(out, "        {} {}", note.marker(), note.message);
                    }
                    let rendered: Vec<String> = allowed
                        .iter()
                        .map(|v| match v {
                            serde_json::Value::String(s) => yaml_scalar(s),
                            other => other.to_string(),
                        })
                        .collect();
                    let _ = writeln!(out, "        allowed: [{}]", rendered.join(", "));
                }
                for note in self.notes_for(&format!("{base}.unique")) {
                    let _ = writeln!(out, "        {} {}", note.marker(), note.message);
                }
            }
        }
        // Render the policy from the STRUCT, never from a hand-written
        // literal: a drifting enum name here would emit a draft that does
        // not parse (it did, once — hence this comment and the round-trip
        // test that caught it).
        let policy = serde_yaml::to_string(&c.policy).unwrap_or_default();
        let _ = writeln!(out, "\npolicy:");
        for line in policy.lines().filter(|l| !l.trim().is_empty()) {
            let annotation = match line.split(':').next().map(str::trim) {
                Some("on_violation") => {
                    "   # block = fail CI / withhold records; warn = report only"
                }
                Some("max_violations") => "  # budget for known dirt while a producer cleans up",
                Some("sample_violations") => " # example violations kept per (field, rule)",
                _ => "",
            };
            let _ = writeln!(out, "  {line}{annotation}");
        }
        out
    }

    /// Emit one optional rule line, preceded by any notes anchored at it.
    fn write_rule<T>(
        &self,
        out: &mut String,
        base: &str,
        suffix: &str,
        value: Option<&T>,
        render: impl Fn(&T) -> String,
    ) {
        use std::fmt::Write;
        let Some(v) = value else { return };
        for note in self.notes_for(&format!("{base}{suffix}")) {
            let _ = writeln!(out, "        {} {}", note.marker(), note.message);
        }
        let _ = writeln!(out, "{}", render(v));
    }

    /// Every note anchored at exactly this spec path.
    fn notes_for<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a Note> + 'a {
        self.notes.iter().filter(move |n| n.path == path)
    }

    /// The machine-readable form: the contract plus the notes, for tooling
    /// (and the console) that wants the uncertainty as data.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "contract": self.contract,
            "sampled": self.sampled,
            "truncated": self.truncated,
            "sources": self.sources,
            "notes": self.notes.iter().map(|n| serde_json::json!({
                "path": n.path,
                "message": n.message,
                "confidence": match n.confidence {
                    Confidence::Observed => "observed",
                    Confidence::Confirm => "confirm",
                },
            })).collect::<Vec<_>>(),
        })
    }
}

impl Note {
    /// The YAML comment prefix that shows how much weight this note carries.
    fn marker(&self) -> &'static str {
        match self.confidence {
            Confidence::Observed => "# observed:",
            Confidence::Confirm => "# confirm:",
        }
    }
}

/// Quote a YAML scalar unless it is unambiguously plain. Values here come
/// from DATA (field names, enum members), so this must never emit something
/// that reparses as a different value.
fn yaml_scalar(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !s.starts_with(['-', '.'])
        && s.parse::<f64>().is_err()
        // serde_yaml resolves 0x/0o/0b scalars as INTEGERS, so a string
        // value like "0x2A" would come back as 42 and never match the data
        // it was drafted from. `f64::parse` does not catch these.
        && !is_yaml_radix_int(s)
        && !matches!(
            s.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n" | "~"
        );
    if plain {
        s.to_string()
    } else {
        let escaped = s
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t");
        format!("\"{escaped}\"")
    }
}

/// Does YAML resolve this plain scalar as a radix integer (`0x2A`, `0o17`,
/// `0b101`)? Only well-formed digits count — `0xZZ` stays a string, and so
/// do underscore-separated decimals like `1_000`, which YAML 1.2 does not
/// treat as a number.
fn is_yaml_radix_int(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let Some((prefix, digits)) = body.get(..2).zip(body.get(2..)) else {
        return false;
    };
    let radix = match prefix {
        "0x" => 16,
        "0o" => 8,
        "0b" => 2,
        _ => return false,
    };
    !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix))
}
