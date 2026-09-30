//! ODCS v3 input: the mapping onto the contract model is exact or refused.
//! Every rule either lands on an engine rule with the same meaning or is
//! listed as unenforced with its path — never dropped silently.

use std::path::{Path, PathBuf};
use std::process::Command;

use covenant::compile::CompiledContract;
use covenant::spec::{Contract, Field, FieldType, LoadedContract, SourceFormat, StringFormat};

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

fn load(yaml: &str) -> LoadedContract {
    Contract::load(yaml, "test.odcs.yaml").expect("an ODCS v3 document loads")
}

fn field<'a>(l: &'a LoadedContract, model: &str, field: &str) -> &'a Field {
    &l.contract.models[model].fields[field]
}

fn unenforced_paths(l: &LoadedContract) -> Vec<&str> {
    l.unenforced.iter().map(|r| r.path.as_str()).collect()
}

/// Everything in here has an exact equivalent, so nothing is unenforced.
const ORDERS: &str = r#"
apiVersion: v3.2.0
kind: DataContract
id: orders
version: 1.2.0
name: Orders
description:
  purpose: Orders as the shop emits them.
team:
  name: data-platform@acme.io
schema:
  - name: orders
    description: One row per order.
    properties:
      - name: order_id
        primaryKey: true
        logicalType: string
        logicalTypeOptions:
          pattern: "^ord_[a-z0-9]{4}$"
          minLength: 8
          maxLength: 8
      - name: amount
        physicalName: amount_cents
        logicalType: integer
        required: true
        logicalTypeOptions:
          minimum: 0
          exclusiveMaximum: 100
      - name: currency
        logicalType: string
        required: true
        enum:
          - value: USD
            label: US dollar
          - value: EUR
      - name: email
        logicalType: string
        logicalTypeOptions:
          format: email
      - name: customer_id
        logicalType: string
        logicalTypeOptions:
          format: uuid
      - name: placed_on
        logicalType: date
        logicalTypeOptions:
          format: yyyy-MM-dd
      - name: placed_at
        logicalType: timestamp
      - name: gift
        logicalType: boolean
        deprecated: true
      - name: score
        logicalType: number
        logicalTypeOptions:
          minimum: 0.5
          maximum: 9.5
"#;

#[test]
fn a_fully_enforceable_contract_maps_exactly() {
    let l = load(ORDERS);
    assert_eq!(
        l.format,
        SourceFormat::Odcs {
            api_version: "v3.2.0".into()
        }
    );
    assert!(l.unenforced.is_empty(), "unexpected: {:?}", l.unenforced);
    let c = &l.contract;
    assert_eq!(c.id, "orders");
    assert_eq!(c.version, "1.2.0");
    assert_eq!(c.owner.as_deref(), Some("data-platform@acme.io"));
    assert_eq!(
        c.description.as_deref(),
        Some("Orders as the shop emits them.")
    );
    assert!(!c.models["orders"].strict, "ODCS schemas are open-world");

    // The only primary key: unique and not null.
    let id = field(&l, "orders", "order_id");
    assert!(id.unique && id.required && !id.nullable);
    assert_eq!(id.pattern.as_deref(), Some("^ord_[a-z0-9]{4}$"));
    assert_eq!((id.min_length, id.max_length), (Some(8), Some(8)));

    // The data carries the physical column name; an exclusive integer
    // bound is exact (< 100 is <= 99).
    let amount = field(&l, "orders", "amount_cents");
    assert_eq!(amount.ty, FieldType::Integer);
    assert!(amount.required && !amount.nullable);
    assert_eq!((amount.min, amount.max), (Some(0.0), Some(99.0)));

    let currency = field(&l, "orders", "currency");
    assert_eq!(
        currency.allowed,
        Some(vec![serde_json::json!("USD"), serde_json::json!("EUR")])
    );

    // ODCS `required: false` (the default): null or absent are both fine.
    let email = field(&l, "orders", "email");
    assert!(!email.required && email.nullable);
    assert_eq!(email.format, Some(StringFormat::Email));

    assert_eq!(field(&l, "orders", "customer_id").ty, FieldType::Uuid);
    assert_eq!(field(&l, "orders", "placed_on").ty, FieldType::Date);
    assert_eq!(field(&l, "orders", "placed_at").ty, FieldType::Timestamp);
    assert_eq!(field(&l, "orders", "gift").deprecated.as_deref(), Some(""));
    let score = field(&l, "orders", "score");
    assert_eq!(
        (score.ty, score.min, score.max),
        (FieldType::Float, Some(0.5), Some(9.5))
    );

    // It is a real, enforceable contract.
    CompiledContract::compile(c).expect("compiles");
    Contract::parse(ORDERS, "orders.odcs.yaml").expect("parse accepts it: nothing to refuse");
}

fn one_property(prop: &str) -> String {
    format!(
        "apiVersion: v3.1.0\nkind: DataContract\nid: t\nversion: 1.0.0\nschema:\n  - name: t\n    properties:\n{}",
        prop.lines()
            .map(|l| format!("      {l}\n"))
            .collect::<String>()
    )
}

#[test]
fn zero_tolerance_library_metrics_become_rules() {
    let l = load(&one_property(
        r#"- name: a
  logicalType: string
  quality:
    - metric: nullValues
      mustBe: 0
    - metric: duplicateValues
      mustBeLessThan: 1
    - metric: invalidValues
      mustBeLessOrEqualTo: 0
      unit: percent
      arguments:
        validValues: [x, y, null]
- name: b
  logicalType: string
  quality:
    - type: library
      metric: invalidValues
      mustBe: 0
      arguments:
        pattern: "^[A-Z]{2}$"
    - metric: missingValues
      mustBe: 0
      arguments:
        missingValues: [null, ""]"#,
    ));
    assert!(l.unenforced.is_empty(), "unexpected: {:?}", l.unenforced);
    let a = field(&l, "t", "a");
    assert!(a.required && !a.nullable, "nullValues: 0 means no nulls");
    assert!(a.unique, "duplicateValues < 1 row means none");
    // A null among valid values leaves the property's nullability alone
    // (nullValues already made it not null).
    assert_eq!(
        a.allowed,
        Some(vec![serde_json::json!("x"), serde_json::json!("y")])
    );
    assert!(!a.nullable);
    let b = field(&l, "t", "b");
    assert_eq!(b.pattern.as_deref(), Some("^[A-Z]{2}$"));
    assert!(b.required && !b.nullable, "null counts as missing");
    assert_eq!(b.min_length, Some(1), "\"\" counts as missing");
}

#[test]
fn must_be_between_is_refused_where_the_standard_contradicts_itself() {
    let rule = |bounds: &str| {
        load(&one_property(&format!(
            "- name: a\n  logicalType: string\n  quality:\n    - metric: nullValues\n      mustBeBetween: {bounds}"
        )))
    };
    // [0, 0]: "no nulls" with the bounds included, unsatisfiable with them
    // excluded. The standard says both, so it is refused, with the fix.
    let zero = rule("[0, 0]");
    assert_eq!(zero.unenforced.len(), 1);
    let why = &zero.unenforced[0].reason;
    assert!(
        why.contains("the standard says both") && why.contains("write mustBe: 0"),
        "{why}"
    );
    // [-1, 1]: zero with the bounds excluded, but one null allowed with them included.
    assert_eq!(rule("[-1, 1]").unenforced.len(), 1);
    // A real threshold is refused as one.
    let threshold = rule("[0, 100]");
    assert!(
        threshold.unenforced[0]
            .reason
            .starts_with("threshold rules"),
        "{:?}",
        threshold.unenforced
    );
    // Where both readings mean zero, it is enforced.
    let both = rule("[-1, 0.5]");
    assert!(both.unenforced.is_empty(), "{:?}", both.unenforced);
    assert!(!field(&both, "t", "a").nullable);
}

#[test]
fn rules_it_cannot_enforce_are_listed_with_their_paths() {
    let l = load(
        r#"
apiVersion: v3.2.0
kind: DataContract
id: t
version: 1.0.0
schema:
  - name: t
    relationships:
      - type: foreignKey
        from: t.a
        to: other.id
    quality:
      - metric: rowCount
        mustBeGreaterThan: 100
      - type: sql
        query: SELECT COUNT(*) FROM t
        mustBeLessThan: 3600
    properties:
      - name: a
        logicalType: string
        quality:
          - metric: nullValues
            mustBeLessThan: 5
            unit: percent
          - type: text
            description: values must be plausible
          - type: custom
            engine: someengine
      - name: b
        logicalType: integer
        logicalTypeOptions:
          multipleOf: 5
          format: i64
      - name: c
        logicalType: number
        logicalTypeOptions:
          exclusiveMinimum: 0
      - name: d
        logicalType: array
      - name: e
        logicalType: time
      - name: f
        logicalType: timestamp
        logicalTypeOptions:
          format: "yyyy-MM-dd HH:mm:ss"
      - name: g
        logicalType: string
        logicalTypeOptions:
          format: hostname
"#,
    );
    let got: Vec<(&str, &str)> = l
        .unenforced
        .iter()
        .map(|r| (r.path.as_str(), r.rule.as_str()))
        .collect();
    let want = [
        ("schema.t.relationships[0]", "relationship"),
        ("schema.t.properties.a.quality[0]", "library nullValues"),
        ("schema.t.properties.a.quality[1]", "text"),
        ("schema.t.properties.a.quality[2]", "custom"),
        ("schema.t.properties.b.logicalTypeOptions.format", "format"),
        (
            "schema.t.properties.b.logicalTypeOptions.multipleOf",
            "multipleOf",
        ),
        (
            "schema.t.properties.c.logicalTypeOptions.exclusiveMinimum",
            "exclusiveMinimum",
        ),
        ("schema.t.properties.d", "logicalType"),
        ("schema.t.properties.e", "logicalType"),
        ("schema.t.properties.f", "timestamp format"),
        ("schema.t.properties.g.logicalTypeOptions.format", "format"),
        ("schema.t.quality[0]", "library rowCount"),
        ("schema.t.quality[1]", "sql"),
    ];
    for w in want {
        assert!(got.contains(&w), "missing {w:?} in {got:?}");
    }
    assert_eq!(got.len(), want.len(), "unexpected extra entries: {got:?}");

    // Fields whose type cannot be checked are left out; a timestamp with a
    // custom format is left out too, since the RFC 3339 check would fail
    // conforming values.
    let fields = &l.contract.models["t"].fields;
    for gone in ["d", "e", "f"] {
        assert!(!fields.contains_key(gone), "{gone} should not be checked");
    }
    // The rest of a property stays enforced when one of its rules is not.
    assert!(fields.contains_key("a") && fields.contains_key("b"));
}

#[test]
fn a_composite_primary_key_checks_each_part_but_refuses_tuple_uniqueness() {
    let l = load(&one_property(
        "- name: tenant\n  logicalType: string\n  primaryKey: true\n  required: false\n- name: id\n  logicalType: integer\n  primaryKey: true",
    ));
    assert_eq!(unenforced_paths(&l), vec!["schema.t"]);
    assert!(l.unenforced[0].reason.contains("tenant, id"));
    for part in ["tenant", "id"] {
        let f = field(&l, "t", part);
        assert!(f.required && !f.nullable && !f.unique, "{part}");
    }
    // Overriding the author's `required: false` is said out loud.
    assert_eq!(l.notes.len(), 1);
    assert_eq!(l.notes[0].path, "schema.t.properties.tenant");
}

#[test]
fn v3_0_exclusive_flags_map_exactly_too() {
    // v3.0.x makes `minimum`/`maximum` exclusive with a boolean flag.
    let l = load(&one_property(
        "- name: i\n  logicalType: integer\n  logicalTypeOptions:\n    minimum: 0\n    exclusiveMinimum: true\n    maximum: 10\n    exclusiveMaximum: true\n\
         - name: j\n  logicalType: integer\n  logicalTypeOptions:\n    maximum: 10\n    exclusiveMaximum: false\n    exclusiveMinimum: true\n\
         - name: x\n  logicalType: number\n  logicalTypeOptions:\n    maximum: 1.5\n    exclusiveMaximum: true",
    ));
    let i = field(&l, "t", "i");
    assert_eq!((i.min, i.max), (Some(1.0), Some(9.0)));
    // `false`, and `true` with no bound to modify, exclude nothing.
    let j = field(&l, "t", "j");
    assert_eq!((j.min, j.max), (None, Some(10.0)));
    // Numbers keep the inclusive bound and refuse the exclusive part.
    assert_eq!(field(&l, "t", "x").max, Some(1.5));
    assert_eq!(
        unenforced_paths(&l),
        vec!["schema.t.properties.x.logicalTypeOptions.exclusiveMaximum"]
    );
}

#[test]
fn schema_level_single_column_duplicates_become_unique() {
    let l = load(
        "apiVersion: v3.1.0\nkind: DataContract\nid: t\nversion: 1.0.0\nschema:\n  - name: t\n    properties:\n      - name: a\n        physicalName: col_a\n        logicalType: string\n    quality:\n      - metric: duplicateValues\n        mustBe: 0\n        arguments:\n          properties: [a]\n",
    );
    assert!(l.unenforced.is_empty(), "{:?}", l.unenforced);
    assert!(field(&l, "t", "col_a").unique);
}

#[test]
fn parse_refuses_what_load_reports() {
    let yaml = one_property("- name: a\n  logicalType: array");
    let err = Contract::parse(&yaml, "c.odcs.yaml")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("c.odcs.yaml: 1 contract rule(s) cannot be enforced yet"),
        "{err}"
    );
    assert!(err.contains("schema.t.properties.a (logicalType)"), "{err}");
    assert!(err.contains("--allow-unenforced"), "{err}");
    assert_eq!(load(&yaml).unenforced.len(), 1);
}

#[test]
fn only_odcs_v3_data_contracts_are_read() {
    let v2 = "apiVersion: v2.2.2\nkind: DataContract\nid: t\nversion: 1.0.0\n";
    let err = Contract::load(v2, "old.yaml").unwrap_err().to_string();
    assert!(err.contains("Covenant reads ODCS v3"), "{err}");
    let kind = "apiVersion: v3.2.0\nkind: DataProduct\nid: t\nversion: 1.0.0\n";
    let err = Contract::load(kind, "p.yaml").unwrap_err().to_string();
    assert!(err.contains("is not DataContract"), "{err}");
}

#[test]
fn a_revision_it_has_not_been_reviewed_against_is_refused_by_name() {
    let doc = one_property("- name: a\n  logicalType: string");
    for v in covenant::odcs::VERSIONS {
        let l = load(&doc.replace("v3.1.0", v));
        assert!(l.unenforced.is_empty(), "{v}: {:?}", l.unenforced);
    }
    let l = load(&doc.replace("v3.1.0", "v3.3.0"));
    assert_eq!(unenforced_paths(&l), vec!["apiVersion"]);
    assert_eq!(l.unenforced[0].rule, "apiVersion v3.3.0");
    assert!(
        l.unenforced[0]
            .reason
            .ends_with("a rule v3.3.0 introduced would go unchecked"),
        "{:?}",
        l.unenforced[0]
    );
    // Read all the same, for a run that allows partial enforcement.
    assert_eq!(field(&l, "t", "a").ty, FieldType::String);
    assert_eq!(
        l.format,
        SourceFormat::Odcs {
            api_version: "v3.3.0".into()
        }
    );
}

#[test]
fn the_reviewed_revisions_are_the_documented_ones() {
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/ODCS.md"))
        .unwrap();
    let section = doc
        .split("\n## Revisions\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("docs/ODCS.md has a Revisions section");
    let documented: Vec<&str> = section
        .lines()
        .filter_map(|l| l.strip_prefix("| v"))
        .filter_map(|l| l.split(' ').next())
        .collect();
    let listed: Vec<String> = covenant::odcs::VERSIONS
        .iter()
        .map(|v| v.trim_start_matches('v').to_string())
        .collect();
    assert_eq!(documented, listed);
}

#[test]
fn a_measure_is_not_a_column_and_a_rule_on_one_is_refused() {
    let l = load(&one_property(
        r#"- name: region
  semanticType: dimension
  logicalType: string
  required: true
- name: revenue
  semanticType: measure
  logicalType: number
  transformLogic: SUM(amount)
- name: orders
  semanticType: measure
  logicalType: integer
  transformLogic: COUNT(*)
  logicalTypeOptions:
    minimum: 1
- name: kind
  semanticType: metric
  logicalType: string"#,
    ));
    let fields = &l.contract.models["t"].fields;
    assert_eq!(fields.keys().collect::<Vec<_>>(), vec!["region"]);
    assert!(fields["region"].required);
    assert_eq!(
        unenforced_paths(&l),
        vec!["schema.t.properties.orders", "schema.t.properties.kind"]
    );
    assert_eq!(l.unenforced[0].rule, "semanticType measure");
    assert!(l.unenforced[1]
        .reason
        .contains("unknown semanticType \"metric\""));
    // The bare measure is left out openly, as a note.
    assert_eq!(l.notes.len(), 1);
    assert_eq!(l.notes[0].path, "schema.t.properties.revenue");
    assert!(l.notes[0].message.starts_with("a measure"), "{:?}", l.notes);
}

#[test]
fn a_variable_is_refused_wherever_it_would_change_a_rule() {
    let l = load(&one_property(
        r#"- name: code
  logicalType: string
  logicalTypeOptions:
    pattern: "^${PREFIX}-[0-9]+$"
- name: region
  logicalType: string
  enum: [eu, "${EXTRA_REGION:-us}"]
- name: amount
  physicalName: ${AMOUNT_COLUMN}
  logicalType: integer
- name: status
  logicalType: string
  quality:
    - metric: invalidValues
      arguments:
        validValues: [open, "${CLOSED}"]
      mustBe: 0
- name: sku
  logicalType: string
  description: "Stock unit, from ${CATALOG}."
  logicalTypeOptions:
    pattern: "^[$]{2}x|\\$\\{B\\}|${9}$"
- name: note
  logicalType: string
  logicalTypeOptions:
    pattern: "^a\\${B}$""#,
    ));
    assert_eq!(
        unenforced_paths(&l),
        vec![
            "schema.t.properties.code.logicalTypeOptions.pattern",
            "schema.t.properties.region.enum",
            "schema.t.properties.amount",
            "schema.t.properties.status.quality[0]",
            // The standard defines no escape, so an escaped `$` does not
            // stop a reference.
            "schema.t.properties.note.logicalTypeOptions.pattern",
        ]
    );
    assert!(
        l.unenforced[1].reason.contains(
            "holds the variable ${EXTRA_REGION:-us}, which Covenant does not resolve yet"
        ),
        "{:?}",
        l.unenforced[1]
    );
    assert_eq!(l.unenforced[2].rule, "physicalName");
    let fields = &l.contract.models["t"].fields;
    // The rest of each property is still checked: its type, and no pattern
    // or allowed set taken from the literal text.
    assert_eq!(fields["code"].ty, FieldType::String);
    assert_eq!(fields["code"].pattern, None);
    assert_eq!(fields["region"].allowed, None);
    assert_eq!(fields["status"].allowed, None);
    assert!(!fields.contains_key("amount") && !fields.contains_key("${AMOUNT_COLUMN}"));
    // Not variables: a description Covenant does not enforce, and regex
    // text that only looks like one.
    assert_eq!(
        fields["sku"].pattern.as_deref(),
        Some("^[$]{2}x|\\$\\{B\\}|${9}$")
    );
}

#[test]
fn a_null_enum_entry_cannot_relax_required_or_a_primary_key() {
    let l = load(&one_property(
        r#"- name: a
  logicalType: string
  required: true
  enum: [x, null]
- name: k
  logicalType: string
  primaryKey: true
  enum: [{ value: y }, { value: null }]
- name: o
  logicalType: string
  enum: [z, null]"#,
    ));
    let (a, k, o) = (
        field(&l, "t", "a"),
        field(&l, "t", "k"),
        field(&l, "t", "o"),
    );
    assert!(a.required && !a.nullable);
    assert!(k.unique && !k.nullable);
    assert!(o.nullable, "an optional property stays optional");
    assert_eq!(a.allowed, Some(vec![serde_json::json!("x")]));
    assert!(l.unenforced.is_empty(), "{:?}", l.unenforced);

    // End to end: the null is refused.
    let compiled = CompiledContract::compile(&l.contract).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let mut out = Vec::new();
    let ok = covenant::engine::row::validate_record(
        model,
        &serde_json::json!({ "a": null, "k": "y" }),
        0,
        None,
        &mut out,
    );
    assert!(!ok && out.len() == 1, "{out:?}");
}

#[test]
fn native_contracts_are_unaffected() {
    let native = "covenant: 1\nid: t\nversion: 1.0.0\nmodels:\n  t:\n    fields:\n      a: { type: string }\n";
    let l = load(native);
    assert_eq!(l.format, SourceFormat::Covenant);
    assert!(l.unenforced.is_empty() && l.notes.is_empty());
}

// --- the official Bitol examples ------------------------------------------------

fn bitol_fixtures() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("fixture dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.to_string_lossy().ends_with(".odcs.yaml") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/odcs/bitol"),
        &mut out,
    );
    out.sort();
    out
}

#[test]
fn every_official_example_loads_and_accounts_for_every_rule() {
    let fixtures = bitol_fixtures();
    assert!(
        fixtures.len() >= 40,
        "found only {} fixtures",
        fixtures.len()
    );
    let mut fully_enforced = 0;
    for path in &fixtures {
        let l = Contract::load_path(path)
            .unwrap_or_else(|e| panic!("{} does not load: {e}", path.display()));
        assert!(
            matches!(l.format, SourceFormat::Odcs { .. }),
            "{}",
            path.display()
        );
        for r in &l.unenforced {
            assert!(
                r.path.starts_with("schema."),
                "{}: unenforced rule outside the schema: {r:?}",
                path.display()
            );
            assert!(!r.reason.is_empty(), "{}: {r:?}", path.display());
        }
        if l.unenforced.is_empty() && !l.contract.models.is_empty() {
            fully_enforced += 1;
        }
    }
    assert!(
        fully_enforced > 0,
        "no official example is fully enforceable"
    );
}

#[test]
fn the_official_completeness_example_is_fully_enforced() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/odcs/bitol/quality/column-completeness.odcs.yaml");
    let l = Contract::load_path(&path).expect("loads");
    assert!(l.unenforced.is_empty(), "{:?}", l.unenforced);
    let f = field(&l, "Air_Quality", "UniqueID");
    assert_eq!(f.ty, FieldType::Float);
    assert!(f.unique && f.required && !f.nullable);
}

#[test]
fn the_official_validity_example_threshold_is_refused_not_dropped() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/odcs/bitol/quality/column-validity.odcs.yaml");
    let l = Contract::load_path(&path).expect("loads");
    assert_eq!(
        unenforced_paths(&l),
        vec!["schema.Air_Quality.properties.air_quality_status.quality[0]"]
    );
}

// --- the CLI --------------------------------------------------------------------

const ODCS_CONTRACT: &str = r#"apiVersion: v3.2.0
kind: DataContract
id: orders
version: 1.0.0
team: { name: data@acme.io }
schema:
  - name: orders
    properties:
      - name: order_id
        primaryKey: true
        logicalType: string
      - name: amount
        logicalType: integer
        required: true
        logicalTypeOptions: { minimum: 0 }
      - name: currency
        logicalType: string
        enum: [{ value: USD }, { value: EUR }]
"#;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(BIN).args(args).output().expect("binary runs")
}

fn tmp(dir: &tempfile::TempDir, name: &str, body: &str) -> String {
    let p = dir.path().join(name);
    std::fs::write(&p, body).unwrap();
    p.display().to_string()
}

#[test]
fn check_enforces_an_odcs_contract_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let c = tmp(&dir, "orders.odcs.yaml", ODCS_CONTRACT);
    let clean = tmp(
        &dir,
        "clean.ndjson",
        "{\"order_id\":\"a\",\"amount\":5,\"currency\":\"USD\"}\n{\"order_id\":\"b\",\"amount\":0,\"currency\":\"EUR\"}\n",
    );
    let dirty = tmp(
        &dir,
        "dirty.ndjson",
        "{\"order_id\":\"a\",\"amount\":-1,\"currency\":\"GBP\"}\n{\"order_id\":\"a\",\"amount\":1,\"currency\":\"USD\"}\n",
    );
    let ok = run(&["check", &clean, "-c", &c]);
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ok.stdout)
    );
    let bad = run(&["check", &dirty, "-c", &c]);
    assert_eq!(bad.status.code(), Some(1));
    let out = String::from_utf8_lossy(&bad.stdout);
    for rule in ["min", "allowed", "unique"] {
        assert!(out.contains(rule), "missing {rule} in:\n{out}");
    }
}

#[test]
fn unenforced_rules_refuse_the_run_unless_allowed_and_then_mark_it_partial() {
    let dir = tempfile::tempdir().unwrap();
    let c = tmp(
        &dir,
        "c.odcs.yaml",
        &format!(
            "{ODCS_CONTRACT}    quality:\n      - metric: rowCount\n        mustBeGreaterThan: 1\n"
        ),
    );
    let data = tmp(
        &dir,
        "d.ndjson",
        "{\"order_id\":\"a\",\"amount\":5,\"currency\":\"USD\"}\n",
    );

    let refused = run(&["check", &data, "-c", &c]);
    assert_eq!(
        refused.status.code(),
        Some(2),
        "unenforceable contract is a run failure"
    );
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(
        err.contains("schema.orders.quality[0] (library rowCount)"),
        "{err}"
    );

    let partial = run(&[
        "check",
        &data,
        "-c",
        &c,
        "--allow-unenforced",
        "--format",
        "json",
    ]);
    assert_eq!(partial.status.code(), Some(0));
    let reports: serde_json::Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(reports[0]["unenforced"][0]["rule"], "library rowCount");
    assert!(String::from_utf8_lossy(&partial.stderr).contains("not enforced"));

    let human = run(&["--allow-unenforced", "check", &data, "-c", &c]);
    let out = String::from_utf8_lossy(&human.stdout);
    assert!(out.starts_with("PASS (partial)"), "{out}");

    // validate: an error without the flag, a warning with it.
    assert_eq!(run(&["validate", &c]).status.code(), Some(1));
    assert_eq!(
        run(&["validate", &c, "--allow-unenforced"]).status.code(),
        Some(0)
    );
}

#[test]
fn diff_classifies_changes_between_odcs_versions() {
    let dir = tempfile::tempdir().unwrap();
    let old = tmp(&dir, "old.odcs.yaml", ODCS_CONTRACT);
    // Removing a property consumers may read is breaking.
    let removed = ODCS_CONTRACT.replace("version: 1.0.0", "version: 2.0.0").replace(
        "      - name: currency\n        logicalType: string\n        enum: [{ value: USD }, { value: EUR }]\n",
        "",
    );
    assert_ne!(
        removed,
        ODCS_CONTRACT.replace("version: 1.0.0", "version: 2.0.0")
    );
    let new = tmp(&dir, "new.odcs.yaml", &removed);
    let out = run(&["diff", &old, &new, "--format", "json"]);
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let changes = report["changes"].as_array().expect("changes");
    assert!(
        changes
            .iter()
            .any(|c| c["path"] == "models.orders.fields.currency" && c["severity"] == "breaking"),
        "{report:#}"
    );
}

#[test]
fn a_partial_diff_names_every_rule_it_did_not_compare() {
    let dir = tempfile::tempdir().unwrap();
    let row_count = "    quality:\n      - metric: rowCount\n        mustBeGreaterThan: 1\n";
    let old = tmp(
        &dir,
        "old.odcs.yaml",
        &format!("{ODCS_CONTRACT}{row_count}"),
    );
    // The new version keeps the rowCount rule and adds a sql one: two rules the
    // diff cannot compare, each named once.
    let new_body = format!(
        "{}{row_count}      - type: sql\n        query: SELECT COUNT(*) FROM orders\n        mustBe: 0\n",
        ODCS_CONTRACT.replace("version: 1.0.0", "version: 1.1.0")
    );
    let new = tmp(&dir, "new.odcs.yaml", &new_body);

    assert_eq!(
        run(&["diff", &old, &new]).status.code(),
        Some(2),
        "refused without the flag"
    );

    let out = run(&["diff", &old, &new, "--allow-unenforced", "--format", "json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rules: Vec<&str> = report["unenforced"]
        .as_array()
        .expect("a partial diff lists them")
        .iter()
        .map(|r| r["rule"].as_str().unwrap())
        .collect();
    assert_eq!(rules, vec!["library rowCount", "sql"]);

    let human = run(&["diff", &old, &new, "--allow-unenforced"]);
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(
        text.lines().next().unwrap().ends_with("(partial)"),
        "{text}"
    );
    assert!(
        text.contains("not compared (--allow-unenforced): 2 contract rule(s)"),
        "{text}"
    );
}
