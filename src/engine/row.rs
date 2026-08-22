//! Record-at-a-time validation over `serde_json` values — the engine behind
//! NDJSON checks and the stream gate. Designed for the hot path: one pass per
//! record, and violation strings are built only when a rule actually fails.
//! The one clean-path allocation is the canonical key for `unique` fields
//! (first sighting of each value); models without `unique` allocate nothing
//! on clean records.

use serde_json::Value;

use crate::compile::{shape, CompiledField, CompiledModel};
use crate::report::{preview, truncate, Rule, Violation};
use crate::spec::FieldType;

use super::UniqueTracker;

/// Validate one record. Appends violations to `out` and returns true when the
/// record is clean. `row` is the 0-based record index used in messages.
pub fn validate_record(
    model: &CompiledModel,
    record: &Value,
    row: u64,
    unique: Option<&mut UniqueTracker>,
    out: &mut Vec<Violation>,
) -> bool {
    let before = out.len();
    let Some(obj) = record.as_object() else {
        out.push(Violation {
            model: model.name.clone(),
            field: None,
            rule: Rule::RecordNotObject,
            row: Some(row),
            value: Some(preview(record)),
            message: format!("row {row}: record is not a JSON object"),
        });
        return false;
    };

    // Field-level checks in contract order.
    let mut unique = unique;
    for (idx, field) in model.fields.iter().enumerate() {
        match obj.get(&field.name) {
            None => {
                if field.required {
                    out.push(violation(
                        model,
                        field,
                        Rule::RequiredMissing,
                        row,
                        None,
                        format!("row {row}: required field {:?} is missing", field.name),
                    ));
                }
            }
            Some(Value::Null) => {
                if !field.nullable {
                    out.push(violation(
                        model,
                        field,
                        Rule::NullNotAllowed,
                        row,
                        None,
                        format!("row {row}: field {:?} is null but the contract forbids null", field.name),
                    ));
                }
            }
            Some(value) => {
                let type_ok = check_value(model, field, value, row, out);
                // Uniqueness applies to type-valid values only — a type
                // mismatch is already reported and its rendering would
                // collide across representations.
                if type_ok {
                    if let Some(tracker) = unique.as_deref_mut() {
                        if tracker.tracks(idx) {
                            let key = canonical_key(field.ty, value);
                            if !tracker.insert(idx, &key) {
                                out.push(violation(
                                    model,
                                    field,
                                    Rule::Unique,
                                    row,
                                    Some(preview(value)),
                                    format!(
                                        "row {row}: field {:?} value {} was already seen (unique)",
                                        field.name,
                                        preview(value)
                                    ),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    // Closed-world check.
    if model.strict {
        for key in obj.keys() {
            if !model.field_index.contains_key(key) {
                out.push(Violation {
                    model: model.name.clone(),
                    field: Some(key.clone()),
                    rule: Rule::UnexpectedField,
                    row: Some(row),
                    value: None,
                    message: format!("row {row}: field {key:?} is not declared in the contract (strict model)"),
                });
            }
        }
    }

    out.len() == before
}

/// Type + constraint checks for a present, non-null value. Returns true when
/// the value matched the contract type (constraints may still have failed).
fn check_value(
    model: &CompiledModel,
    field: &CompiledField,
    value: &Value,
    row: u64,
    out: &mut Vec<Violation>,
) -> bool {
    match field.ty {
        FieldType::String => match value.as_str() {
            Some(s) => {
                check_string_constraints(model, field, s, row, out);
                true
            }
            None => {
                push_type_mismatch(model, field, value, row, out);
                false
            }
        },
        FieldType::Integer => {
            // JSON has one number type; an integer field accepts only values
            // with an exact integer representation (3 yes, 3.5 no, 3.0 no —
            // serde_json parses 3.0 as f64). Bounds and allowed-set checks
            // run in the integer domain (i128) so u64 values above i64::MAX
            // don't wrap and bounds beyond 2^53 don't lose precision.
            let int_val: Option<i128> = value
                .as_i64()
                .map(|n| n as i128)
                .or_else(|| value.as_u64().map(|n| n as i128));
            match int_val {
                Some(n) => {
                    check_integer_constraints(model, field, n, value, row, out);
                    if let Some(allowed) = &field.allowed {
                        if !allowed.contains_integer(n) {
                            push_allowed(model, field, value, row, out);
                        }
                    }
                    true
                }
                None => {
                    push_type_mismatch(model, field, value, row, out);
                    false
                }
            }
        }
        FieldType::Float => match value.as_f64() {
            // as_f64 succeeds for integer JSON numbers too — int → float widening is fine.
            Some(n) => {
                check_numeric_constraints(model, field, n, value, row, out);
                if let Some(allowed) = &field.allowed {
                    if !allowed.contains_f64(n) {
                        push_allowed(model, field, value, row, out);
                    }
                }
                true
            }
            None => {
                push_type_mismatch(model, field, value, row, out);
                false
            }
        },
        FieldType::Boolean => match value.as_bool() {
            Some(b) => {
                if let Some(allowed) = &field.allowed {
                    if !allowed.contains_bool(b) {
                        push_allowed(model, field, value, row, out);
                    }
                }
                true
            }
            None => {
                push_type_mismatch(model, field, value, row, out);
                false
            }
        },
        FieldType::Timestamp => check_stringly(model, field, value, row, out, shape::is_timestamp, "an RFC 3339 timestamp"),
        FieldType::Date => check_stringly(model, field, value, row, out, shape::is_date, "a YYYY-MM-DD date"),
        FieldType::Uuid => check_stringly(model, field, value, row, out, shape::is_uuid, "a canonical UUID"),
    }
}

/// Timestamp/date/uuid: a string with extra shape; `allowed` compares the raw string.
fn check_stringly(
    model: &CompiledModel,
    field: &CompiledField,
    value: &Value,
    row: u64,
    out: &mut Vec<Violation>,
    is_valid: fn(&str) -> bool,
    expected: &str,
) -> bool {
    match value.as_str() {
        Some(s) if is_valid(s) => {
            if let Some(allowed) = &field.allowed {
                if !allowed.contains_str(s) {
                    push_allowed(model, field, value, row, out);
                }
            }
            true
        }
        Some(s) => {
            out.push(violation(
                model,
                field,
                Rule::TypeMismatch,
                row,
                Some(truncate(s, 64)),
                format!(
                    "row {row}: field {:?} value {} is not {expected}",
                    field.name,
                    preview(value)
                ),
            ));
            false
        }
        None => {
            push_type_mismatch(model, field, value, row, out);
            false
        }
    }
}

fn check_string_constraints(
    model: &CompiledModel,
    field: &CompiledField,
    s: &str,
    row: u64,
    out: &mut Vec<Violation>,
) {
    let len = s.chars().count();
    if let Some(lo) = field.min_length {
        if len < lo {
            out.push(violation(
                model,
                field,
                Rule::MinLength,
                row,
                Some(truncate(s, 64)),
                format!("row {row}: field {:?} length {len} is below min_length {lo}", field.name),
            ));
        }
    }
    if let Some(hi) = field.max_length {
        if len > hi {
            out.push(violation(
                model,
                field,
                Rule::MaxLength,
                row,
                Some(truncate(s, 64)),
                format!("row {row}: field {:?} length {len} exceeds max_length {hi}", field.name),
            ));
        }
    }
    if let Some(re) = &field.pattern {
        if !re.is_match(s) {
            out.push(violation(
                model,
                field,
                Rule::Pattern,
                row,
                Some(truncate(s, 64)),
                format!(
                    "row {row}: field {:?} value {:?} does not match pattern {:?}",
                    field.name,
                    truncate(s, 64),
                    field.pattern_src.as_deref().unwrap_or("")
                ),
            ));
        }
    }
    if let Some(fmt) = field.format {
        if !shape::matches_format(fmt, s) {
            out.push(violation(
                model,
                field,
                Rule::Format,
                row,
                Some(truncate(s, 64)),
                format!(
                    "row {row}: field {:?} value {:?} is not a valid {}",
                    field.name,
                    truncate(s, 64),
                    fmt.name()
                ),
            ));
        }
    }
    if let Some(allowed) = &field.allowed {
        if !allowed.contains_str(s) {
            out.push(violation(
                model,
                field,
                Rule::Allowed,
                row,
                Some(truncate(s, 64)),
                format!(
                    "row {row}: field {:?} value {:?} not in allowed set [{}]",
                    field.name,
                    truncate(s, 64),
                    allowed.describe()
                ),
            ));
        }
    }
}

/// Bounds for integer values, compared in the integer domain.
fn check_integer_constraints(
    model: &CompiledModel,
    field: &CompiledField,
    n: i128,
    value: &Value,
    row: u64,
    out: &mut Vec<Violation>,
) {
    if let Some(min) = field.min {
        if shape::int_below_min(n, min) {
            out.push(violation(
                model,
                field,
                Rule::Min,
                row,
                Some(preview(value)),
                format!("row {row}: field {:?} value {n} is below min {min}", field.name),
            ));
        }
    }
    if let Some(max) = field.max {
        if shape::int_above_max(n, max) {
            out.push(violation(
                model,
                field,
                Rule::Max,
                row,
                Some(preview(value)),
                format!("row {row}: field {:?} value {n} exceeds max {max}", field.name),
            ));
        }
    }
}

fn check_numeric_constraints(
    model: &CompiledModel,
    field: &CompiledField,
    n: f64,
    value: &Value,
    row: u64,
    out: &mut Vec<Violation>,
) {
    if let Some(min) = field.min {
        if n < min {
            out.push(violation(
                model,
                field,
                Rule::Min,
                row,
                Some(preview(value)),
                format!("row {row}: field {:?} value {n} is below min {min}", field.name),
            ));
        }
    }
    if let Some(max) = field.max {
        if n > max {
            out.push(violation(
                model,
                field,
                Rule::Max,
                row,
                Some(preview(value)),
                format!("row {row}: field {:?} value {n} exceeds max {max}", field.name),
            ));
        }
    }
}

fn push_type_mismatch(
    model: &CompiledModel,
    field: &CompiledField,
    value: &Value,
    row: u64,
    out: &mut Vec<Violation>,
) {
    out.push(violation(
        model,
        field,
        Rule::TypeMismatch,
        row,
        Some(preview(value)),
        format!(
            "row {row}: field {:?} expected {} but got {}",
            field.name,
            field.ty.name(),
            json_type_name(value)
        ),
    ));
}

fn push_allowed(
    model: &CompiledModel,
    field: &CompiledField,
    value: &Value,
    row: u64,
    out: &mut Vec<Violation>,
) {
    let allowed = field.allowed.as_ref().expect("caller checked");
    out.push(violation(
        model,
        field,
        Rule::Allowed,
        row,
        Some(preview(value)),
        format!(
            "row {row}: field {:?} value {} not in allowed set [{}]",
            field.name,
            preview(value),
            allowed.describe()
        ),
    ));
}

fn violation(
    model: &CompiledModel,
    field: &CompiledField,
    rule: Rule,
    row: u64,
    value: Option<String>,
    message: String,
) -> Violation {
    Violation {
        model: model.name.clone(),
        field: Some(field.name.clone()),
        rule,
        row: Some(row),
        value,
        message,
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Canonical rendering used as the uniqueness key, normalized per the
/// *contract* type: a float field keys by numeric value, so JSON `1` and
/// `1.0` (both valid floats, equal values) collide as duplicates instead of
/// slipping past uniqueness on their distinct source renderings.
fn canonical_key(ty: FieldType, v: &Value) -> String {
    match (ty, v) {
        (_, Value::String(s)) => s.clone(),
        (FieldType::Float, _) => v
            .as_f64()
            // `-0.0 == 0.0` but they render "-0"/"0" — one value, one key.
            .map(|f| if f == 0.0 { "0".to_string() } else { f.to_string() })
            .unwrap_or_else(|| v.to_string()),
        (FieldType::Integer, _) => v
            .as_i64()
            .map(|i| i.to_string())
            .or_else(|| v.as_u64().map(|u| u.to_string()))
            .unwrap_or_else(|| v.to_string()),
        _ => v.to_string(),
    }
}
