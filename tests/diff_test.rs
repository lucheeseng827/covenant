//! Breaking-change classification: severity, producer/consumer impact, and
//! the semver-bump discipline.

use covenant::diff::{diff, Impact, Severity};
use covenant::spec::Contract;

fn contract(version: &str, fields_yaml: &str) -> Contract {
    let yaml = format!(
        r#"
covenant: 1
id: orders
version: {version}
models:
  orders:
    fields:
{fields_yaml}
"#
    );
    Contract::parse(&yaml, "<test>").unwrap()
}

fn base(version: &str) -> Contract {
    contract(
        version,
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    )
}

#[test]
fn no_changes_no_findings() {
    let report = diff(&base("1.0.0"), &base("1.0.0"));
    assert!(report.changes.is_empty(), "{:?}", report.changes);
    assert_eq!(report.max_severity(), None);
}

#[test]
fn field_removed_breaks_consumers() {
    let new = contract(
        "2.0.0",
        r#"
      order_id: { type: string, required: true }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &new);
    let c = report
        .changes
        .iter()
        .find(|c| c.path.ends_with("fields.amount"))
        .unwrap();
    assert_eq!(c.severity, Severity::Breaking);
    assert_eq!(c.impact, Impact::Consumers);
}

#[test]
fn required_field_added_breaks_producers() {
    let new = contract(
        "2.0.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
      region:   { type: string, required: true }
"#,
    );
    let report = diff(&base("1.0.0"), &new);
    let c = report
        .changes
        .iter()
        .find(|c| c.path.ends_with("fields.region"))
        .unwrap();
    assert_eq!(c.severity, Severity::Breaking);
    assert_eq!(c.impact, Impact::Producers);
}

#[test]
fn optional_field_added_is_info() {
    let new = contract(
        "1.0.1",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
      note:     { type: string }
"#,
    );
    let report = diff(&base("1.0.0"), &new);
    assert_eq!(report.max_severity(), Some(Severity::Info));
}

#[test]
fn type_change_breaks_both_but_int_to_float_is_risky() {
    let new = contract(
        "2.0.0",
        r#"
      order_id: { type: integer, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &new);
    let c = report
        .changes
        .iter()
        .find(|c| c.path.ends_with("fields.order_id"))
        .unwrap();
    assert_eq!((c.severity, c.impact), (Severity::Breaking, Impact::Both));

    let widened = contract(
        "1.1.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: float, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &widened);
    let c = report
        .changes
        .iter()
        .find(|c| c.path.ends_with("fields.amount"))
        .unwrap();
    assert_eq!((c.severity, c.impact), (Severity::Risky, Impact::Consumers));
}

#[test]
fn tightened_bound_hits_producers_loosened_hits_consumers() {
    let tightened = contract(
        "1.1.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 10, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &tightened);
    let c = report.changes.iter().find(|c| c.message.contains("min tightened")).unwrap();
    assert_eq!((c.severity, c.impact), (Severity::Risky, Impact::Producers));

    let loosened = contract(
        "1.1.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 500 }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &loosened);
    let c = report.changes.iter().find(|c| c.message.contains("max loosened")).unwrap();
    assert_eq!((c.severity, c.impact), (Severity::Risky, Impact::Consumers));
}

#[test]
fn allowed_set_narrow_vs_widen() {
    let narrowed = contract(
        "1.1.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD] }
"#,
    );
    let report = diff(&base("1.0.0"), &narrowed);
    let c = report.changes.iter().find(|c| c.message.contains("narrowed")).unwrap();
    assert_eq!(c.impact, Impact::Producers);

    let widened = contract(
        "1.1.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR, GBP] }
"#,
    );
    let report = diff(&base("1.0.0"), &widened);
    let c = report.changes.iter().find(|c| c.message.contains("widened")).unwrap();
    assert_eq!(c.impact, Impact::Consumers);
}

#[test]
fn nullability_and_requiredness_transitions() {
    let new = contract(
        "2.0.0",
        r#"
      order_id: { type: string, required: true, nullable: true }
      amount:   { type: integer, min: 0, max: 100, required: true }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &new);
    let nullable = report
        .changes
        .iter()
        .find(|c| c.message.contains("became nullable"))
        .unwrap();
    assert_eq!((nullable.severity, nullable.impact), (Severity::Breaking, Impact::Consumers));
    let required = report
        .changes
        .iter()
        .find(|c| c.message.contains("became required"))
        .unwrap();
    assert_eq!((required.severity, required.impact), (Severity::Breaking, Impact::Producers));
}

#[test]
fn model_removed_is_breaking() {
    let old = base("1.0.0");
    let new = Contract::parse(
        r#"
covenant: 1
id: orders
version: 2.0.0
models:
  other: { fields: { a: { type: string } } }
"#,
        "<test>",
    )
    .unwrap();
    let report = diff(&old, &new);
    assert!(report
        .changes
        .iter()
        .any(|c| c.severity == Severity::Breaking && c.message.contains("model removed")));
    assert!(report
        .changes
        .iter()
        .any(|c| c.severity == Severity::Info && c.message == "model added"));
}

#[test]
fn insufficient_version_bump_is_itself_breaking() {
    // Breaking change (field removed) with only a patch bump.
    let new = contract(
        "1.0.1",
        r#"
      order_id: { type: string, required: true }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &new);
    let v = report.changes.iter().find(|c| c.path == "version").unwrap();
    assert_eq!(v.severity, Severity::Breaking);
    assert!(v.message.contains("major"));

    // Same change with a major bump: no version finding.
    let new = contract(
        "2.0.0",
        r#"
      order_id: { type: string, required: true }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &new);
    assert!(report.changes.iter().all(|c| c.path != "version"));
}

#[test]
fn risky_change_needs_minor_bump() {
    let tightened = contract(
        "1.0.1",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 10, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&base("1.0.0"), &tightened);
    let v = report.changes.iter().find(|c| c.path == "version").unwrap();
    assert!(v.message.contains("minor"));
}

#[test]
fn version_going_backwards_without_changes_is_risky() {
    let report = diff(&base("2.0.0"), &base("1.0.0"));
    let v = report.changes.iter().find(|c| c.path == "version").unwrap();
    assert_eq!(v.severity, Severity::Risky);
    assert!(v.message.contains("backwards"));
}

/// Iterating a prerelease of the same base version is the phase where
/// breaking changes are expected — no spurious major-bump finding.
#[test]
fn prerelease_iteration_tolerates_breaking_changes() {
    let old = contract(
        "2.0.0-alpha.1",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let new = contract(
        "2.0.0-alpha.2",
        r#"
      order_id: { type: string, required: true }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&old, &new);
    assert!(
        report.changes.iter().any(|c| c.severity == Severity::Breaking),
        "field removal is still reported"
    );
    assert!(
        report.changes.iter().all(|c| c.path != "version"),
        "no version finding during prerelease iteration: {:?}",
        report.changes
    );
}

/// A version the classifier can't parse must surface a finding, not
/// silently disable the bump discipline.
#[test]
fn non_semver_version_is_flagged_not_ignored() {
    let old = contract(
        "one-point-oh",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let new = contract(
        "one-point-one",
        r#"
      order_id: { type: string, required: true }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );
    let report = diff(&old, &new);
    let v = report.changes.iter().find(|c| c.path == "version").unwrap();
    assert!(v.message.contains("not semver"));
}

/// Policy is load-bearing: weakening block -> warn is a diffable change.
#[test]
fn policy_weakening_is_risky() {
    let old = Contract::parse(
        r#"
covenant: 1
id: orders
version: 1.0.0
models: { orders: { fields: { a: { type: string } } } }
policy: { on_violation: block }
"#,
        "<test>",
    )
    .unwrap();
    let new = Contract::parse(
        r#"
covenant: 1
id: orders
version: 1.1.0
models: { orders: { fields: { a: { type: string } } } }
policy: { on_violation: warn, max_violations: 100 }
"#,
        "<test>",
    )
    .unwrap();
    let report = diff(&old, &new);
    assert!(report
        .changes
        .iter()
        .any(|c| c.path == "policy.on_violation" && c.severity == Severity::Risky));
    assert!(report
        .changes
        .iter()
        .any(|c| c.path == "policy.max_violations" && c.message.contains("raised")));
}

/// YAML `1` and `1.0` are the same allowed number — no phantom narrow+widen.
#[test]
fn allowed_numeric_representations_compare_by_value() {
    let old = contract(
        "1.0.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
      qty:      { type: float, allowed: [1, 2] }
"#,
    );
    let new = contract(
        "1.0.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
      qty:      { type: float, allowed: [1.0, 2.0] }
"#,
    );
    let report = diff(&old, &new);
    assert!(report.changes.is_empty(), "{:?}", report.changes);
}

#[test]
fn info_change_needs_any_bump() {
    let added = contract(
        "1.0.0",
        r#"
      order_id: { type: string, required: true }
      amount:   { type: integer, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
      note:     { type: string }
"#,
    );
    let report = diff(&base("1.0.0"), &added);
    let v = report.changes.iter().find(|c| c.path == "version").unwrap();
    assert!(v.message.contains("patch"));
}

#[test]
fn deprecation_transitions_are_info_level_consumer_signals() {
    let old = contract("1.0.0", "      a: { type: string }\n");
    let new = contract("1.0.1", "      a: { type: string, deprecated: \"use b\" }\n");
    let report = diff(&old, &new);
    let dep = report
        .changes
        .iter()
        .find(|c| c.message.contains("marked deprecated"))
        .expect("deprecation change");
    assert_eq!(dep.severity, Severity::Info);
    assert_eq!(dep.impact, Impact::Consumers);
    assert!(dep.message.contains("use b"), "{}", dep.message);

    // The boolean form parses too, and un-deprecating is also info.
    let old = contract("1.0.0", "      a: { type: string, deprecated: true }\n");
    let new = contract("1.0.1", "      a: { type: string }\n");
    let report = diff(&old, &new);
    assert!(report.changes.iter().any(|c| c.message.contains("no longer deprecated")));
}

#[test]
fn removing_a_deprecated_field_is_still_breaking_but_says_so() {
    let old = contract("1.0.0", "      a: { type: string, deprecated: true }\n");
    let new = contract("2.0.0", "      b: { type: string }\n");
    let report = diff(&old, &new);
    let removal = report
        .changes
        .iter()
        .find(|c| c.message.contains("field removed"))
        .expect("removal change");
    assert_eq!(removal.severity, Severity::Breaking, "deprecation never licenses a break");
    assert!(removal.message.contains("(was deprecated)"), "{}", removal.message);
}

#[test]
fn a_changed_deprecation_note_is_reported_to_consumers() {
    // Readers planning a migration against the old guidance need to hear
    // the new guidance — the note change is info-level but consumer-facing.
    let old = contract("1.1.0", "      email: { type: string, deprecated: \"use b\" }\n");
    let new = contract("1.1.1", "      email: { type: string, deprecated: \"use contact_id\" }\n");
    let report = covenant::diff::diff(&old, &new);
    assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
    assert!(matches!(report.changes[0].severity, covenant::diff::Severity::Info));
    assert!(
        report.changes[0].message.contains("deprecation note changed"),
        "{}",
        report.changes[0].message
    );
    assert!(report.changes[0].message.contains("use contact_id"), "{}", report.changes[0].message);

    // An unchanged note is not a change at all.
    let same = covenant::diff::diff(&old, &old);
    assert!(same.changes.is_empty(), "{:?}", same.changes);
}
