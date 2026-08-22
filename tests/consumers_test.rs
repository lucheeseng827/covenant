//! Consumer manifests + blast radius: manifest validation, directory
//! loading, the matching rules (field / model / contract-wide / excluded),
//! and the CLI exit-code contract for `--fail-on breaking-with-consumers`.

use std::process::Command;

use covenant::consumers::{annotate, load_dir, ConsumerManifest};
use covenant::diff::{diff, Severity};
use covenant::spec::Contract;

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

const OLD: &str = r#"
covenant: 1
id: orders
version: 1.0.0
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true }
      amount:   { type: integer, required: true, min: 0 }
      currency: { type: string, allowed: [USD, EUR] }
      email:    { type: string, nullable: true }
  refunds:
    fields:
      refund_id: { type: string, required: true }
"#;

fn contract(src: &str) -> Contract {
    Contract::parse(src, "<test>").unwrap()
}

fn manifest(src: &str) -> ConsumerManifest {
    ConsumerManifest::parse(src, "<test>").unwrap()
}

fn write(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path
}

// ---- manifest parsing + validation ----

#[test]
fn manifest_parses_and_round_trips() {
    let m = manifest(
        r#"
consumer: 1
id: rollup
owner: fin@acme.io
consumes:
  - contract: orders
    model: orders
    fields: [email, currency]
"#,
    );
    assert_eq!(m.id, "rollup");
    assert_eq!(m.owner.as_deref(), Some("fin@acme.io"));
    assert_eq!(m.consumes.len(), 1);
    assert_eq!(m.consumes[0].fields.as_deref(), Some(&["email".to_string(), "currency".into()][..]));
}

#[test]
fn manifest_rejects_unknown_keys() {
    let err = ConsumerManifest::parse(
        "consumer: 1\nid: x\nconsumes: [{contract: orders}]\nfeilds: []\n",
        "<test>",
    )
    .unwrap_err();
    assert!(err.to_string().contains("consumer manifest error"), "{err}");
    // The message must name the offending key — that's what makes
    // deny_unknown_fields a usable typo tripwire.
    assert!(err.to_string().contains("feilds"), "{err}");
}

#[test]
fn manifest_rejects_a_blank_model_selector() {
    // model: "" would match no model at all and silently shrink the blast
    // radius — the exact opposite of what the author meant.
    let err = ConsumerManifest::parse(
        "consumer: 1\nid: x\nconsumes: [{contract: orders, model: \"  \", fields: [a]}]\n",
        "<test>",
    )
    .unwrap_err();
    assert!(err.to_string().contains("model is blank"), "{err}");
}

#[test]
fn manifest_rejects_wrong_revision() {
    let err =
        ConsumerManifest::parse("consumer: 2\nid: x\nconsumes: [{contract: orders}]\n", "<test>")
            .unwrap_err();
    assert!(err.to_string().contains("revision 2"), "{err}");
}

#[test]
fn manifest_rejects_empty_id_and_empty_consumes() {
    assert!(ConsumerManifest::parse("consumer: 1\nid: \"\"\nconsumes: [{contract: c}]\n", "<t>")
        .is_err());
    assert!(ConsumerManifest::parse("consumer: 1\nid: x\nconsumes: []\n", "<t>").is_err());
}

#[test]
fn manifest_rejects_explicit_empty_fields_list() {
    let err = ConsumerManifest::parse(
        "consumer: 1\nid: x\nconsumes: [{contract: orders, fields: []}]\n",
        "<test>",
    )
    .unwrap_err();
    assert!(err.to_string().contains("empty list"), "{err}");
}

// ---- directory loading ----

#[test]
fn load_dir_is_recursive_deterministic_and_skips_non_manifest_files() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "b.yaml", "consumer: 1\nid: b\nconsumes: [{contract: orders}]\n");
    write(dir.path(), "sub/a.yml", "consumer: 1\nid: a\nconsumes: [{contract: orders}]\n");
    write(dir.path(), "README.md", "not a manifest");
    let ids: Vec<String> = load_dir(dir.path()).unwrap().into_iter().map(|m| m.id).collect();
    // Sorted by path: b.yaml < sub/a.yml.
    assert_eq!(ids, vec!["b", "a"]);
}

#[test]
fn load_dir_refuses_an_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_dir(dir.path()).unwrap_err();
    assert!(err.to_string().contains("no consumer manifests"), "{err}");
}

#[test]
fn load_dir_fails_on_an_invalid_manifest() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "ok.yaml", "consumer: 1\nid: ok\nconsumes: [{contract: orders}]\n");
    write(dir.path(), "bad.yaml", "consumer: 1\nid: bad\n");
    assert!(load_dir(dir.path()).is_err());
}

#[cfg(unix)]
#[test]
fn load_dir_does_not_recurse_through_symlinked_directories() {
    // A symlink back to the root would recurse forever if followed; a
    // symlinked manifest FILE should still load.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.yaml", "consumer: 1\nid: a\nconsumes: [{contract: orders}]\n");
    std::os::unix::fs::symlink(dir.path(), dir.path().join("cycle")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("a.yaml"), dir.path().join("link.yaml")).unwrap();
    let ids: Vec<String> = load_dir(dir.path()).unwrap().into_iter().map(|m| m.id).collect();
    assert_eq!(ids, vec!["a", "a"], "real file + symlinked file, no cycle");
}

// ---- matching rules ----

fn radius(old: &str, new: &str, manifests: &[ConsumerManifest]) -> covenant::diff::DiffReport {
    let old = contract(old);
    let new = contract(new);
    let mut report = diff(&old, &new);
    annotate(&mut report, &old, &new, manifests);
    report
}

#[test]
fn field_removal_hits_the_consumer_that_declared_it() {
    let new = OLD.replace("      email:    { type: string, nullable: true }\n", "");
    let m = manifest("consumer: 1\nid: rollup\nowner: fin@acme.io\nconsumes: [{contract: orders, model: orders, fields: [email]}]\n");
    let report = radius(OLD, &new, &[m]);
    let ci = report.consumer_impact.unwrap();
    assert_eq!(ci.consumers_of_contract, 1);
    assert_eq!(ci.impacted.len(), 1);
    let hit = &ci.impacted[0];
    assert_eq!(hit.consumer, "rollup");
    assert_eq!(hit.owner.as_deref(), Some("fin@acme.io"));
    assert_eq!(hit.severity, Severity::Breaking);
    assert_eq!(hit.fields, vec!["email"]);
    assert_eq!(hit.changes, vec!["models.orders.fields.email"]);
    assert!(ci.unaffected.is_empty());
}

#[test]
fn producer_side_changes_never_hit_consumers() {
    // min tightened + field became required: both producer-impact.
    let new = OLD
        .replace("min: 0", "min: 100")
        .replace("currency: { type: string,", "currency: { type: string, required: true,");
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [amount, currency]}]\n");
    let report = radius(OLD, &new, &[m]);
    let ci = report.consumer_impact.unwrap();
    assert!(ci.impacted.is_empty(), "{:?}", ci.impacted);
    assert_eq!(ci.unaffected, vec!["r"]);
}

#[test]
fn whole_model_consumer_is_hit_by_any_consumer_impacting_field_change() {
    let new = OLD.replace("      email:    { type: string, nullable: true }\n", "");
    let m = manifest("consumer: 1\nid: replicator\nconsumes: [{contract: orders, model: orders}]\n");
    let report = radius(OLD, &new, &[m]);
    let ci = report.consumer_impact.unwrap();
    assert_eq!(ci.impacted.len(), 1);
    assert_eq!(ci.impacted[0].severity, Severity::Breaking);
}

#[test]
fn omitted_model_matches_any_model_of_the_contract() {
    // refund_id removed from the refunds model; the consumer names no model.
    let new = OLD.replace("      refund_id: { type: string, required: true }\n", "      other: { type: string }\n");
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [refund_id]}]\n");
    let report = radius(OLD, &new, &[m]);
    assert_eq!(report.consumer_impact.unwrap().impacted.len(), 1);
}

#[test]
fn model_scoped_consumer_ignores_changes_in_other_models() {
    let new = OLD.replace("      refund_id: { type: string, required: true }\n", "      other: { type: string }\n");
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, model: orders}]\n");
    let report = radius(OLD, &new, &[m]);
    let ci = report.consumer_impact.unwrap();
    assert!(ci.impacted.is_empty(), "{:?}", ci.impacted);
}

#[test]
fn other_contracts_manifests_are_counted_but_not_matched() {
    let new = OLD.replace("      email:    { type: string, nullable: true }\n", "");
    let m = manifest("consumer: 1\nid: other\nconsumes: [{contract: payments, fields: [email]}]\n");
    let report = radius(OLD, &new, &[m]);
    let ci = report.consumer_impact.unwrap();
    assert_eq!(ci.manifests, 1);
    assert_eq!(ci.consumers_of_contract, 0);
    assert!(ci.impacted.is_empty());
    assert!(ci.unaffected.is_empty());
}

#[test]
fn model_removal_hits_every_consumer_of_that_model() {
    let new = r#"
covenant: 1
id: orders
version: 1.0.0
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true }
      amount:   { type: integer, required: true, min: 0 }
      currency: { type: string, allowed: [USD, EUR] }
      email:    { type: string, nullable: true }
"#;
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, model: refunds, fields: [refund_id]}]\n");
    let report = radius(OLD, new, &[m]);
    let ci = report.consumer_impact.unwrap();
    assert_eq!(ci.impacted.len(), 1);
    assert_eq!(ci.impacted[0].severity, Severity::Breaking);
    assert_eq!(ci.impacted[0].changes, vec!["models.refunds"]);
    // Model-level hit: no specific declared field to name.
    assert!(ci.impacted[0].fields.is_empty());
}

#[test]
fn policy_weakening_hits_all_consumers_contract_wide() {
    let old = format!("{OLD}policy: {{ on_violation: block }}\n");
    let new = format!(
        "{}policy: {{ on_violation: warn }}\n",
        OLD.replace("version: 1.0.0", "version: 1.1.0")
    );
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [order_id]}]\n");
    let report = radius(&old, &new, &[m]);
    let ci = report.consumer_impact.unwrap();
    assert_eq!(ci.impacted.len(), 1);
    assert_eq!(ci.impacted[0].changes, vec!["policy.on_violation"]);
    assert!(ci.impacted[0].fields.is_empty());
}

#[test]
fn the_version_bump_finding_alone_is_not_a_blast_radius() {
    // email nullable -> non-nullable is producer-impact (risky), and the
    // version didn't move: the only consumer-impacting finding is the
    // version-bump one, which is process discipline and excluded.
    let new = OLD.replace("nullable: true", "nullable: false");
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders}]\n");
    let report = radius(OLD, &new, &[m]);
    assert!(report.changes.iter().any(|c| c.path == "version"), "precondition");
    let ci = report.consumer_impact.unwrap();
    assert!(ci.impacted.is_empty(), "{:?}", ci.impacted);
    assert_eq!(ci.unaffected, vec!["r"]);
}

#[test]
fn severity_is_the_worst_hit_and_impacted_sorts_worst_first() {
    // email removed (breaking) + allowed widened (risky, consumers).
    let new = OLD
        .replace("      email:    { type: string, nullable: true }\n", "")
        .replace("allowed: [USD, EUR]", "allowed: [USD, EUR, JPY]");
    let worst = manifest("consumer: 1\nid: a_worst\nconsumes: [{contract: orders, fields: [email, currency]}]\n");
    let mild = manifest("consumer: 1\nid: z_mild\nconsumes: [{contract: orders, fields: [currency]}]\n");
    // Pass the mild one first: sorting must be by severity, not input order.
    let report = radius(OLD, &new, &[mild, worst]);
    let ci = report.consumer_impact.unwrap();
    assert_eq!(ci.impacted.len(), 2);
    assert_eq!(ci.impacted[0].consumer, "a_worst");
    assert_eq!(ci.impacted[0].severity, Severity::Breaking);
    assert_eq!(ci.impacted[0].fields, vec!["currency", "email"]);
    assert_eq!(ci.impacted[1].consumer, "z_mild");
    assert_eq!(ci.impacted[1].severity, Severity::Risky);
}

#[test]
fn ambiguous_dotted_paths_widen_the_radius_instead_of_hiding_consumers() {
    // Model "a.b" with field "fields.c" and model "a.b.fields" with field
    // "c" both render to path models.a.b.fields.fields.c. Every
    // interpretation must count: consumers of EITHER field are hit.
    let old = r#"
covenant: 1
id: orders
version: 1.0.0
models:
  a.b:
    fields:
      fields.c: { type: string }
  a.b.fields:
    fields:
      c: { type: string }
"#;
    // Remove the field from model "a.b" only (breaking, consumers).
    let new = r#"
covenant: 1
id: orders
version: 2.0.0
models:
  a.b:
    fields:
      other: { type: string }
  a.b.fields:
    fields:
      c: { type: string }
"#;
    let first = manifest(
        "consumer: 1\nid: first\nconsumes: [{contract: orders, model: a.b, fields: [fields.c]}]\n",
    );
    let second = manifest(
        "consumer: 1\nid: second\nconsumes: [{contract: orders, model: a.b.fields, fields: [c]}]\n",
    );
    let report = radius(old, new, &[first, second]);
    let ci = report.consumer_impact.unwrap();
    let hit: Vec<&str> = ci.impacted.iter().map(|c| c.consumer.as_str()).collect();
    assert!(hit.contains(&"first") && hit.contains(&"second"), "{hit:?}");
}

#[test]
fn render_labels_model_scoped_hits_as_model_scoped_not_contract_wide() {
    let new = r#"
covenant: 1
id: orders
version: 1.0.0
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true }
      amount:   { type: integer, required: true, min: 0 }
      currency: { type: string, allowed: [USD, EUR] }
      email:    { type: string, nullable: true }
"#;
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, model: refunds}]\n");
    let report = radius(OLD, new, &[m]);
    let human = report.render_human();
    assert!(human.contains("model-scoped"), "{human}");
    assert!(!human.contains("contract-wide"), "{human}");
}

#[test]
fn no_manifests_supplied_means_no_consumer_impact_key() {
    let old = contract(OLD);
    let new = contract(&OLD.replace("min: 0", "min: 1"));
    let report = diff(&old, &new);
    assert!(report.consumer_impact.is_none());
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("consumer_impact").is_none());
}

// ---- consumer-side verification (verify_against / scoping) ----

#[test]
fn verify_against_is_clean_for_a_consistent_manifest() {
    let m = manifest(
        "consumer: 1\nid: r\nconsumes: [{contract: orders, model: orders, fields: [email, amount], verified: 1.0.0}]\n",
    );
    assert!(m.verify_against(&contract(OLD)).is_empty());
}

#[test]
fn verify_against_flags_missing_fields_and_models() {
    let m = manifest(
        "consumer: 1\nid: r\nconsumes: [{contract: orders, model: orders, fields: [ghost]}, {contract: orders, model: nope}]\n",
    );
    let findings = m.verify_against(&contract(OLD));
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(findings.iter().any(|f| f.path == "consumes[0].fields.ghost"
        && f.message.contains("not enforced")));
    assert!(findings.iter().any(|f| f.path == "consumes[1].model"
        && f.message.contains("available: orders, refunds")));
}

#[test]
fn verify_against_with_omitted_model_accepts_fields_from_any_model() {
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [refund_id]}]\n");
    assert!(m.verify_against(&contract(OLD)).is_empty());
}

#[test]
fn verify_against_grades_version_pin_drift() {
    let doc = contract(&OLD.replace("version: 1.0.0", "version: 2.1.0"));
    // Major behind = error; minor behind = warning; ahead = warning.
    let stale = manifest("consumer: 1\nid: a\nconsumes: [{contract: orders, verified: 1.9.0}]\n");
    let findings = stale.verify_against(&doc);
    assert!(findings.iter().any(|f| matches!(f.level, covenant::spec::LintLevel::Error)
        && f.message.contains("major")), "{findings:?}");

    let drift = manifest("consumer: 1\nid: b\nconsumes: [{contract: orders, verified: 2.0.0}]\n");
    let findings = drift.verify_against(&doc);
    assert_eq!(findings.len(), 1);
    assert!(matches!(findings[0].level, covenant::spec::LintLevel::Warning));
    assert!(findings[0].message.contains("v2.0.0 -> v2.1.0"), "{}", findings[0].message);

    let ahead = manifest("consumer: 1\nid: c\nconsumes: [{contract: orders, verified: 3.0.0}]\n");
    let findings = ahead.verify_against(&doc);
    assert_eq!(findings.len(), 1);
    assert!(matches!(findings[0].level, covenant::spec::LintLevel::Warning));
    assert!(findings[0].message.contains("ahead"), "{}", findings[0].message);
}

#[test]
fn a_0x_minor_bump_past_the_pin_is_breaking_not_a_warning() {
    // Semver item 4: pre-1.0, minor bumps are the breaking segment. A pin
    // at 0.1.0 against a 0.2.0 contract is exactly the stale-pin case the
    // gate exists to block.
    let doc = contract(&OLD.replace("version: 1.0.0", "version: 0.2.0"));
    let pinned = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, verified: 0.1.0}]\n");
    let findings = pinned.verify_against(&doc);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(matches!(findings[0].level, covenant::spec::LintLevel::Error));
    assert!(findings[0].message.contains("0.x minor"), "{}", findings[0].message);

    // A 0.x PATCH bump stays a warning — that segment is non-breaking.
    let doc = contract(&OLD.replace("version: 1.0.0", "version: 0.1.5"));
    let findings = pinned.verify_against(&doc);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(matches!(findings[0].level, covenant::spec::LintLevel::Warning));
}

#[test]
fn manifest_rejects_a_non_semver_pin() {
    let err = ConsumerManifest::parse(
        "consumer: 1\nid: x\nconsumes: [{contract: orders, verified: latest}]\n",
        "<test>",
    )
    .unwrap_err();
    assert!(err.to_string().contains("not valid semver"), "{err}");
}

#[test]
fn scoping_filters_fields_and_switches_strict_off() {
    let doc = contract(OLD);
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, model: orders, fields: [amount]}]\n");
    let scoped = covenant::consumers::scope_contract_to_consumer(&doc, "orders", &m, "<t>").unwrap();
    let model = &scoped.models["orders"];
    assert!(!model.strict);
    assert_eq!(model.fields.keys().collect::<Vec<_>>(), vec!["amount"]);
    // Other models are untouched.
    assert!(scoped.models.contains_key("refunds"));
}

#[test]
fn scoping_keeps_all_fields_for_a_whole_model_consumer() {
    let doc = contract(OLD);
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, model: orders}]\n");
    let scoped = covenant::consumers::scope_contract_to_consumer(&doc, "orders", &m, "<t>").unwrap();
    let model = &scoped.models["orders"];
    assert!(!model.strict);
    assert_eq!(model.fields.len(), doc.models["orders"].fields.len());
}

#[test]
fn scoping_refuses_when_the_declared_fields_all_live_in_another_model() {
    // Model omitted + fields that exist only in `refunds`: verify_against
    // passes (any-model rule), but scoping the `orders` model to it would
    // retain zero fields — that must be this clear diagnosis, not a
    // "contract declares no fields" compile error blaming a valid contract.
    let doc = contract(OLD);
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [refund_id]}]\n");
    let err = covenant::consumers::scope_contract_to_consumer(&doc, "orders", &m, "<t>")
        .unwrap_err();
    assert!(err.to_string().contains("declares no fields of model"), "{err}");
}

#[test]
fn scoping_returns_an_error_for_an_unknown_model_name_instead_of_panicking() {
    let doc = contract(OLD);
    // Whole-contract block: the blocks filter matches any model name, so a
    // bad name used to reach an expect() — it must be a ModelNotFound error.
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [amount]}]\n");
    let err = covenant::consumers::scope_contract_to_consumer(&doc, "typo", &m, "<t>")
        .unwrap_err();
    assert!(err.to_string().contains("not found in contract"), "{err}");
}

#[test]
fn scoping_refuses_a_stale_or_unrelated_manifest() {
    let doc = contract(OLD);
    let stale = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [ghost]}]\n");
    let err = covenant::consumers::scope_contract_to_consumer(&doc, "orders", &stale, "<t>")
        .unwrap_err();
    assert!(err.to_string().contains("consumer-check"), "{err}");

    let other = manifest("consumer: 1\nid: r\nconsumes: [{contract: payments, fields: [x]}]\n");
    let err = covenant::consumers::scope_contract_to_consumer(&doc, "orders", &other, "<t>")
        .unwrap_err();
    assert!(err.to_string().contains("declares nothing"), "{err}");
}

// ---- CLI: exit codes + output ----

fn cli_fixtures(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let old = write(dir, "old.yaml", OLD);
    // email removed (breaking, consumers) with a proper major bump so the
    // only breaking finding is the field-level one.
    let new = write(
        dir,
        "new.yaml",
        &OLD.replace("      email:    { type: string, nullable: true }\n", "")
            .replace("version: 1.0.0", "version: 2.0.0"),
    );
    (old, new)
}

#[test]
fn cli_diff_with_consumers_renders_the_blast_radius() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = cli_fixtures(dir.path());
    write(&dir.path().join("consumers"), "rollup.yaml",
        "consumer: 1\nid: rollup\nowner: fin@acme.io\nconsumes: [{contract: orders, fields: [email]}]\n");
    write(&dir.path().join("consumers"), "safe.yaml",
        "consumer: 1\nid: safe\nconsumes: [{contract: orders, fields: [order_id]}]\n");
    let out = Command::new(BIN)
        .args(["diff"])
        .arg(&old)
        .arg(&new)
        .arg("--consumers")
        .arg(dir.path().join("consumers"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("impacted consumers (2 manifests, 2 consume orders)"), "{stdout}");
    assert!(stdout.contains("rollup (fin@acme.io) via email"), "{stdout}");
    assert!(stdout.contains("unaffected: safe"), "{stdout}");
}

#[test]
fn cli_diff_json_includes_consumer_impact_only_when_requested() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = cli_fixtures(dir.path());
    write(&dir.path().join("consumers"), "rollup.yaml",
        "consumer: 1\nid: rollup\nconsumes: [{contract: orders, fields: [email]}]\n");

    let with = Command::new(BIN)
        .args(["diff"])
        .arg(&old)
        .arg(&new)
        .arg("--consumers")
        .arg(dir.path().join("consumers"))
        .args(["--format", "json", "--fail-on", "never"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&with.stdout).unwrap();
    assert_eq!(json["consumer_impact"]["impacted"][0]["consumer"], "rollup");
    assert_eq!(json["consumer_impact"]["impacted"][0]["severity"], "breaking");

    let without = Command::new(BIN)
        .args(["diff"])
        .arg(&old)
        .arg(&new)
        .args(["--format", "json", "--fail-on", "never"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&without.stdout).unwrap();
    assert!(json.get("consumer_impact").is_none());
}

#[test]
fn fail_on_breaking_with_consumers_passes_when_nobody_declared_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = cli_fixtures(dir.path());
    write(&dir.path().join("consumers"), "safe.yaml",
        "consumer: 1\nid: safe\nconsumes: [{contract: orders, fields: [order_id]}]\n");
    let out = Command::new(BIN)
        .args(["diff"])
        .arg(&old)
        .arg(&new)
        .arg("--consumers")
        .arg(dir.path().join("consumers"))
        .args(["--fail-on", "breaking-with-consumers"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn fail_on_breaking_with_consumers_fails_on_a_declared_hit() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = cli_fixtures(dir.path());
    write(&dir.path().join("consumers"), "rollup.yaml",
        "consumer: 1\nid: rollup\nconsumes: [{contract: orders, fields: [email]}]\n");
    let out = Command::new(BIN)
        .args(["diff"])
        .arg(&old)
        .arg(&new)
        .arg("--consumers")
        .arg(dir.path().join("consumers"))
        .args(["--fail-on", "breaking-with-consumers"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn fail_on_breaking_with_consumers_without_manifests_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = cli_fixtures(dir.path());
    let out = Command::new(BIN)
        .args(["diff"])
        .arg(&old)
        .arg(&new)
        .args(["--fail-on", "breaking-with-consumers"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--consumers"), "stderr should name the fix");
}

#[test]
fn cli_consumer_check_exit_codes_and_output() {
    let dir = tempfile::tempdir().unwrap();
    let contract_path = write(dir.path(), "c.yaml", OLD);
    write(&dir.path().join("m"), "good.yaml",
        "consumer: 1\nid: good\nconsumes: [{contract: orders, model: orders, fields: [amount], verified: 1.0.0}]\n");
    write(&dir.path().join("m"), "other.yaml",
        "consumer: 1\nid: other\nconsumes: [{contract: payments, fields: [x]}]\n");

    // Consistent set: exit 0, human output shows OK and the skip.
    let out = Command::new(BIN)
        .args(["consumer-check"])
        .arg(dir.path().join("m"))
        .arg("--contract")
        .arg(&contract_path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("OK    good"), "{stdout}");
    assert!(stdout.contains("skip  other"), "{stdout}");
    assert!(stdout.contains("1 ok · 0 failed · 0 warned · 1 skipped"), "{stdout}");

    // A stale manifest fails the run with exit 1 and JSON carries findings.
    write(&dir.path().join("m"), "stale.yaml",
        "consumer: 1\nid: stale\nconsumes: [{contract: orders, fields: [ghost]}]\n");
    let out = Command::new(BIN)
        .args(["consumer-check"])
        .arg(dir.path().join("m"))
        .arg("--contract")
        .arg(&contract_path)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    // Same envelope as POST /v1/consumers/verify: the artifact must record
    // which contract version the pins were graded against.
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["contract"], "orders");
    assert_eq!(json["version"], "1.0.0");
    let stale = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["consumer"] == "stale")
        .unwrap();
    assert_eq!(stale["consumes_contract"], true);
    assert_eq!(stale["findings"][0]["level"], "error");

    // A set where nothing consumes the contract is a usage-level error (2).
    let out = Command::new(BIN)
        .args(["consumer-check"])
        .arg(dir.path().join("m").join("other.yaml"))
        .arg("--contract")
        .arg(&contract_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nothing to verify"));
}

#[test]
fn cli_consumer_check_refuses_an_invalid_contract() {
    // A contract failing its own lint must exit 2 like every other
    // enforcement path — NOT produce FAIL verdicts blaming the consumers
    // (a non-semver contract version would fail every pinned manifest).
    let dir = tempfile::tempdir().unwrap();
    let contract_path = write(
        dir.path(),
        "bad.yaml",
        "covenant: 1\nid: orders\nversion: \"1.0\"\nmodels: { m: { fields: { a: { type: string } } } }\n",
    );
    let manifest_path = write(dir.path(), "m.yaml",
        "consumer: 1\nid: pinned\nconsumes: [{contract: orders, fields: [a], verified: 1.0.0}]\n");
    let out = Command::new(BIN)
        .args(["consumer-check"])
        .arg(&manifest_path)
        .arg("--contract")
        .arg(&contract_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("invalid"),
        "stderr should blame the contract, not the consumer"
    );
}

#[test]
fn cli_check_as_consumer_scopes_the_verdict_to_declared_fields() {
    let dir = tempfile::tempdir().unwrap();
    let contract_path = write(dir.path(), "c.yaml", OLD);
    let manifest_path = write(dir.path(), "consumer.yaml",
        "consumer: 1\nid: amounts_job\nconsumes: [{contract: orders, model: orders, fields: [amount]}]\n");
    // Dirt ONLY in fields this consumer never reads: order_id has the wrong
    // type, an undeclared field appears despite strict: true.
    let clean_for_consumer = write(dir.path(), "clean.ndjson",
        "{\"order_id\": 123, \"amount\": 5, \"zzz\": true}\n");
    let out = Command::new(BIN)
        .args(["check"])
        .arg(&clean_for_consumer)
        .arg("--contract")
        .arg(&contract_path)
        .args(["--model", "orders", "--as-consumer"])
        .arg(&manifest_path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("checking as consumer amounts_job — 1 of 4 fields"), "{stdout}");

    // The JSON report must carry the scoping marker — a scoped verdict is
    // not full conformance to the contract version it names.
    let out = Command::new(BIN)
        .args(["check"])
        .arg(&clean_for_consumer)
        .arg("--contract")
        .arg(&contract_path)
        .args(["--model", "orders", "--as-consumer"])
        .arg(&manifest_path)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["as_consumer"], "amounts_job", "{json}");

    // Dirt in the declared field still fails.
    let dirty = write(dir.path(), "dirty.ndjson", "{\"amount\": -3}\n");
    let out = Command::new(BIN)
        .args(["check"])
        .arg(&dirty)
        .arg("--contract")
        .arg(&contract_path)
        .args(["--model", "orders", "--as-consumer"])
        .arg(&manifest_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));

    // A stale manifest cannot scope a check: hard error, not a narrower run.
    let stale_path = write(dir.path(), "stale.yaml",
        "consumer: 1\nid: stale\nconsumes: [{contract: orders, fields: [ghost]}]\n");
    let out = Command::new(BIN)
        .args(["check"])
        .arg(&clean_for_consumer)
        .arg("--contract")
        .arg(&contract_path)
        .args(["--model", "orders", "--as-consumer"])
        .arg(&stale_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("consumer-check"));
}

#[test]
fn cli_consumers_pointing_at_an_empty_dir_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = cli_fixtures(dir.path());
    std::fs::create_dir(dir.path().join("empty")).unwrap();
    let out = Command::new(BIN)
        .args(["diff"])
        .arg(&old)
        .arg(&new)
        .arg("--consumers")
        .arg(dir.path().join("empty"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no consumer manifests"),
        "stderr should explain the empty directory"
    );
}

#[test]
fn reading_a_deprecated_field_warns_with_the_migration_note() {
    let doc = contract(
        "\ncovenant: 1\nid: orders\nversion: 1.0.0\nmodels:\n  orders:\n    fields:\n      email: { type: string, deprecated: \"use contact_id\" }\n      ok:    { type: string }\n",
    );
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [email, ok]}]\n");
    let findings = m.verify_against(&doc);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(matches!(findings[0].level, covenant::spec::LintLevel::Warning));
    assert_eq!(findings[0].path, "consumes[0].fields.email");
    assert!(findings[0].message.contains("use contact_id"), "{}", findings[0].message);
}

#[test]
fn a_bare_deprecated_true_warns_without_a_note() {
    let doc = contract(
        "\ncovenant: 1\nid: orders\nversion: 1.0.0\nmodels:\n  orders:\n    fields:\n      email: { type: string, deprecated: true }\n",
    );
    let m = manifest("consumer: 1\nid: r\nconsumes: [{contract: orders, fields: [email]}]\n");
    let findings = m.verify_against(&doc);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0].message.contains("is deprecated — plan migration"),
        "{}",
        findings[0].message
    );
}

#[test]
fn a_whole_model_block_is_warned_about_deprecated_fields_too() {
    // A block with no field list reads everything — including the
    // deprecated fields — so it gets the same countdown reminder, anchored
    // at the block (the block is the reader the deprecation countdown applies to).
    let doc = contract(
        "\ncovenant: 1\nid: orders\nversion: 1.0.0\nmodels:\n  orders:\n    fields:\n      email: { type: string, deprecated: \"use contact_id\" }\n      ok:    { type: string }\n  clean:\n    fields:\n      id: { type: string }\n",
    );
    let m = manifest("consumer: 1\nid: firehose\nconsumes: [{contract: orders, model: orders}]\n");
    let findings = m.verify_against(&doc);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(matches!(findings[0].level, covenant::spec::LintLevel::Warning));
    assert_eq!(findings[0].path, "consumes[0]");
    assert!(findings[0].message.contains("deprecated field(s) email"), "{}", findings[0].message);

    // A MATCHING whole-model block over a model holding nothing deprecated
    // stays silent — the check runs and finds nothing, it isn't skipped.
    let quiet = manifest("consumer: 1\nid: quiet\nconsumes: [{contract: orders, model: clean}]\n");
    assert!(quiet.verify_against(&doc).is_empty());
}
