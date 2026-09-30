//! `covenant export postgres`, held to its claim: a row the exported table
//! rejects is exactly a row the engines report, given the rows the table
//! holds, for values of the column types.
//!
//! The DDL's shape and refusals are checked here without a database. The
//! claim itself needs a real Postgres, so those tests are ignored by default
//! and run as
//!
//! ```text
//! COVENANT_TEST_POSTGRES=postgresql://… cargo test --test postgres_test -- --ignored
//! ```
//!
//! with `psql` on the PATH, against a UTF8 database. CI's `postgres` job
//! does exactly that.
// Arrow fixtures (Parquet files, RecordBatches) throughout: this suite runs
// in every build with the `arrow` feature, which is on by default.
#![cfg(feature = "arrow")]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field as ArrowField, Schema};
use covenant::compile::{shape, CompiledContract, CompiledModel};
use covenant::conformance::load_all;
use covenant::engine::row::validate_record;
use covenant::engine::UniqueTracker;
use covenant::error::CovenantError;
use covenant::postgres::{export, pg_regex, Options};
use covenant::spec::{Contract, FieldType};
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

fn demo() -> Contract {
    Contract::from_path(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/contracts/orders.yaml"),
    )
    .unwrap()
}

fn native(fields: &str) -> Contract {
    let yaml = format!(
        "covenant: 1\nid: t\nversion: 1.0.0\nmodels:\n  t:\n    fields:\n{}",
        fields
            .lines()
            .map(|l| format!("      {l}\n"))
            .collect::<String>()
    );
    Contract::parse(&yaml, "t.yaml").unwrap()
}

// --- the DDL, without a database -----------------------------------------------

#[test]
fn the_demo_contract_exports_in_full() {
    let sql = export(&demo(), &Options::default()).unwrap().sql;
    for line in [
        "-- Covenant contract orders v1.2.0, model orders, as a Postgres table.",
        "CREATE TABLE \"orders\" (",
        "  \"order_id\" text NOT NULL,",
        "  \"amount_cents\" bigint NOT NULL,",
        "  \"customer_email\" text,",
        "  \"created_at\" timestamptz NOT NULL,",
        "  CONSTRAINT \"order_id.pattern\" CHECK ((\"order_id\" COLLATE \"C\") ~ '^ord_[a-z0-9]{12}$'),",
        "  CONSTRAINT \"amount_cents.min\" CHECK (\"amount_cents\" >= 0),",
        "  CONSTRAINT \"amount_cents.max\" CHECK (\"amount_cents\" <= 5000000),",
        "  CONSTRAINT \"currency.allowed\" CHECK (\"currency\" IN ('USD', 'EUR', 'GBP')),",
        "  CONSTRAINT \"orders.order_id.unique\" UNIQUE (\"order_id\")",
        "COMMENT ON TABLE \"orders\" IS 'Covenant contract orders v1.2.0, model orders';",
    ] {
        assert!(sql.contains(line), "missing {line:?} in\n{sql}");
    }
    assert!(!sql.contains("PARTIAL"), "{sql}");
}

#[test]
fn a_rule_without_an_exact_equivalent_refuses_the_export_by_name() {
    let c = native(
        "code: { type: string, pattern: '^\\d{5}$' }\nat: { type: timestamp, unique: true }\nok: { type: integer, min: 1 }",
    );
    let err = export(&c, &Options::default()).unwrap_err();
    assert!(
        matches!(err, CovenantError::DialectUnenforced { count: 2, .. }),
        "{err}"
    );
    let text = err.to_string();
    assert!(
        text.contains("models.t.fields.code (pattern): \\d is Unicode-wide in Covenant"),
        "{text}"
    );
    assert!(
        text.contains("models.t.fields.at (unique): Covenant compares a timestamp's text"),
        "{text}"
    );

    let partial = export(
        &c,
        &Options {
            allow_unenforced: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(partial.unenforced.len(), 2);
    assert!(partial
        .sql
        .contains("-- PARTIAL: this table does not enforce these contract rules:"));
    assert!(partial
        .sql
        .contains("CONSTRAINT \"ok.min\" CHECK (\"ok\" >= 1)"));
    assert!(!partial.sql.contains("code.pattern") && !partial.sql.contains("UNIQUE"));
}

#[test]
fn names_postgres_would_truncate_are_refused_and_long_constraint_names_shortened() {
    let long = "x".repeat(64);
    let err = export(
        &native(&format!("{long}: {{ type: string }}")),
        &Options::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("cannot be a Postgres identifier"), "{err}");

    let err = export(
        &native("a: { type: string }"),
        &Options {
            table: Some("db.public.orders"),
            ..Options::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("expected table or schema.table"), "{err}");

    let name = "y".repeat(63);
    let sql = export(
        &native(&format!("{name}: {{ type: integer, min: 0 }}")),
        &Options::default(),
    )
    .unwrap()
    .sql;
    let constraint = sql
        .lines()
        .find_map(|l| l.trim().strip_prefix("CONSTRAINT \""))
        .and_then(|l| l.split('"').next())
        .unwrap();
    assert!(constraint.len() <= 63, "{constraint}");
    assert!(
        constraint.starts_with("yyyy") && constraint.contains('~'),
        "{constraint}"
    );
}

#[test]
fn the_cli_exports_to_a_file_and_refuses_with_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let contract = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/contracts/orders.yaml");
    let out = dir.path().join("orders.sql");
    let run = Command::new(BIN)
        .args(["export", "postgres", "--table", "analytics.orders", "-c"])
        .arg(&contract)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(
        run.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let sql = std::fs::read_to_string(&out).unwrap();
    assert!(
        sql.contains("CREATE TABLE \"analytics\".\"orders\" ("),
        "{sql}"
    );

    let odcs = dir.path().join("c.odcs.yaml");
    std::fs::write(
        &odcs,
        "apiVersion: v3.1.0\nkind: DataContract\nid: c\nversion: 1.0.0\nschema:\n  - name: t\n    \
         properties:\n      - name: a\n        logicalType: string\n        logicalTypeOptions:\n          \
         pattern: '\\w+'\n      - name: b\n        logicalType: array\n",
    )
    .unwrap();
    let refused = Command::new(BIN)
        .args(["export", "postgres", "-c"])
        .arg(&odcs)
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("schema.t.properties.b"));

    let partial = Command::new(BIN)
        .args(["export", "postgres", "--allow-unenforced", "-c"])
        .arg(&odcs)
        .output()
        .unwrap();
    assert_eq!(partial.status.code(), Some(0));
    let sql = String::from_utf8_lossy(&partial.stdout);
    // The reader's rule and the dialect's, both listed.
    assert!(
        sql.contains("--   schema.t.properties.b (logicalType)"),
        "{sql}"
    );
    assert!(sql.contains("--   models.t.fields.a (pattern)"), "{sql}");
    assert!(String::from_utf8_lossy(&partial.stderr)
        .contains("1 contract rule(s) have no exact equivalent"));
}

// --- the claim, against a real Postgres ---------------------------------------

/// Run a script through psql and return its tab-separated output rows.
fn psql(script: &str) -> Vec<Vec<String>> {
    let url = std::env::var("COVENANT_TEST_POSTGRES")
        .expect("set COVENANT_TEST_POSTGRES to the Postgres to test against");
    let mut child = Command::new("psql")
        .args([
            "-X",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "-A",
            "-t",
            "-F",
            "\t",
            "-f",
            "-",
        ])
        .arg(&url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("psql on the PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "psql failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sql_float(f: f64) -> String {
    let text = if f.is_nan() {
        "NaN".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
    } else {
        f.to_string()
    };
    format!("'{text}'::float8")
}

/// A JSON value as a literal of the column's type, or `None` when it is not
/// a value of that type — how such a value reaches a table is the loader's
/// business, not the table's.
fn literal(ty: FieldType, v: &Value) -> Option<String> {
    if v.is_null() {
        return Some("NULL".into());
    }
    match ty {
        FieldType::String | FieldType::Uuid => v.as_str().map(sql_text),
        FieldType::Integer => v.as_i64().map(|n| n.to_string()),
        FieldType::Float => v.as_f64().map(sql_float),
        FieldType::Boolean => v.as_bool().map(|b| b.to_string()),
        FieldType::Date => v
            .as_str()
            .filter(|s| shape::is_date(s))
            .map(|s| format!("{}::date", sql_text(s))),
        FieldType::Timestamp => v
            .as_str()
            .filter(|s| shape::is_timestamp(s))
            .map(|s| format!("{}::timestamptz", sql_text(s))),
    }
}

const NOT_OF_TYPE: &str = "a value not of its column's type";
/// A table has no absent value, so these two are its columnar reading of
/// nulls (ARCHITECTURE.md's null-semantics table), not a rule it breaks.
const ABSENT_REQUIRED: &str = "a required, nullable field's key is absent: in a table, a null";
const NULL_OPTIONAL: &str = "an optional field that forbids null holds one: in a table, absent";

/// The INSERT that loads `record`, or why a table cannot hold it as it is.
/// Undeclared keys go in for a strict model (Postgres refuses the unknown
/// column, as Covenant refuses the key) and are dropped otherwise.
fn insert(table: &str, model: &CompiledModel, record: &Value) -> Result<String, &'static str> {
    let obj = record.as_object().ok_or(NOT_OF_TYPE)?;
    for f in &model.fields {
        match obj.get(&f.name) {
            None if f.required && f.nullable => return Err(ABSENT_REQUIRED),
            Some(Value::Null) if !f.required && !f.nullable => return Err(NULL_OPTIONAL),
            _ => {}
        }
    }
    let (mut cols, mut vals) = (Vec::new(), Vec::new());
    for (key, value) in obj {
        let quoted = format!("\"{}\"", key.replace('"', "\"\""));
        match model.field_index.get(key) {
            Some(&i) => {
                vals.push(literal(model.fields[i].ty, value).ok_or(NOT_OF_TYPE)?);
                cols.push(quoted);
            }
            None if model.strict => {
                vals.push("NULL".into());
                cols.push(quoted);
            }
            None => {}
        }
    }
    Ok(if cols.is_empty() {
        format!("INSERT INTO {table} DEFAULT VALUES")
    } else {
        format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            cols.join(", "),
            vals.join(", ")
        )
    })
}

/// The engines' verdict on each record as a write path sees it: a record is
/// accepted when the row engine reports nothing, given the records accepted
/// before it — a rejected record does not count toward uniqueness.
fn write_path(model: &CompiledModel, records: &[Value]) -> Vec<(bool, Vec<String>)> {
    let mut accepted: Vec<&Value> = Vec::new();
    let mut verdicts = Vec::new();
    for (i, record) in records.iter().enumerate() {
        let mut tracker = UniqueTracker::new(model);
        for a in &accepted {
            validate_record(model, a, 0, Some(&mut tracker), &mut Vec::new());
        }
        let mut out = Vec::new();
        let ok = validate_record(model, record, i as u64, Some(&mut tracker), &mut out);
        if ok {
            accepted.push(record);
        }
        verdicts.push((ok, out.into_iter().map(|v| v.message).collect()));
    }
    verdicts
}

/// One table's worth of the claim: create it, load `records` one at a time,
/// and pair Postgres's verdict on each loadable record with the engines'.
struct Trial {
    label: String,
    ddl: String,
    records: Vec<(usize, String, bool, Vec<String>)>,
    /// Records a table cannot hold as they are, and why.
    skipped: Vec<&'static str>,
}

impl Trial {
    fn new(label: &str, table: &str, contract: &Contract, records: &[Value]) -> Trial {
        let ddl = export(
            contract,
            &Options {
                table: Some(table),
                ..Options::default()
            },
        )
        .unwrap_or_else(|e| panic!("{label}: {e}"))
        .sql;
        let compiled = CompiledContract::compile(contract).unwrap();
        let model = compiled.resolve_model(None).unwrap();
        let verdicts = write_path(model, records);
        let mut trial = Trial {
            label: label.to_string(),
            ddl,
            records: Vec::new(),
            skipped: Vec::new(),
        };
        for (i, (r, (ok, why))) in records.iter().zip(verdicts).enumerate() {
            match insert(table, model, r) {
                Ok(sql) => trial.records.push((i, sql, ok, why)),
                Err(reason) => trial.skipped.push(reason),
            }
        }
        trial
    }
}

/// Run every trial in one session and return the disagreements.
fn disagreements(schema: &str, trials: &[Trial]) -> (usize, Vec<String>) {
    let mut script = format!(
        "SET client_min_messages = warning;\nDROP SCHEMA IF EXISTS {schema} CASCADE;\nCREATE SCHEMA {schema};\n\
         CREATE TEMP TABLE results (trial int, rec int, ok boolean, why text);\n"
    );
    for (t, trial) in trials.iter().enumerate() {
        script.push_str(&trial.ddl);
        script.push_str("DO $trial$ BEGIN\n");
        for (i, sql, _, _) in &trial.records {
            script.push_str(&format!(
                "  BEGIN {sql}; INSERT INTO results VALUES ({t}, {i}, true, NULL);\n  \
                 EXCEPTION WHEN others THEN INSERT INTO results VALUES ({t}, {i}, false, SQLERRM); END;\n"
            ));
        }
        script.push_str("END $trial$;\n");
    }
    script.push_str("SELECT trial, rec, ok, coalesce(why, '') FROM results ORDER BY trial, rec;\n");
    script.push_str(&format!("DROP SCHEMA {schema} CASCADE;\n"));
    let rows = psql(&script);
    let submitted: usize = trials.iter().map(|t| t.records.len()).sum();
    assert_eq!(
        rows.len(),
        submitted,
        "Postgres did not report on every record"
    );
    let mut compared = 0;
    let mut wrong = Vec::new();
    for row in rows {
        let (t, i): (usize, usize) = (row[0].parse().unwrap(), row[1].parse().unwrap());
        let pg_ok = row[2] == "t";
        let trial = &trials[t];
        let (_, sql, ok, why) = trial.records.iter().find(|r| r.0 == i).unwrap();
        compared += 1;
        if pg_ok != *ok {
            wrong.push(format!(
                "{} record {i}: Postgres {} ({}), the engines {} ({})\n    {sql}",
                trial.label,
                if pg_ok { "accepts" } else { "rejects" },
                row[3],
                if *ok { "accept" } else { "reject" },
                why.join("; ")
            ));
        }
    }
    (compared, wrong)
}

#[test]
#[ignore = "needs COVENANT_TEST_POSTGRES and psql"]
fn postgres_rejects_the_rows_the_engines_report_on_every_conformance_vector() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/odcs");
    let mut trials = Vec::new();
    let mut unsupported = Vec::new();
    for (n, (_, v)) in load_all(&[suite]).unwrap().into_iter().enumerate() {
        let loaded = Contract::load(&v.contract, &v.id).unwrap();
        if !loaded.unenforced.is_empty() {
            unsupported.push(v.id.clone());
            continue;
        }
        trials.push(Trial::new(
            &v.id,
            &format!("covenant_vectors.t{n}"),
            &loaded.contract,
            &v.records,
        ));
    }
    // Covenant cannot enforce these at all (tests/conformance_test.rs).
    assert_eq!(unsupported.len(), 6, "{unsupported:?}");
    // ODCS fields are required and not nullable, or neither, so every
    // record left out is one whose value is not of its column's type.
    for t in &trials {
        assert!(
            t.skipped.iter().all(|&r| r == NOT_OF_TYPE),
            "{}: {:?}",
            t.label,
            t.skipped
        );
    }
    let (compared, wrong) = disagreements("covenant_vectors", &trials);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // Records a table cannot hold as they are (a string in an integer
    // column, a date Covenant rejects as text) are the loader's business.
    assert!(compared >= 90, "only {compared} records compared");
}

#[test]
#[ignore = "needs COVENANT_TEST_POSTGRES and psql"]
fn postgres_agrees_with_the_engines_on_edge_values() {
    let contract = native(
        r#"i: { type: integer, min: 0.5, max: 9007199254740993 }
f: { type: float, min: 0, max: 10 }
fa: { type: float, allowed: [0, 1.5, 0.1] }
s: { type: string, min_length: 2, max_length: 3 }
p: { type: string, pattern: "^a.b$|日|^x+?\\.y{2,3}$" }
e: { type: string, format: email }
u: { type: string, format: uri }
id: { type: uuid }
ids: { type: uuid, allowed: ["6f1e0d3a-8c2b-4a5d-9e7f-0123456789ab"] }
d: { type: date, allowed: ["2024-02-29"] }
b: { type: boolean, allowed: [true] }
k: { type: string, unique: true }
n: { type: integer, required: true, nullable: true }
m: { type: string, nullable: false }"#,
    );
    let mut records = vec![json!({ "n": null })];
    let mut add = |field: &str, values: Vec<Value>| {
        for v in values {
            records.push(json!({ "n": 1, field: v }));
        }
    };
    add(
        "i",
        vec![
            json!(0),
            json!(1),
            json!(9007199254740992_i64),
            json!(9007199254740993_i64),
            json!(i64::MAX),
            json!(i64::MIN),
        ],
    );
    add(
        "f",
        vec![
            json!(0.0),
            json!(-0.0),
            json!(10.0),
            json!(10.000000000000002),
            json!(-1e-300),
            json!(3),
        ],
    );
    add(
        "fa",
        vec![
            json!(0),
            json!(-0.0),
            json!(1.5),
            json!(0.1),
            json!(0.30000000000000004),
        ],
    );
    add(
        "s",
        vec![
            json!("a"),
            json!("ab"),
            json!("日本"),
            json!("e\u{301}"),
            json!("👍👍👍"),
            json!("abcd"),
        ],
    );
    add(
        "p",
        vec![
            json!("a\nb"),
            json!("a\rb"),
            json!("axb"),
            json!("x日x"),
            json!("xxx.yy"),
            json!("x.yyyy"),
            json!("ab"),
        ],
    );
    add(
        "e",
        vec![
            json!("a@b.c"),
            json!("@b.c"),
            json!("a@.c"),
            json!("a@b."),
            json!("a@@b.c"),
            json!("a@b"),
            json!("a b@c.d"),
            json!("a\u{a0}@c.d"),
            json!("a@c.d\u{3000}"),
            json!("a@c.d\u{feff}"),
            json!("ü@ß.é"),
        ],
    );
    add(
        "u",
        vec![
            json!("mailto:x"),
            json!("1a:b"),
            json!("a:"),
            json!("a+b-c.d:x:y"),
            json!("a:b c"),
            json!(":x"),
            json!("a\u{85}:x"),
        ],
    );
    add(
        "id",
        vec![
            json!("6F1E0D3A-8C2B-4A5D-9E7F-0123456789AB"),
            json!("6f1e0d3a8c2b4a5d9e7f0123456789ab"),
            json!("{6f1e0d3a-8c2b-4a5d-9e7f-0123456789ab}"),
            json!("6f1e0d3a-8c2b-4a5d-9e7f-0123456789ag"),
        ],
    );
    add(
        "ids",
        vec![
            json!("6f1e0d3a-8c2b-4a5d-9e7f-0123456789ab"),
            json!("6F1E0D3A-8C2B-4A5D-9E7F-0123456789AB"),
        ],
    );
    add(
        "d",
        vec![
            json!("2024-02-29"),
            json!("2024-01-01"),
            json!("2024-03-01"),
        ],
    );
    add("b", vec![json!(true), json!(false)]);
    add("m", vec![json!(null), json!("x")]);
    // Uniqueness over the rows the table keeps: a rejected row's value is
    // free for a later one.
    records.push(json!({ "n": 1, "k": "dup", "s": "a" }));
    records.push(json!({ "n": 1, "k": "dup" }));
    records.push(json!({ "n": 1, "k": "dup" }));
    records.push(json!({ "n": 1, "k": "Dup" }));
    records.push(json!({}));

    let trial = Trial::new("edges", "covenant_edges.t", &contract, &records);
    // Every edge value is of its column's type; the two records left out
    // are the columnar reading of nulls, which the table shares with the
    // Arrow engine and not with the row engine.
    let mut skipped = trial.skipped.clone();
    skipped.sort_unstable();
    assert_eq!(skipped, vec![ABSENT_REQUIRED, NULL_OPTIONAL]);
    let (compared, wrong) = disagreements("covenant_edges", &[trial]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(compared, records.len() - 2);
}

#[test]
#[ignore = "needs COVENANT_TEST_POSTGRES and psql"]
fn postgres_agrees_with_the_arrow_engine_on_values_json_cannot_carry() {
    let contract = native("f: { type: float, min: 0, max: 10, unique: true }");
    let compiled = CompiledContract::compile(&contract).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let values = [
        f64::NAN,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.0,
        0.0,
        5.0,
    ];
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![ArrowField::new(
            "f",
            DataType::Float64,
            true,
        )])),
        vec![Arc::new(Float64Array::from(values.to_vec()))],
    )
    .unwrap();
    // One row per batch, uniqueness over the rows accepted so far.
    let mut accepted: Vec<f64> = Vec::new();
    let mut engine = Vec::new();
    for (i, v) in values.iter().enumerate() {
        let mut tracker = UniqueTracker::new(model);
        let mut prior = covenant::report::Collector::new(usize::MAX);
        if !accepted.is_empty() {
            let prev = RecordBatch::try_new(
                batch.schema(),
                vec![Arc::new(Float64Array::from(accepted.clone()))],
            )
            .unwrap();
            covenant::engine::arrow::validate_batch(
                model,
                &prev,
                0,
                Some(&mut tracker),
                &mut prior,
            );
        }
        let mut out = covenant::report::Collector::new(usize::MAX);
        covenant::engine::arrow::validate_batch(
            model,
            &batch.slice(i, 1),
            0,
            Some(&mut tracker),
            &mut out,
        );
        let report = out.into_report(
            covenant::report::ReportHeader {
                contract_id: "t".into(),
                contract_version: "1.0.0".into(),
                owner: None,
                model: "t".into(),
                source: "edge".into(),
            },
            1,
        );
        let ok = report.violations == 0;
        if ok {
            accepted.push(*v);
        }
        engine.push(ok);
    }

    let ddl = export(
        &contract,
        &Options {
            table: Some("covenant_arrow.t"),
            ..Options::default()
        },
    )
    .unwrap()
    .sql;
    let mut script = format!(
        "SET client_min_messages = warning;\nDROP SCHEMA IF EXISTS covenant_arrow CASCADE;\nCREATE SCHEMA covenant_arrow;\n{ddl}\
         CREATE TEMP TABLE results (rec int, ok boolean);\nDO $t$ BEGIN\n"
    );
    for (i, v) in values.iter().enumerate() {
        script.push_str(&format!(
            "  BEGIN INSERT INTO covenant_arrow.t (\"f\") VALUES ({}); INSERT INTO results VALUES ({i}, true);\n  \
             EXCEPTION WHEN others THEN INSERT INTO results VALUES ({i}, false); END;\n",
            sql_float(*v)
        ));
    }
    script.push_str("END $t$;\nSELECT rec, ok FROM results ORDER BY rec;\nDROP SCHEMA covenant_arrow CASCADE;\n");
    let pg: Vec<bool> = psql(&script).iter().map(|r| r[1] == "t").collect();
    assert_eq!(pg, engine, "values {values:?}");
    // NaN passes both bounds in the engine; its second occurrence is a
    // duplicate; the infinities break a bound; -0.0 and 0.0 are one value.
    assert_eq!(engine, vec![true, false, false, false, true, false, true]);
}

#[test]
#[ignore = "needs COVENANT_TEST_POSTGRES and psql"]
fn translated_patterns_match_what_the_regex_crate_matches() {
    let patterns = [
        "^ord_[a-z0-9]{4}$",
        "a.c",
        "^[^a-c]+$",
        "^(?:ab|cd)*?e{1,2}$",
        r"^\.\$\\[\]\-x-z]+$",
        "^[à-ÿ]$",
        "日.本",
        "^a|b$",
        r"\t\r\n",
        "",
    ];
    let inputs = [
        "ord_ab12", "ORD_ab12", "abc", "a\nc", "a\rc", "xyz", "dde", "ababcde", "e", ".$\\]-y",
        "é", "日x本", "日\n本", "a", "zb", "\t\r\n", "", "ä",
    ];
    let mut cases = Vec::new();
    for (p, pattern) in patterns.iter().enumerate() {
        let translated = pg_regex(pattern).unwrap_or_else(|e| panic!("{pattern}: {e}"));
        for (i, input) in inputs.iter().enumerate() {
            cases.push(format!(
                "({p}, {i}, {}, {})",
                sql_text(input),
                sql_text(&translated)
            ));
        }
    }
    let script = format!(
        "SELECT p, i, (input COLLATE \"C\") ~ pattern\nFROM (VALUES\n{}\n) AS cases(p, i, input, pattern)\nORDER BY p, i;\n",
        cases.join(",\n")
    );
    let mut wrong = Vec::new();
    for row in psql(&script) {
        let (p, i): (usize, usize) = (row[0].parse().unwrap(), row[1].parse().unwrap());
        let rust = regex::Regex::new(patterns[p]).unwrap().is_match(inputs[i]);
        if (row[2] == "t") != rust {
            wrong.push(format!(
                "{:?} on {:?}: Postgres {}, the regex crate {rust}",
                patterns[p], inputs[i], row[2]
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
