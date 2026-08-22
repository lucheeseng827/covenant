//! Contract parsing + linting: a contract the runtime can't fully understand
//! must be rejected before any enforcement runs.

use covenant::spec::{Contract, FieldType, LintLevel};

const GOOD: &str = r#"
covenant: 1
id: orders
version: 1.2.0
owner: data@acme.io
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true, unique: true, pattern: "^ord_[a-z0-9]{12}$" }
      amount:   { type: integer, required: true, min: 0, max: 5000000 }
      currency: { type: string, required: true, allowed: [USD, EUR, GBP] }
      email:    { type: string, format: email, nullable: true }
      created:  { type: timestamp, required: true }
"#;

#[test]
fn parses_and_lints_clean() {
    let c = Contract::parse(GOOD, "<test>").unwrap();
    assert_eq!(c.id, "orders");
    assert_eq!(c.models.len(), 1);
    let model = &c.models["orders"];
    assert!(model.strict);
    assert_eq!(model.fields["order_id"].ty, FieldType::String);
    assert!(model.fields["order_id"].unique);
    assert_eq!(model.fields["amount"].min, Some(0.0));
    let findings = c.lint();
    assert!(
        findings.iter().all(|f| f.level != LintLevel::Error),
        "unexpected errors: {findings:?}"
    );
    assert!(c.is_enforceable());
}

#[test]
fn field_order_is_preserved() {
    let c = Contract::parse(GOOD, "<test>").unwrap();
    let names: Vec<&str> = c.models["orders"].fields.keys().map(String::as_str).collect();
    assert_eq!(names, ["order_id", "amount", "currency", "email", "created"]);
}

#[test]
fn json_contracts_parse_too() {
    let json = r#"{
      "covenant": 1, "id": "t", "version": "0.1.0",
      "models": { "m": { "fields": { "a": { "type": "string" } } } }
    }"#;
    let c = Contract::parse(json, "<test>").unwrap();
    assert!(c.is_enforceable());
}

#[test]
fn unknown_keys_are_rejected() {
    let bad = r#"
covenant: 1
id: t
version: 1.0.0
models:
  m:
    fields:
      a: { type: string, patern: "typo" }
"#;
    assert!(Contract::parse(bad, "<test>").is_err());
}

fn lint_of(yaml: &str) -> Vec<(LintLevel, String)> {
    Contract::parse(yaml, "<test>")
        .unwrap()
        .lint()
        .into_iter()
        .map(|f| (f.level, f.message))
        .collect()
}

#[test]
fn bad_regex_is_an_error() {
    let findings = lint_of(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { a: { type: string, pattern: "([" } } } }
"#,
    );
    assert!(findings
        .iter()
        .any(|(l, m)| *l == LintLevel::Error && m.contains("does not compile")));
}

#[test]
fn min_above_max_is_an_error() {
    let findings = lint_of(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { a: { type: integer, min: 10, max: 1 } } } }
"#,
    );
    assert!(findings.iter().any(|(l, m)| *l == LintLevel::Error && m.contains("exceeds max")));
}

#[test]
fn constraints_on_wrong_type_are_errors() {
    let findings = lint_of(
        r#"
covenant: 1
id: t
version: 1.0.0
models:
  m:
    fields:
      a: { type: integer, pattern: "x" }
      b: { type: string, min: 1 }
      c: { type: boolean, format: email }
"#,
    );
    let errors: Vec<_> = findings.iter().filter(|(l, _)| *l == LintLevel::Error).collect();
    assert_eq!(errors.len(), 3, "{findings:?}");
}

#[test]
fn empty_allowed_and_type_incompatible_allowed_are_errors() {
    let findings = lint_of(
        r#"
covenant: 1
id: t
version: 1.0.0
models:
  m:
    fields:
      a: { type: string, allowed: [] }
      b: { type: integer, allowed: [1, "two"] }
"#,
    );
    assert!(findings.iter().any(|(l, m)| *l == LintLevel::Error && m.contains("allowed is empty")));
    assert!(findings.iter().any(|(l, m)| *l == LintLevel::Error && m.contains("is not a integer")));
}

#[test]
fn bad_semver_is_an_error() {
    let findings = lint_of(
        r#"
covenant: 1
id: t
version: not-a-version
models: { m: { fields: { a: { type: string } } } }
"#,
    );
    assert!(findings.iter().any(|(l, m)| *l == LintLevel::Error && m.contains("semver")));
}

#[test]
fn unique_nullable_is_a_warning_not_error() {
    let findings = lint_of(
        r#"
covenant: 1
id: t
version: 1.0.0
owner: x@y.z
models: { m: { fields: { a: { type: string, unique: true, nullable: true } } } }
"#,
    );
    assert!(findings.iter().any(|(l, m)| *l == LintLevel::Warning && m.contains("unique + nullable")));
    assert!(findings.iter().all(|(l, _)| *l != LintLevel::Error));
}

#[test]
fn unsupported_spec_revision_is_an_error() {
    let findings = lint_of(
        r#"
covenant: 2
id: t
version: 1.0.0
models: { m: { fields: { a: { type: string } } } }
"#,
    );
    assert!(findings
        .iter()
        .any(|(l, m)| *l == LintLevel::Error && m.contains("unsupported spec revision")));
}

#[test]
fn empty_models_and_empty_fields_are_errors() {
    let no_models = lint_of("covenant: 1\nid: t\nversion: 1.0.0\nmodels: {}\n");
    assert!(no_models
        .iter()
        .any(|(l, m)| *l == LintLevel::Error && m.contains("no models")));

    let no_fields = lint_of("covenant: 1\nid: t\nversion: 1.0.0\nmodels: { m: { fields: {} } }\n");
    assert!(no_fields
        .iter()
        .any(|(l, m)| *l == LintLevel::Error && m.contains("no fields")));
}

#[test]
fn non_finite_bounds_are_errors() {
    let findings = lint_of(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { a: { type: float, min: .nan }, b: { type: float, max: .inf } } } }
"#,
    );
    let finite_errors = findings
        .iter()
        .filter(|(l, m)| *l == LintLevel::Error && m.contains("finite"))
        .count();
    assert_eq!(finite_errors, 2, "{findings:?}");
}

/// An allowed value the engines' shape check would reject can never match a
/// conforming value — the lint flags the unsatisfiable entry.
#[test]
fn unsatisfiable_temporal_allowed_values_are_errors() {
    let findings = lint_of(
        r#"
covenant: 1
id: t
version: 1.0.0
models:
  m:
    fields:
      d: { type: date, allowed: ["11/08/2026"] }
      t: { type: timestamp, allowed: ["2026-08-11T09:30:00Z", "yesterday"] }
      u: { type: uuid, allowed: ["not-a-uuid"] }
"#,
    );
    let errs = findings
        .iter()
        .filter(|(l, m)| *l == LintLevel::Error && m.contains("could ever match"))
        .count();
    assert_eq!(errs, 3, "{findings:?}");
}

#[test]
fn compile_refuses_invalid_contract() {
    let c = Contract::parse(
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { a: { type: string, pattern: "([" } } } }
"#,
        "<test>",
    )
    .unwrap();
    let err = covenant::CompiledContract::compile(&c).unwrap_err();
    assert!(err.to_string().contains("invalid"));
}
