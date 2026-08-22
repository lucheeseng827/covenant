//! Row-engine rule coverage: every rule the contract can express fires on
//! the record that breaks it and stays quiet on the record that doesn't.

use covenant::compile::CompiledContract;
use covenant::engine::row::validate_record;
use covenant::engine::UniqueTracker;
use covenant::report::Rule;
use covenant::spec::Contract;
use serde_json::json;

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
      email:    { type: string, format: email, nullable: true }
      website:  { type: string, format: uri }
      active:   { type: boolean }
      created:  { type: timestamp, required: true }
      day:      { type: date }
      trace:    { type: uuid }
      note:     { type: string, min_length: 2, max_length: 5 }
"#;

fn compiled() -> CompiledContract {
    let doc = Contract::parse(CONTRACT, "<test>").unwrap();
    CompiledContract::compile(&doc).unwrap()
}

fn rules_for(record: serde_json::Value) -> Vec<Rule> {
    let contract = compiled();
    let model = contract.resolve_model(None).unwrap();
    let mut out = Vec::new();
    validate_record(model, &record, 0, None, &mut out);
    out.iter().map(|v| v.rule).collect()
}

fn clean_record() -> serde_json::Value {
    json!({
        "order_id": "ord_ab12",
        "amount": 50,
        "created": "2026-08-11T09:30:00Z",
    })
}

#[test]
fn clean_record_passes() {
    assert!(rules_for(clean_record()).is_empty());
}

#[test]
fn full_clean_record_passes() {
    let mut r = clean_record();
    let obj = r.as_object_mut().unwrap();
    obj.insert("ratio".into(), json!(0.5));
    obj.insert("currency".into(), json!("USD"));
    obj.insert("email".into(), json!("a@b.co"));
    obj.insert("website".into(), json!("https://acme.io/x"));
    obj.insert("active".into(), json!(true));
    obj.insert("day".into(), json!("2026-08-11"));
    obj.insert("trace".into(), json!("6f1e0d3a-8c2b-4a5d-9e7f-0123456789ab"));
    obj.insert("note".into(), json!("hey"));
    assert!(rules_for(r).is_empty());
}

#[test]
fn non_object_record() {
    assert_eq!(rules_for(json!([1, 2])), vec![Rule::RecordNotObject]);
    assert_eq!(rules_for(json!("hi")), vec![Rule::RecordNotObject]);
}

#[test]
fn required_missing() {
    let mut r = clean_record();
    r.as_object_mut().unwrap().remove("created");
    assert_eq!(rules_for(r), vec![Rule::RequiredMissing]);
}

#[test]
fn null_not_allowed_vs_nullable() {
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("amount".into(), json!(null));
    assert_eq!(rules_for(r), vec![Rule::NullNotAllowed]);

    // email is nullable — explicit null passes.
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("email".into(), json!(null));
    assert!(rules_for(r).is_empty());

    // optional non-nullable field: absent passes, null violates.
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("currency".into(), json!(null));
    assert_eq!(rules_for(r), vec![Rule::NullNotAllowed]);
}

#[test]
fn type_mismatches() {
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("amount".into(), json!("fifty"));
    assert_eq!(rules_for(r), vec![Rule::TypeMismatch]);

    // 3.5 is not an integer; 3.0 parses as float in JSON and is rejected too.
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("amount".into(), json!(3.5));
    assert_eq!(rules_for(r), vec![Rule::TypeMismatch]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("amount".into(), json!(3.0));
    assert_eq!(rules_for(r), vec![Rule::TypeMismatch]);

    // integers widen into float fields.
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("ratio".into(), json!(1));
    assert!(rules_for(r).is_empty());

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("active".into(), json!("yes"));
    assert_eq!(rules_for(r), vec![Rule::TypeMismatch]);

    // malformed timestamp/date/uuid are type mismatches.
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("created".into(), json!("2026-08-11 09:30"));
    assert_eq!(rules_for(r), vec![Rule::TypeMismatch]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("day".into(), json!("11/08/2026"));
    assert_eq!(rules_for(r), vec![Rule::TypeMismatch]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("trace".into(), json!("not-a-uuid"));
    assert_eq!(rules_for(r), vec![Rule::TypeMismatch]);
}

#[test]
fn numeric_bounds() {
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("amount".into(), json!(-1));
    assert_eq!(rules_for(r), vec![Rule::Min]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("amount".into(), json!(101));
    assert_eq!(rules_for(r), vec![Rule::Max]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("ratio".into(), json!(1.5));
    assert_eq!(rules_for(r), vec![Rule::Max]);
}

#[test]
fn string_constraints() {
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("order_id".into(), json!("nope"));
    assert_eq!(rules_for(r), vec![Rule::Pattern]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("currency".into(), json!("JPY"));
    assert_eq!(rules_for(r), vec![Rule::Allowed]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("email".into(), json!("not-an-email"));
    assert_eq!(rules_for(r), vec![Rule::Format]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("website".into(), json!("no scheme here"));
    assert_eq!(rules_for(r), vec![Rule::Format]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("note".into(), json!("x"));
    assert_eq!(rules_for(r), vec![Rule::MinLength]);

    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("note".into(), json!("toolong"));
    assert_eq!(rules_for(r), vec![Rule::MaxLength]);
}

#[test]
fn strict_rejects_undeclared_fields() {
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("surprise".into(), json!(1));
    assert_eq!(rules_for(r), vec![Rule::UnexpectedField]);
}

#[test]
fn unique_across_records() {
    let contract = compiled();
    let model = contract.resolve_model(None).unwrap();
    let mut tracker = UniqueTracker::new(model);
    let mut out = Vec::new();

    let a = clean_record();
    assert!(validate_record(model, &a, 0, Some(&mut tracker), &mut out));

    // Same order_id again → unique violation.
    assert!(!validate_record(model, &a, 1, Some(&mut tracker), &mut out));
    assert_eq!(out.iter().map(|v| v.rule).collect::<Vec<_>>(), vec![Rule::Unique]);

    // A different id is fine.
    let mut b = clean_record();
    b.as_object_mut().unwrap().insert("order_id".into(), json!("ord_zz99"));
    out.clear();
    assert!(validate_record(model, &b, 2, Some(&mut tracker), &mut out));
}

/// Regression: u64 allowed values above i64::MAX must not wrap through an
/// `as i64` cast — both false rejects (value in set reported as violation)
/// and false accepts (wrapped value colliding with a negative entry).
#[test]
fn u64_allowed_values_above_i64_max() {
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
    let mut out = Vec::new();

    // The value is literally in the allowed set — must pass.
    let rec: serde_json::Value = serde_json::from_str(r#"{"id": 18446744073709551615}"#).unwrap();
    assert!(validate_record(model, &rec, 0, None, &mut out), "{out:?}");

    // With allowed [-1], the same value must NOT pass via wrapping (u64::MAX as i64 == -1).
    let contract = Contract::parse(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { id: { type: integer, allowed: [-1] } } } }
"#,
        "<test>",
    )
    .unwrap();
    let compiled = CompiledContract::compile(&contract).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    out.clear();
    assert!(!validate_record(model, &rec, 0, None, &mut out));
    assert_eq!(out[0].rule, Rule::Allowed);
}

/// Regression: integer bounds compare in the integer domain — a bound at
/// 2^53 must reject 2^53+1 instead of losing it to f64 rounding.
#[test]
fn integer_bounds_beyond_f64_precision() {
    let contract = Contract::parse(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { n: { type: integer, max: 9007199254740992 } } } }
"#,
        "<test>",
    )
    .unwrap();
    let compiled = CompiledContract::compile(&contract).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let mut out = Vec::new();

    let at_bound: serde_json::Value = serde_json::from_str(r#"{"n": 9007199254740992}"#).unwrap();
    assert!(validate_record(model, &at_bound, 0, None, &mut out), "{out:?}");

    let over: serde_json::Value = serde_json::from_str(r#"{"n": 9007199254740993}"#).unwrap();
    assert!(!validate_record(model, &over, 1, None, &mut out));
    assert_eq!(out[0].rule, Rule::Max);
}

/// Regression: float uniqueness keys by numeric value, so JSON `1` and `1.0`
/// (equal float values in different renderings) collide as duplicates.
#[test]
fn float_unique_normalizes_representations() {
    let contract = Contract::parse(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { x: { type: float, unique: true } } } }
"#,
        "<test>",
    )
    .unwrap();
    let compiled = CompiledContract::compile(&contract).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let mut tracker = UniqueTracker::new(model);
    let mut out = Vec::new();

    let a: serde_json::Value = serde_json::from_str(r#"{"x": 1}"#).unwrap();
    assert!(validate_record(model, &a, 0, Some(&mut tracker), &mut out));
    let b: serde_json::Value = serde_json::from_str(r#"{"x": 1.0}"#).unwrap();
    assert!(!validate_record(model, &b, 1, Some(&mut tracker), &mut out));
    assert_eq!(out[0].rule, Rule::Unique);

    // -0.0 == 0.0: one value, one key, so the pair is a duplicate too.
    out.clear();
    let c: serde_json::Value = serde_json::from_str(r#"{"x": 0.0}"#).unwrap();
    assert!(validate_record(model, &c, 2, Some(&mut tracker), &mut out));
    let d: serde_json::Value = serde_json::from_str(r#"{"x": -0.0}"#).unwrap();
    assert!(!validate_record(model, &d, 3, Some(&mut tracker), &mut out));
    assert_eq!(out[0].rule, Rule::Unique);
}

#[test]
fn violation_carries_row_and_field() {
    let contract = compiled();
    let model = contract.resolve_model(None).unwrap();
    let mut out = Vec::new();
    let mut r = clean_record();
    r.as_object_mut().unwrap().insert("amount".into(), json!(-5));
    validate_record(model, &r, 42, None, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].row, Some(42));
    assert_eq!(out[0].field.as_deref(), Some("amount"));
    assert!(out[0].message.contains("row 42"));
}
