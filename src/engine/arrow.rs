//! Columnar validation over Arrow `RecordBatch`es — the enforcement point for
//! Parquet/CSV CI checks and for embedding Covenant inside Arrow-native
//! pipelines (`covenant::engine::arrow::validate_batch` is the library API).
//!
//! Columnar null semantics (documented in ARCHITECTURE.md): a column has no
//! way to distinguish "key absent" from "explicit null", so nulls in an
//! optional (`required: false`) field's column are treated as absent and
//! pass; nulls in a `required` field's column violate unless `nullable`.
//! A `required` field whose column is missing entirely is a schema-level
//! violation; an optional field's missing column is fine.
//!
//! Known representation limits (honest failures, never silent skips):
//! dictionary-encoded, decimal, and Float16 columns are reported as
//! `schema_type_mismatch` — the engine refuses to half-check a column it
//! cannot iterate. Dictionary support is not implemented yet.

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Date64Type, Float32Type, Float64Type, Int16Type, Int32Type, Int64Type, Int8Type,
    TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
    TimestampSecondType, UInt16Type, UInt32Type, UInt64Type, UInt8Type,
};
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, TimeUnit};

use crate::compile::{shape, CompiledField, CompiledModel};
use crate::report::{truncate, Collector, Rule, Violation};
use crate::spec::FieldType;

use super::UniqueTracker;

/// Validate one batch. `base_row` offsets row numbers so multi-batch files
/// report absolute positions. Returns the number of rows seen.
pub fn validate_batch(
    model: &CompiledModel,
    batch: &RecordBatch,
    base_row: u64,
    mut unique: Option<&mut UniqueTracker>,
    out: &mut Collector,
) -> u64 {
    let schema = batch.schema();

    // Schema pass: declared fields present and representable.
    for (idx, field) in model.fields.iter().enumerate() {
        let Some((col_idx, _)) = schema.column_with_name(&field.name) else {
            if field.required {
                out.record(Some(&field.name), Rule::SchemaMissingField, || Violation {
                    model: model.name.clone(),
                    field: Some(field.name.clone()),
                    rule: Rule::SchemaMissingField,
                    row: None,
                    value: None,
                    message: format!(
                        "required field {:?} is missing from the batch schema",
                        field.name
                    ),
                });
            }
            continue;
        };
        let column = batch.column(col_idx);
        if !type_compatible(field.ty, column.data_type()) {
            out.record(Some(&field.name), Rule::SchemaTypeMismatch, || Violation {
                model: model.name.clone(),
                field: Some(field.name.clone()),
                rule: Rule::SchemaTypeMismatch,
                row: None,
                value: Some(column.data_type().to_string()),
                message: format!(
                    "field {:?} declared {} but the column is {} (unsupported or incompatible)",
                    field.name,
                    field.ty.name(),
                    column.data_type()
                ),
            });
            continue;
        }
        check_column(
            model,
            field,
            idx,
            column.as_ref(),
            base_row,
            unique.as_deref_mut(),
            out,
        );
    }

    // Closed-world pass.
    if model.strict {
        for schema_field in schema.fields() {
            if !model.field_index.contains_key(schema_field.name()) {
                let name = schema_field.name().clone();
                out.record(Some(&name), Rule::UnexpectedField, || Violation {
                    model: model.name.clone(),
                    field: Some(name.clone()),
                    rule: Rule::UnexpectedField,
                    row: None,
                    value: None,
                    message: format!(
                        "column {name:?} is not declared in the contract (strict model)"
                    ),
                });
            }
        }
    }

    batch.num_rows() as u64
}

/// Contract type ↔ Arrow column type. Deliberately explicit (no `is_integer`
/// umbrella) so every accepted representation has a matching iteration arm
/// below — compat and iteration must never disagree.
fn type_compatible(ty: FieldType, dt: &DataType) -> bool {
    let stringly = matches!(dt, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View);
    let integer = matches!(
        dt,
        DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
    );
    match ty {
        FieldType::String => stringly,
        FieldType::Integer => integer,
        FieldType::Float => integer || matches!(dt, DataType::Float32 | DataType::Float64),
        FieldType::Boolean => matches!(dt, DataType::Boolean),
        // Native timestamp columns are type-guaranteed; string columns are
        // accepted and validated per value.
        FieldType::Timestamp => stringly || matches!(dt, DataType::Timestamp(_, _)),
        FieldType::Date => stringly || matches!(dt, DataType::Date32 | DataType::Date64),
        FieldType::Uuid => stringly || matches!(dt, DataType::FixedSizeBinary(16)),
    }
}

/// Null-handling shared by every column check: returns true when the row's
/// value must be checked, false when the row is exempt (valid null / treated
/// as absent), and records a violation when the null is illegal.
fn null_gate(
    model: &CompiledModel,
    field: &CompiledField,
    is_null: bool,
    row: u64,
    out: &mut Collector,
) -> bool {
    if !is_null {
        return true;
    }
    if field.required && !field.nullable {
        out.record(Some(&field.name), Rule::NullNotAllowed, || Violation {
            model: model.name.clone(),
            field: Some(field.name.clone()),
            rule: Rule::NullNotAllowed,
            row: Some(row),
            value: None,
            message: format!(
                "row {row}: field {:?} is null but the contract forbids null",
                field.name
            ),
        });
    }
    false
}

/// Per-row scalar extracted from a numeric/temporal column, carrying enough
/// type information to compare in the right domain (integers in i128 so
/// nothing wraps or loses precision, f32 in f32 so widening rounding error
/// doesn't fabricate violations).
enum NumVal {
    Int(i128),
    F64(f64),
    F32(f32),
    /// Temporal/binary value: type-guaranteed by the schema, no comparable
    /// scalar for value constraints (those are refused at column level).
    Opaque,
}

fn check_column(
    model: &CompiledModel,
    field: &CompiledField,
    field_idx: usize,
    column: &dyn Array,
    base_row: u64,
    mut unique: Option<&mut UniqueTracker>,
    out: &mut Collector,
) {
    // `allowed` has no evaluable rendering on native temporal / uuid-binary
    // columns. Refuse loudly, once per column, instead of silently passing
    // every value — a gate that half-checks certifies bad data. (min/max,
    // pattern, and length on these contract types are already lint errors.)
    if field.allowed.is_some()
        && matches!(
            column.data_type(),
            DataType::Timestamp(_, _)
                | DataType::Date32
                | DataType::Date64
                | DataType::FixedSizeBinary(16)
        )
    {
        out.record(Some(&field.name), Rule::SchemaTypeMismatch, || Violation {
            model: model.name.clone(),
            field: Some(field.name.clone()),
            rule: Rule::SchemaTypeMismatch,
            row: None,
            value: Some(column.data_type().to_string()),
            message: format!(
                "field {:?} declares `allowed` but the column is native {} — the constraint cannot \
                 be evaluated; encode the column as strings or drop the constraint",
                field.name,
                column.data_type()
            ),
        });
    }

    let track_unique =
        field.unique && unique.as_ref().map(|t| t.tracks(field_idx)).unwrap_or(false);

    // Fast path: nothing to look at per-row for this column.
    let needs_value_pass = track_unique
        || field.pattern.is_some()
        || field.allowed.is_some()
        || field.format.is_some()
        || field.min.is_some()
        || field.max.is_some()
        || field.min_length.is_some()
        || field.max_length.is_some()
        // timestamp/date/uuid carried in string columns need a per-value
        // shape pass; plain strings without constraints don't.
        || matches!(field.ty, FieldType::Timestamp | FieldType::Date | FieldType::Uuid)
            && is_string_column(column.data_type());
    if !needs_value_pass && (column.null_count() == 0 || field.nullable || !field.required) {
        return;
    }

    match column.data_type() {
        DataType::Utf8 => {
            let a = column.as_string::<i32>();
            for i in 0..a.len() {
                let row = base_row + i as u64;
                if !null_gate(model, field, a.is_null(i), row, out) {
                    continue;
                }
                check_str_value(model, field, field_idx, a.value(i), row, unique.as_deref_mut(), track_unique, out);
            }
        }
        DataType::LargeUtf8 => {
            let a = column.as_string::<i64>();
            for i in 0..a.len() {
                let row = base_row + i as u64;
                if !null_gate(model, field, a.is_null(i), row, out) {
                    continue;
                }
                check_str_value(model, field, field_idx, a.value(i), row, unique.as_deref_mut(), track_unique, out);
            }
        }
        DataType::Utf8View => {
            let a = column.as_string_view();
            for i in 0..a.len() {
                let row = base_row + i as u64;
                if !null_gate(model, field, a.is_null(i), row, out) {
                    continue;
                }
                check_str_value(model, field, field_idx, a.value(i), row, unique.as_deref_mut(), track_unique, out);
            }
        }
        DataType::Boolean => {
            let a = column.as_boolean();
            for i in 0..a.len() {
                let row = base_row + i as u64;
                if !null_gate(model, field, a.is_null(i), row, out) {
                    continue;
                }
                let v = a.value(i);
                if let Some(allowed) = &field.allowed {
                    if !allowed.contains_bool(v) {
                        push_allowed_violation(model, field, &v.to_string(), row, out);
                    }
                }
                if track_unique {
                    check_unique(model, field, field_idx, &v.to_string(), row, unique.as_deref_mut(), out);
                }
            }
        }
        dt => {
            // Numeric / temporal / fixed-binary columns share one shape:
            // extract a per-row scalar, then run numeric constraints +
            // uniqueness on it.
            macro_rules! numeric_loop {
                ($cast:expr, $to_num:expr) => {{
                    let a = $cast;
                    for i in 0..a.len() {
                        let row = base_row + i as u64;
                        if !null_gate(model, field, a.is_null(i), row, out) {
                            continue;
                        }
                        #[allow(clippy::redundant_closure_call)]
                        let (num, key): (NumVal, String) = $to_num(a.value(i));
                        check_numeric_value(model, field, num, &key, row, out);
                        if track_unique {
                            check_unique(model, field, field_idx, &key, row, unique.as_deref_mut(), out);
                        }
                    }
                }};
            }
            match dt {
                DataType::Int8 => numeric_loop!(column.as_primitive::<Int8Type>(), |v: i8| (NumVal::Int(v as i128), (v as i64).to_string())),
                DataType::Int16 => numeric_loop!(column.as_primitive::<Int16Type>(), |v: i16| (NumVal::Int(v as i128), (v as i64).to_string())),
                DataType::Int32 => numeric_loop!(column.as_primitive::<Int32Type>(), |v: i32| (NumVal::Int(v as i128), (v as i64).to_string())),
                DataType::Int64 => numeric_loop!(column.as_primitive::<Int64Type>(), |v: i64| (NumVal::Int(v as i128), v.to_string())),
                DataType::UInt8 => numeric_loop!(column.as_primitive::<UInt8Type>(), |v: u8| (NumVal::Int(v as i128), (v as u64).to_string())),
                DataType::UInt16 => numeric_loop!(column.as_primitive::<UInt16Type>(), |v: u16| (NumVal::Int(v as i128), (v as u64).to_string())),
                DataType::UInt32 => numeric_loop!(column.as_primitive::<UInt32Type>(), |v: u32| (NumVal::Int(v as i128), (v as u64).to_string())),
                DataType::UInt64 => numeric_loop!(column.as_primitive::<UInt64Type>(), |v: u64| (NumVal::Int(v as i128), v.to_string())),
                DataType::Float32 => numeric_loop!(column.as_primitive::<Float32Type>(), |v: f32| (NumVal::F32(v), v.to_string())),
                DataType::Float64 => numeric_loop!(column.as_primitive::<Float64Type>(), |v: f64| (NumVal::F64(v), v.to_string())),
                // Temporal + binary columns: type already satisfies the
                // contract; only nulls and uniqueness are checkable (value
                // constraints were refused at column level above).
                DataType::Timestamp(TimeUnit::Second, _) => numeric_loop!(column.as_primitive::<TimestampSecondType>(), |v: i64| (NumVal::Opaque, v.to_string())),
                DataType::Timestamp(TimeUnit::Millisecond, _) => numeric_loop!(column.as_primitive::<TimestampMillisecondType>(), |v: i64| (NumVal::Opaque, v.to_string())),
                DataType::Timestamp(TimeUnit::Microsecond, _) => numeric_loop!(column.as_primitive::<TimestampMicrosecondType>(), |v: i64| (NumVal::Opaque, v.to_string())),
                DataType::Timestamp(TimeUnit::Nanosecond, _) => numeric_loop!(column.as_primitive::<TimestampNanosecondType>(), |v: i64| (NumVal::Opaque, v.to_string())),
                DataType::Date32 => numeric_loop!(column.as_primitive::<Date32Type>(), |v: i32| (NumVal::Opaque, (v as i64).to_string())),
                DataType::Date64 => numeric_loop!(column.as_primitive::<Date64Type>(), |v: i64| (NumVal::Opaque, v.to_string())),
                DataType::FixedSizeBinary(16) => {
                    let a = column.as_fixed_size_binary();
                    for i in 0..a.len() {
                        let row = base_row + i as u64;
                        if !null_gate(model, field, a.is_null(i), row, out) {
                            continue;
                        }
                        if track_unique {
                            let key: String = a.value(i).iter().map(|b| format!("{b:02x}")).collect();
                            check_unique(model, field, field_idx, &key, row, unique.as_deref_mut(), out);
                        }
                    }
                }
                // type_compatible() admits nothing else. If the two ever
                // drift, report it as an honest failure — this is a library
                // embedded in other people's pipelines, so it must not panic
                // (debug builds still assert to catch the drift in tests).
                other => {
                    debug_assert!(
                        false,
                        "column type {other} passed compatibility but has no check arm"
                    );
                    let rendered = other.to_string();
                    out.record(Some(&field.name), Rule::SchemaTypeMismatch, || Violation {
                        model: model.name.clone(),
                        field: Some(field.name.clone()),
                        rule: Rule::SchemaTypeMismatch,
                        row: None,
                        value: Some(rendered.clone()),
                        message: format!(
                            "field {:?} column type {rendered} has no value-check implementation",
                            field.name
                        ),
                    });
                }
            }
        }
    }
}

fn is_string_column(dt: &DataType) -> bool {
    matches!(dt, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View)
}

/// String-column value: shape (for stringly contract types) then constraints.
#[allow(clippy::too_many_arguments)]
fn check_str_value(
    model: &CompiledModel,
    field: &CompiledField,
    field_idx: usize,
    s: &str,
    row: u64,
    unique: Option<&mut UniqueTracker>,
    track_unique: bool,
    out: &mut Collector,
) {
    // Stringly non-string types (timestamp/date/uuid in a string column):
    // shape first; a malformed value gets one type_mismatch and no
    // constraint/uniqueness noise on top.
    let shape_ok = match field.ty {
        FieldType::Timestamp => shape::is_timestamp(s),
        FieldType::Date => shape::is_date(s),
        FieldType::Uuid => shape::is_uuid(s),
        _ => true,
    };
    if !shape_ok {
        out.record(Some(&field.name), Rule::TypeMismatch, || Violation {
            model: model.name.clone(),
            field: Some(field.name.clone()),
            rule: Rule::TypeMismatch,
            row: Some(row),
            value: Some(truncate(s, 64)),
            message: format!(
                "row {row}: field {:?} value {:?} is not a valid {}",
                field.name,
                truncate(s, 64),
                field.ty.name()
            ),
        });
        return;
    }

    if field.ty == FieldType::String {
        let len = s.chars().count();
        if let Some(lo) = field.min_length {
            if len < lo {
                out.record(Some(&field.name), Rule::MinLength, || Violation {
                    model: model.name.clone(),
                    field: Some(field.name.clone()),
                    rule: Rule::MinLength,
                    row: Some(row),
                    value: Some(truncate(s, 64)),
                    message: format!(
                        "row {row}: field {:?} length {len} is below min_length {lo}",
                        field.name
                    ),
                });
            }
        }
        if let Some(hi) = field.max_length {
            if len > hi {
                out.record(Some(&field.name), Rule::MaxLength, || Violation {
                    model: model.name.clone(),
                    field: Some(field.name.clone()),
                    rule: Rule::MaxLength,
                    row: Some(row),
                    value: Some(truncate(s, 64)),
                    message: format!(
                        "row {row}: field {:?} length {len} exceeds max_length {hi}",
                        field.name
                    ),
                });
            }
        }
        if let Some(re) = &field.pattern {
            if !re.is_match(s) {
                out.record(Some(&field.name), Rule::Pattern, || Violation {
                    model: model.name.clone(),
                    field: Some(field.name.clone()),
                    rule: Rule::Pattern,
                    row: Some(row),
                    value: Some(truncate(s, 64)),
                    message: format!(
                        "row {row}: field {:?} value {:?} does not match pattern {:?}",
                        field.name,
                        truncate(s, 64),
                        field.pattern_src.as_deref().unwrap_or("")
                    ),
                });
            }
        }
        if let Some(fmt) = field.format {
            if !shape::matches_format(fmt, s) {
                out.record(Some(&field.name), Rule::Format, || Violation {
                    model: model.name.clone(),
                    field: Some(field.name.clone()),
                    rule: Rule::Format,
                    row: Some(row),
                    value: Some(truncate(s, 64)),
                    message: format!(
                        "row {row}: field {:?} value {:?} is not a valid {}",
                        field.name,
                        truncate(s, 64),
                        fmt.name()
                    ),
                });
            }
        }
    }

    if let Some(allowed) = &field.allowed {
        if !allowed.contains_str(s) {
            push_allowed_violation(model, field, s, row, out);
        }
    }
    if track_unique {
        check_unique(model, field, field_idx, s, row, unique, out);
    }
}

fn check_numeric_value(
    model: &CompiledModel,
    field: &CompiledField,
    num: NumVal,
    key: &str,
    row: u64,
    out: &mut Collector,
) {
    // Which domain a bound compares in follows the *value's* representation:
    // integers compare in i128 (no 2^53 precision loss, no u64 wrap), f32
    // compares in f32 (widening to f64 would fabricate violations on values
    // like 0.1 that aren't exactly representable), f64 compares directly.
    let (below_min, above_max, allowed_hit): (bool, bool, Option<bool>) = match num {
        NumVal::Opaque => return,
        NumVal::Int(n) => (
            field.min.map(|m| shape::int_below_min(n, m)).unwrap_or(false),
            field.max.map(|m| shape::int_above_max(n, m)).unwrap_or(false),
            field.allowed.as_ref().map(|a| {
                if field.ty == FieldType::Integer {
                    a.contains_integer(n)
                } else {
                    a.contains_f64(n as f64)
                }
            }),
        ),
        NumVal::F64(n) => (
            field.min.map(|m| n < m).unwrap_or(false),
            field.max.map(|m| n > m).unwrap_or(false),
            field.allowed.as_ref().map(|a| a.contains_f64(n)),
        ),
        NumVal::F32(n) => (
            field.min.map(|m| n < m as f32).unwrap_or(false),
            field.max.map(|m| n > m as f32).unwrap_or(false),
            field.allowed.as_ref().map(|a| a.contains_f32(n)),
        ),
    };

    if below_min {
        let min = field.min.expect("checked");
        out.record(Some(&field.name), Rule::Min, || Violation {
            model: model.name.clone(),
            field: Some(field.name.clone()),
            rule: Rule::Min,
            row: Some(row),
            value: Some(key.to_string()),
            message: format!("row {row}: field {:?} value {key} is below min {min}", field.name),
        });
    }
    if above_max {
        let max = field.max.expect("checked");
        out.record(Some(&field.name), Rule::Max, || Violation {
            model: model.name.clone(),
            field: Some(field.name.clone()),
            rule: Rule::Max,
            row: Some(row),
            value: Some(key.to_string()),
            message: format!("row {row}: field {:?} value {key} exceeds max {max}", field.name),
        });
    }
    if allowed_hit == Some(false) {
        push_allowed_violation(model, field, key, row, out);
    }
}

fn check_unique(
    model: &CompiledModel,
    field: &CompiledField,
    field_idx: usize,
    key: &str,
    row: u64,
    unique: Option<&mut UniqueTracker>,
    out: &mut Collector,
) {
    let Some(tracker) = unique else { return };
    if !tracker.insert(field_idx, key) {
        out.record(Some(&field.name), Rule::Unique, || Violation {
            model: model.name.clone(),
            field: Some(field.name.clone()),
            rule: Rule::Unique,
            row: Some(row),
            value: Some(truncate(key, 64)),
            message: format!(
                "row {row}: field {:?} value {:?} was already seen (unique)",
                field.name,
                truncate(key, 64)
            ),
        });
    }
}

fn push_allowed_violation(
    model: &CompiledModel,
    field: &CompiledField,
    rendered: &str,
    row: u64,
    out: &mut Collector,
) {
    let allowed = field.allowed.as_ref().expect("caller checked");
    out.record(Some(&field.name), Rule::Allowed, || Violation {
        model: model.name.clone(),
        field: Some(field.name.clone()),
        rule: Rule::Allowed,
        row: Some(row),
        value: Some(truncate(rendered, 64)),
        message: format!(
            "row {row}: field {:?} value {:?} not in allowed set [{}]",
            field.name,
            truncate(rendered, 64),
            allowed.describe()
        ),
    });
}
