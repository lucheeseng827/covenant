//! Columnar-engine coverage: RecordBatches built in memory, exercising the
//! schema pass, columnar null semantics, per-value constraints, uniqueness
//! across batches, and the honest-failure arms (unsupported column types).

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::types::Int32Type;
use arrow_array::{
    ArrayRef, BooleanArray, DictionaryArray, Float32Array, Float64Array, Int64Array, RecordBatch,
    StringArray, TimestampMillisecondArray, UInt64Array,
};
use covenant::compile::CompiledContract;
use covenant::engine::arrow::validate_batch;
use covenant::engine::UniqueTracker;
use covenant::report::{Collector, Rule};
use covenant::spec::Contract;

const CONTRACT: &str = r#"
covenant: 1
id: orders
version: 1.0.0
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true, unique: true, pattern: "^ord_[a-z0-9]{4}$" }
      amount:   { type: integer, required: true, min: 0, max: 100 }
      ratio:    { type: float, min: 0.0, max: 1.0 }
      currency: { type: string, allowed: [USD, EUR] }
      active:   { type: boolean }
      created:  { type: timestamp, required: true }
"#;

fn compiled() -> CompiledContract {
    let doc = Contract::parse(CONTRACT, "<test>").unwrap();
    CompiledContract::compile(&doc).unwrap()
}

fn batch(cols: Vec<(&str, ArrayRef)>) -> RecordBatch {
    RecordBatch::try_from_iter(cols.into_iter().map(|(n, a)| (n.to_string(), a))).unwrap()
}

fn rule_counts(c: &covenant::report::CheckReport) -> HashMap<(String, &'static str), u64> {
    c.per_rule
        .iter()
        .map(|rc| ((rc.field.clone(), rc.rule), rc.count))
        .collect()
}

fn check(batches: &[RecordBatch]) -> covenant::report::CheckReport {
    let contract = compiled();
    let model = contract.resolve_model(None).unwrap();
    let mut collector = Collector::new(contract.policy.sample_violations);
    let mut unique = UniqueTracker::new(model);
    let mut rows = 0u64;
    for b in batches {
        rows += validate_batch(model, b, rows, Some(&mut unique), &mut collector);
    }
    collector.into_report(
        covenant::report::ReportHeader {
            contract_id: contract.id.clone(),
            contract_version: contract.version.clone(),
            owner: None,
            model: model.name.clone(),
            source: "<memory>".into(),
        },
        rows,
    )
}

fn clean_batch() -> RecordBatch {
    batch(vec![
        (
            "order_id",
            Arc::new(StringArray::from(vec!["ord_ab12", "ord_cd34"])) as ArrayRef,
        ),
        ("amount", Arc::new(Int64Array::from(vec![10, 99])) as ArrayRef),
        (
            "created",
            Arc::new(TimestampMillisecondArray::from(vec![1_700_000_000_000i64, 1_700_000_100_000])) as ArrayRef,
        ),
    ])
}

#[test]
fn clean_batch_passes() {
    let report = check(&[clean_batch()]);
    assert_eq!(report.rows, 2);
    assert_eq!(report.violations, 0, "{:?}", report.samples);
}

#[test]
fn missing_required_column_is_schema_violation() {
    let b = batch(vec![(
        "order_id",
        Arc::new(StringArray::from(vec!["ord_ab12"])) as ArrayRef,
    )]);
    let report = check(&[b]);
    let counts = rule_counts(&report);
    // amount and created are required and absent; ratio/currency/active are
    // optional and absent — no findings for them.
    assert_eq!(counts[&("amount".to_string(), "schema_missing_field")], 1);
    assert_eq!(counts[&("created".to_string(), "schema_missing_field")], 1);
    assert_eq!(report.violations, 2);
}

#[test]
fn incompatible_column_type_is_schema_violation() {
    let b = batch(vec![
        (
            "order_id",
            Arc::new(StringArray::from(vec!["ord_ab12"])) as ArrayRef,
        ),
        // amount declared integer, column is utf8.
        ("amount", Arc::new(StringArray::from(vec!["ten"])) as ArrayRef),
        (
            "created",
            Arc::new(TimestampMillisecondArray::from(vec![0i64])) as ArrayRef,
        ),
    ]);
    let report = check(&[b]);
    let counts = rule_counts(&report);
    assert_eq!(counts[&("amount".to_string(), "schema_type_mismatch")], 1);
}

#[test]
fn columnar_null_semantics() {
    // required non-nullable: nulls violate. optional non-nullable: nulls are
    // "absent" and pass (a column cannot distinguish the two).
    let b = batch(vec![
        (
            "order_id",
            Arc::new(StringArray::from(vec![Some("ord_ab12"), None])) as ArrayRef,
        ),
        ("amount", Arc::new(Int64Array::from(vec![Some(10), Some(20)])) as ArrayRef),
        (
            "currency",
            Arc::new(StringArray::from(vec![None::<&str>, Some("USD")])) as ArrayRef,
        ),
        (
            "created",
            Arc::new(TimestampMillisecondArray::from(vec![Some(0i64), None])) as ArrayRef,
        ),
    ]);
    let report = check(&[b]);
    let counts = rule_counts(&report);
    assert_eq!(counts[&("order_id".to_string(), "null_not_allowed")], 1);
    assert_eq!(counts[&("created".to_string(), "null_not_allowed")], 1);
    assert!(!counts.contains_key(&("currency".to_string(), "null_not_allowed")));
    assert_eq!(report.violations, 2);
}

#[test]
fn value_constraints_fire() {
    let b = batch(vec![
        (
            "order_id",
            Arc::new(StringArray::from(vec!["BAD-ID", "ord_cd34"])) as ArrayRef,
        ),
        ("amount", Arc::new(Int64Array::from(vec![-5, 101])) as ArrayRef),
        ("ratio", Arc::new(Float64Array::from(vec![0.5, 1.5])) as ArrayRef),
        (
            "currency",
            Arc::new(StringArray::from(vec!["USD", "JPY"])) as ArrayRef,
        ),
        ("active", Arc::new(BooleanArray::from(vec![true, false])) as ArrayRef),
        (
            "created",
            Arc::new(TimestampMillisecondArray::from(vec![0i64, 1])) as ArrayRef,
        ),
    ]);
    let report = check(&[b]);
    let counts = rule_counts(&report);
    assert_eq!(counts[&("order_id".to_string(), "pattern")], 1);
    assert_eq!(counts[&("amount".to_string(), "min")], 1);
    assert_eq!(counts[&("amount".to_string(), "max")], 1);
    assert_eq!(counts[&("ratio".to_string(), "max")], 1);
    assert_eq!(counts[&("currency".to_string(), "allowed")], 1);
    // Row numbers are absolute.
    assert!(report
        .samples
        .iter()
        .any(|v| v.rule == Rule::Max && v.field.as_deref() == Some("amount") && v.row == Some(1)));
}

#[test]
fn unique_spans_batches() {
    let b1 = clean_batch();
    // Second batch reuses ord_ab12 from the first.
    let b2 = batch(vec![
        (
            "order_id",
            Arc::new(StringArray::from(vec!["ord_ab12", "ord_ef56"])) as ArrayRef,
        ),
        ("amount", Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef),
        (
            "created",
            Arc::new(TimestampMillisecondArray::from(vec![2i64, 3])) as ArrayRef,
        ),
    ]);
    let report = check(&[b1, b2]);
    let counts = rule_counts(&report);
    assert_eq!(counts[&("order_id".to_string(), "unique")], 1);
    // The duplicate is at absolute row 2 (first row of batch 2).
    assert!(report
        .samples
        .iter()
        .any(|v| v.rule == Rule::Unique && v.row == Some(2)));
}

#[test]
fn strict_flags_undeclared_columns() {
    let mut cols = vec![
        (
            "order_id",
            Arc::new(StringArray::from(vec!["ord_ab12"])) as ArrayRef,
        ),
        ("amount", Arc::new(Int64Array::from(vec![1])) as ArrayRef),
        (
            "created",
            Arc::new(TimestampMillisecondArray::from(vec![0i64])) as ArrayRef,
        ),
    ];
    cols.push(("mystery", Arc::new(Int64Array::from(vec![9])) as ArrayRef));
    let report = check(&[batch(cols)]);
    let counts = rule_counts(&report);
    assert_eq!(counts[&("mystery".to_string(), "unexpected_field")], 1);
}

#[test]
fn timestamp_strings_are_validated_per_value() {
    // A string column can carry timestamps — values are shape-checked.
    let b = batch(vec![
        (
            "order_id",
            Arc::new(StringArray::from(vec!["ord_ab12", "ord_cd34"])) as ArrayRef,
        ),
        ("amount", Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef),
        (
            "created",
            Arc::new(StringArray::from(vec!["2026-08-11T09:30:00Z", "yesterday"])) as ArrayRef,
        ),
    ]);
    let report = check(&[b]);
    let counts = rule_counts(&report);
    assert_eq!(counts[&("created".to_string(), "type_mismatch")], 1);
}

#[test]
fn sample_cap_bounds_samples_not_counts() {
    // 50 bad amounts with the default cap of 10: exact count, 10 samples.
    let n = 50;
    let b = batch(vec![
        (
            "order_id",
            Arc::new(StringArray::from(
                (0..n).map(|i| format!("ord_a{i:03}")).collect::<Vec<_>>(),
            )) as ArrayRef,
        ),
        (
            "amount",
            Arc::new(Int64Array::from(vec![-1i64; n])) as ArrayRef,
        ),
        (
            "created",
            Arc::new(TimestampMillisecondArray::from(
                (0..n as i64).collect::<Vec<_>>(),
            )) as ArrayRef,
        ),
    ]);
    let report = check(&[b]);
    let counts = rule_counts(&report);
    assert_eq!(counts[&("amount".to_string(), "min")], 50);
    let min_samples = report
        .samples
        .iter()
        .filter(|v| v.rule == Rule::Min)
        .count();
    // The cap comes from the contract's policy, not a magic number.
    assert_eq!(min_samples, compiled().policy.sample_violations);
}

/// The module header's "honest failure" claim, proven: an unsupported
/// (dictionary-encoded) column is reported as schema_type_mismatch, never
/// half-checked or panicked on.
#[test]
fn dictionary_column_is_honest_schema_mismatch() {
    let dict: DictionaryArray<Int32Type> = vec!["ord_ab12", "ord_cd34"].into_iter().collect();
    let b = batch(vec![
        ("order_id", Arc::new(dict) as ArrayRef),
        ("amount", Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef),
        (
            "created",
            Arc::new(TimestampMillisecondArray::from(vec![0i64, 1])) as ArrayRef,
        ),
    ]);
    let report = check(&[b]);
    let counts = rule_counts(&report);
    assert_eq!(counts[&("order_id".to_string(), "schema_type_mismatch")], 1);
}

/// `allowed` on a native temporal column has no evaluable rendering — the
/// engine must refuse loudly (once per column), not silently pass all rows.
#[test]
fn allowed_on_native_timestamp_is_refused_not_skipped() {
    let contract = Contract::parse(
        r#"
covenant: 1
id: t
version: 1.0.0
models:
  m:
    fields:
      created: { type: timestamp, allowed: ["2026-08-11T09:30:00Z"] }
"#,
        "<test>",
    )
    .unwrap();
    let compiled = CompiledContract::compile(&contract).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let b = batch(vec![(
        "created",
        Arc::new(TimestampMillisecondArray::from(vec![0i64, 1])) as ArrayRef,
    )]);
    let mut collector = Collector::new(10);
    validate_batch(model, &b, 0, None, &mut collector);
    let report = collector.into_report(
        covenant::report::ReportHeader {
            contract_id: "t".into(),
            contract_version: "1.0.0".into(),
            owner: None,
            model: "m".into(),
            source: "<memory>".into(),
        },
        2,
    );
    let counts = rule_counts(&report);
    // Exactly one column-level refusal — not two per-row skips, not silence.
    assert_eq!(counts[&("created".to_string(), "schema_type_mismatch")], 1);
}

/// f32 values compare in f32 precision: 0.1f32 widened to f64 is not 0.1,
/// and exact-f64 matching would fabricate violations on conforming data.
#[test]
fn float32_columns_compare_in_f32_domain() {
    let contract = Contract::parse(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { r: { type: float, min: 0.1, allowed: [0.1, 0.2] } } } }
"#,
        "<test>",
    )
    .unwrap();
    let compiled = CompiledContract::compile(&contract).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let b = batch(vec![(
        "r",
        Arc::new(Float32Array::from(vec![0.1f32, 0.2])) as ArrayRef,
    )]);
    let mut collector = Collector::new(10);
    validate_batch(model, &b, 0, None, &mut collector);
    assert_eq!(collector.total(), 0);
}

/// u64 columns above i64::MAX: allowed-set membership must not wrap.
#[test]
fn uint64_allowed_above_i64_max() {
    let contract = Contract::parse(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { id: { type: integer, allowed: [18446744073709551615] } } } }
"#,
        "<test>",
    )
    .unwrap();
    let compiled = CompiledContract::compile(&contract).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let b = batch(vec![(
        "id",
        Arc::new(UInt64Array::from(vec![u64::MAX])) as ArrayRef,
    )]);
    let mut collector = Collector::new(10);
    validate_batch(model, &b, 0, None, &mut collector);
    assert_eq!(collector.total(), 0, "in-set u64::MAX must pass");
}
