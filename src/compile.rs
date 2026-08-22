//! Compile a parsed [`Contract`](crate::spec::Contract) into the artifact the
//! hot path runs: regexes pre-built, allowed-sets hashed, per-field checks
//! resolved once. Enforcement points (`check`, `gate`, the Arrow validator)
//! only ever see a [`CompiledContract`] — nothing re-parses YAML or
//! re-compiles a regex per record.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use regex::Regex;

use crate::error::{CovenantError, Result};
use crate::spec::{Contract, Field, FieldType, LintLevel, StringFormat};

#[derive(Debug)]
/// A contract compiled for the hot path: regexes built, allowed-sets
/// hashed, models indexed. The only form the engines accept.
pub struct CompiledContract {
    pub id: String,
    pub version: String,
    pub owner: Option<String>,
    pub policy: crate::spec::Policy,
    pub models: IndexMap<String, CompiledModel>,
}

#[derive(Debug)]
/// One model's compiled field set, with an O(1) name index for strict-mode
/// membership checks.
pub struct CompiledModel {
    pub name: String,
    pub strict: bool,
    pub fields: Vec<CompiledField>,
    /// field name → index into `fields`, for O(1) membership in strict mode.
    pub field_index: HashMap<String, usize>,
}

#[derive(Debug)]
/// A field's promise in enforcement-ready form (compiled regex, hashed
/// allowed set, resolved bounds).
pub struct CompiledField {
    pub name: String,
    pub ty: FieldType,
    pub required: bool,
    pub nullable: bool,
    pub unique: bool,
    pub pattern: Option<Regex>,
    /// Kept alongside the compiled regex so messages can quote the source.
    pub pattern_src: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub allowed: Option<CompiledAllowed>,
    pub format: Option<StringFormat>,
}

/// Allowed-set with the common all-strings case hashed for O(1) lookup.
#[derive(Debug)]
pub enum CompiledAllowed {
    Strings(HashSet<String>),
    /// Mixed/numeric sets stay a small vec — enum sets are tens of values,
    /// linear scan is cheaper than a polymorphic hash.
    Values(Vec<serde_json::Value>),
}

impl CompiledAllowed {
    /// String membership (O(1) for all-string sets).
    pub fn contains_str(&self, s: &str) -> bool {
        match self {
            CompiledAllowed::Strings(set) => set.contains(s),
            CompiledAllowed::Values(vals) => vals.iter().any(|v| v.as_str() == Some(s)),
        }
    }

    /// Integer membership over the full i128 range, so u64 values above
    /// `i64::MAX` compare correctly instead of wrapping (lint admits both
    /// i64 and u64 allowed values, so enforcement must match both).
    pub fn contains_integer(&self, n: i128) -> bool {
        match self {
            CompiledAllowed::Strings(_) => false,
            CompiledAllowed::Values(vals) => vals.iter().any(|v| {
                v.as_i64().map(|x| x as i128 == n).unwrap_or(false)
                    || v.as_u64().map(|x| x as i128 == n).unwrap_or(false)
            }),
        }
    }

    /// Float membership by exact f64 equality.
    pub fn contains_f64(&self, n: f64) -> bool {
        match self {
            CompiledAllowed::Strings(_) => false,
            CompiledAllowed::Values(vals) => vals.iter().any(|v| v.as_f64() == Some(n)),
        }
    }

    /// Float32 membership compared in f32 precision — an f32 column value
    /// widened to f64 carries rounding error that exact f64 equality would
    /// never match (0.1f32 != 0.1f64).
    pub fn contains_f32(&self, n: f32) -> bool {
        match self {
            CompiledAllowed::Strings(_) => false,
            CompiledAllowed::Values(vals) => vals
                .iter()
                .any(|v| v.as_f64().map(|x| x as f32 == n).unwrap_or(false)),
        }
    }

    /// Boolean membership.
    pub fn contains_bool(&self, b: bool) -> bool {
        match self {
            CompiledAllowed::Strings(_) => false,
            CompiledAllowed::Values(vals) => vals.iter().any(|v| v.as_bool() == Some(b)),
        }
    }

    /// Render the set for violation messages.
    pub fn describe(&self) -> String {
        match self {
            CompiledAllowed::Strings(set) => {
                let mut items: Vec<&str> = set.iter().map(String::as_str).collect();
                items.sort_unstable();
                items.join(", ")
            }
            CompiledAllowed::Values(vals) => vals
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

impl CompiledContract {
    /// Lints first and refuses to compile a contract with error-level
    /// findings — enforcement against a half-understood contract reports
    /// nonsense with authority, the worst possible failure mode.
    pub fn compile(contract: &Contract) -> Result<CompiledContract> {
        let errors: Vec<String> = contract
            .lint()
            .into_iter()
            .filter(|f| f.level == LintLevel::Error)
            .map(|f| format!("{}: {}", f.path, f.message))
            .collect();
        if !errors.is_empty() {
            return Err(CovenantError::ContractInvalid {
                id: contract.id.clone(),
                details: errors.join("; "),
            });
        }

        let mut models = IndexMap::new();
        for (model_name, model) in &contract.models {
            let mut fields = Vec::with_capacity(model.fields.len());
            let mut field_index = HashMap::with_capacity(model.fields.len());
            for (field_name, field) in &model.fields {
                field_index.insert(field_name.clone(), fields.len());
                fields.push(compile_field(field_name, field));
            }
            models.insert(
                model_name.clone(),
                CompiledModel {
                    name: model_name.clone(),
                    strict: model.strict,
                    fields,
                    field_index,
                },
            );
        }

        Ok(CompiledContract {
            id: contract.id.clone(),
            version: contract.version.clone(),
            owner: contract.owner.clone(),
            policy: contract.policy.clone(),
            models,
        })
    }

    /// Resolve which model to enforce: an explicit name, or the only model
    /// when the contract has exactly one.
    pub fn resolve_model(&self, requested: Option<&str>) -> Result<&CompiledModel> {
        let available = || self.models.keys().cloned().collect::<Vec<_>>().join(", ");
        match requested {
            Some(name) => self
                .models
                .get(name)
                .ok_or_else(|| CovenantError::ModelNotFound {
                    contract_id: self.id.clone(),
                    model: name.to_string(),
                    available: available(),
                }),
            None if self.models.len() == 1 => Ok(self.models.values().next().unwrap()),
            None => Err(CovenantError::ModelAmbiguous {
                contract_id: self.id.clone(),
                count: self.models.len(),
                available: available(),
            }),
        }
    }
}

fn compile_field(name: &str, field: &Field) -> CompiledField {
    // Lint already proved the pattern compiles; expect() documents the invariant.
    let pattern = field
        .pattern
        .as_deref()
        .map(|p| Regex::new(p).expect("lint guarantees pattern compiles"));

    let allowed = field.allowed.as_ref().map(|values| {
        if values.iter().all(|v| v.is_string()) {
            CompiledAllowed::Strings(
                values
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
            )
        } else {
            CompiledAllowed::Values(values.clone())
        }
    });

    CompiledField {
        name: name.to_string(),
        ty: field.ty,
        required: field.required,
        nullable: field.nullable,
        unique: field.unique,
        pattern,
        pattern_src: field.pattern.clone(),
        min: field.min,
        max: field.max,
        min_length: field.min_length,
        max_length: field.max_length,
        allowed,
        format: field.format,
    }
}

/// Value-shape helpers shared by the row and Arrow engines.
pub mod shape {
    use super::StringFormat;

    /// RFC 3339 timestamp (`2026-08-11T09:30:00Z`, offsets allowed).
    pub fn is_timestamp(s: &str) -> bool {
        chrono::DateTime::parse_from_rfc3339(s).is_ok()
    }

    /// Canonical `YYYY-MM-DD` calendar date. Shape is checked byte-wise
    /// first because chrono's numeric specifiers are lenient (1-digit
    /// months, signed years, leading whitespace) and a *contract* wants
    /// exactly one accepted rendering.
    pub fn is_date(s: &str) -> bool {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return false;
        }
        let digits_ok = b
            .iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7) || c.is_ascii_digit());
        digits_ok && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
    }

    /// Integer-domain bound checks: comparing an i64/u64 after an `as f64`
    /// cast loses precision beyond 2^53 and lets values slip past a bound
    /// they exceed, so integer values compare against the integerized bound.
    /// (Float casts saturate in Rust, so huge f64 bounds stay correct.)
    pub fn int_below_min(n: i128, min: f64) -> bool {
        n < min.ceil() as i128
    }

    /// See [`int_below_min`]: integer-domain upper-bound check.
    pub fn int_above_max(n: i128, max: f64) -> bool {
        n > max.floor() as i128
    }

    /// Canonical 8-4-4-4-12 hex UUID (case-insensitive).
    pub fn is_uuid(s: &str) -> bool {
        let bytes = s.as_bytes();
        if bytes.len() != 36 {
            return false;
        }
        for (i, b) in bytes.iter().enumerate() {
            match i {
                8 | 13 | 18 | 23 => {
                    if *b != b'-' {
                        return false;
                    }
                }
                _ => {
                    if !b.is_ascii_hexdigit() {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Deliberately loose format checks: the goal is catching wrong-field
    /// data (an ID in the email column), not RFC-grade validation.
    pub fn matches_format(fmt: StringFormat, s: &str) -> bool {
        match fmt {
            StringFormat::Email => {
                let Some(at) = s.find('@') else { return false };
                let (local, domain) = s.split_at(at);
                let domain = &domain[1..];
                !local.is_empty()
                    && !domain.is_empty()
                    && domain.contains('.')
                    && !domain.starts_with('.')
                    && !domain.ends_with('.')
                    && !s.contains(char::is_whitespace)
                    && s.matches('@').count() == 1
            }
            StringFormat::Uri => {
                // scheme ":" — RFC 3986 scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
                let Some(colon) = s.find(':') else {
                    return false;
                };
                let scheme = &s[..colon];
                let mut chars = scheme.chars();
                match chars.next() {
                    Some(c) if c.is_ascii_alphabetic() => {}
                    _ => return false,
                }
                chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                    && s.len() > colon + 1
                    && !s.contains(char::is_whitespace)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shape;
    use crate::spec::StringFormat;

    #[test]
    fn timestamp_shapes() {
        assert!(shape::is_timestamp("2026-08-11T09:30:00Z"));
        assert!(shape::is_timestamp("2026-08-11T09:30:00.123+02:00"));
        assert!(!shape::is_timestamp("2026-08-11 09:30:00"));
        assert!(!shape::is_timestamp("2026-08-11"));
        assert!(!shape::is_timestamp(""));
    }

    #[test]
    fn date_shapes() {
        assert!(shape::is_date("2026-08-11"));
        assert!(!shape::is_date("2026-13-01"));
        assert!(!shape::is_date("11/08/2026"));
        assert!(!shape::is_date("2026-08-11T00:00:00Z"));
        // Non-canonical renderings chrono would tolerate are contract errors.
        assert!(!shape::is_date("2026-8-1"));
        assert!(!shape::is_date(" 2026-08-11"));
        assert!(!shape::is_date("+2026-08-11"));
    }

    #[test]
    fn uuid_shapes() {
        assert!(shape::is_uuid("6f1e0d3a-8c2b-4a5d-9e7f-0123456789ab"));
        assert!(shape::is_uuid("6F1E0D3A-8C2B-4A5D-9E7F-0123456789AB"));
        assert!(!shape::is_uuid("6f1e0d3a8c2b4a5d9e7f0123456789ab"));
        assert!(!shape::is_uuid("6f1e0d3a-8c2b-4a5d-9e7f-0123456789a"));
        assert!(!shape::is_uuid("6f1e0d3a-8c2b-4a5d-9e7f-0123456789ag"));
    }

    #[test]
    fn email_shapes() {
        assert!(shape::matches_format(StringFormat::Email, "a@b.co"));
        assert!(shape::matches_format(
            StringFormat::Email,
            "first.last+tag@sub.domain.io"
        ));
        assert!(!shape::matches_format(StringFormat::Email, "no-at-sign"));
        assert!(!shape::matches_format(StringFormat::Email, "@domain.co"));
        assert!(!shape::matches_format(StringFormat::Email, "a@nodot"));
        assert!(!shape::matches_format(
            StringFormat::Email,
            "a@.leading.dot"
        ));
        assert!(!shape::matches_format(StringFormat::Email, "two@@b.co"));
        assert!(!shape::matches_format(StringFormat::Email, "sp ace@b.co"));
    }

    #[test]
    fn uri_shapes() {
        assert!(shape::matches_format(
            StringFormat::Uri,
            "https://acme.io/x?y=1"
        ));
        assert!(shape::matches_format(StringFormat::Uri, "s3://bucket/key"));
        assert!(shape::matches_format(
            StringFormat::Uri,
            "urn:isbn:0451450523"
        ));
        assert!(!shape::matches_format(StringFormat::Uri, "no colon"));
        assert!(!shape::matches_format(StringFormat::Uri, "1http://x"));
        assert!(!shape::matches_format(StringFormat::Uri, "http:"));
        assert!(!shape::matches_format(
            StringFormat::Uri,
            "http://with space"
        ));
    }
}
