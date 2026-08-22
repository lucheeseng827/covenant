//! End-to-end CLI tests against the real binary: exit codes are the product
//! contract CI scripts key off, so they are pinned here.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

const CONTRACT: &str = r#"
covenant: 1
id: orders
version: 1.0.0
owner: data@acme.io
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true, unique: true, pattern: "^ord_[a-z0-9]{4}$" }
      amount:   { type: integer, required: true, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#;

fn write(dir: &tempfile::TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn validate_clean_contract_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let out = Command::new(BIN).arg("validate").arg(&contract).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn validate_broken_contract_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(
        &dir,
        "c.yaml",
        r#"
covenant: 1
id: t
version: 1.0.0
models: { m: { fields: { a: { type: string, pattern: "([" } } } }
"#,
    );
    let out = Command::new(BIN).arg("validate").arg(&contract).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("does not compile"));
}

#[test]
fn validate_unparseable_contract_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", "covenant: [not: valid");
    let out = Command::new(BIN).arg("validate").arg(&contract).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn check_ndjson_pass_and_fail() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let good = write(
        &dir,
        "good.ndjson",
        r#"{"order_id":"ord_ab12","amount":10}
{"order_id":"ord_cd34","amount":20,"currency":"USD"}
"#,
    );
    let bad = write(
        &dir,
        "bad.ndjson",
        r#"{"order_id":"ord_ab12","amount":10}
{"order_id":"ord_ab12","amount":-5,"currency":"JPY"}
not json at all
"#,
    );

    let out = Command::new(BIN)
        .args(["check", "--contract"])
        .arg(&contract)
        .arg(&good)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stdout).contains("PASS"));

    let out = Command::new(BIN)
        .args(["check", "--contract"])
        .arg(&contract)
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL"), "{stdout}");
    // duplicate id, min, allowed, invalid JSON line
    assert!(stdout.contains("unique"), "{stdout}");
    assert!(stdout.contains("min"), "{stdout}");
    assert!(stdout.contains("allowed"), "{stdout}");
    assert!(stdout.contains("record_not_object"), "{stdout}");
    // The failing report names the owning team.
    assert!(stdout.contains("data@acme.io"), "{stdout}");
}

#[test]
fn check_json_format_is_machine_readable() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let bad = write(&dir, "bad.ndjson", r#"{"order_id":"nope","amount":10}"#);
    let out = Command::new(BIN)
        .args(["check", "--format", "json", "--contract"])
        .arg(&contract)
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let reports: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reports[0]["contract_id"], "orders");
    assert_eq!(reports[0]["violations"], 1);
    assert_eq!(reports[0]["per_rule"][0]["rule"], "pattern");
}

#[test]
fn check_csv_and_parquet() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);

    let csv = write(
        &dir,
        "data.csv",
        "order_id,amount\nord_ab12,10\nord_cd34,999\n",
    );
    let out = Command::new(BIN)
        .args(["check", "--contract"])
        .arg(&contract)
        .arg(&csv)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("max"));

    // Parquet fixture written through the arrow writer.
    let parquet_path = dir.path().join("data.parquet");
    let batch = RecordBatch::try_from_iter(vec![
        (
            "order_id".to_string(),
            Arc::new(StringArray::from(vec!["ord_ab12", "ord_cd34"])) as ArrayRef,
        ),
        (
            "amount".to_string(),
            Arc::new(Int64Array::from(vec![10i64, 20])) as ArrayRef,
        ),
    ])
    .unwrap();
    let file = std::fs::File::create(&parquet_path).unwrap();
    let mut writer =
        parquet::arrow::ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let out = Command::new(BIN)
        .args(["check", "--contract"])
        .arg(&contract)
        .arg(&parquet_path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn gate_blocks_dirty_records() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let dlq = dir.path().join("dlq.ndjson");

    let mut child = Command::new(BIN)
        .args(["gate", "--quiet", "--dlq"])
        .arg(&dlq)
        .arg("--contract")
        .arg(&contract)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(
            br#"{"order_id":"ord_ab12","amount":10}
{"order_id":"ord_zz99","amount":-1}
{"order_id":"ord_cd34","amount":20}
"#,
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();

    // Blocked records mean a failed (exit 1) gate under policy block.
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let passed: Vec<&str> = stdout.lines().collect();
    assert_eq!(passed.len(), 2, "{stdout}");
    assert!(passed[0].contains("ord_ab12"));
    assert!(passed[1].contains("ord_cd34"));

    // DLQ carries the record and its violations for replay.
    let dlq_content = std::fs::read_to_string(&dlq).unwrap();
    let envelope: serde_json::Value =
        serde_json::from_str(dlq_content.lines().next().unwrap()).unwrap();
    assert_eq!(envelope["contract_id"], "orders");
    assert_eq!(envelope["record"]["order_id"], "ord_zz99");
    assert_eq!(envelope["violations"][0]["rule"], "min");
}

/// The DLQ is the only copy of blocked records — a rerun must append, never
/// truncate the previous run's dead letters.
#[test]
fn gate_stats_flag_writes_the_sidecar_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let dlq = dir.path().join("dlq.ndjson");
    let stats = dir.path().join("stats.json");

    let mut child = Command::new(BIN)
        .args(["gate", "--quiet", "--dlq"])
        .arg(&dlq)
        .arg("--stats")
        .arg(&stats)
        .arg("--contract")
        .arg(&contract)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"order_id\":\"ord_ab12\",\"amount\":10}\n{\"order_id\":\"ord_zz99\",\"amount\":-1}\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "one blocked record fails the stream");

    let snapshot: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&stats).unwrap()).unwrap();
    assert_eq!(snapshot["records"], 2);
    assert_eq!(snapshot["blocked"], 1);
    assert_eq!(snapshot["per_rule"][0]["rule"], "min");
    // Envelopes now carry a timestamp for the DLQ read endpoint.
    let envelope: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&dlq).unwrap().lines().next().unwrap())
            .unwrap();
    assert!(envelope["ts"].as_str().is_some(), "{envelope}");
}

#[test]
fn gate_dlq_appends_across_runs() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let dlq = dir.path().join("dlq.ndjson");

    for bad_id in ["ord_zz99", "ord_yy88"] {
        let mut child = Command::new(BIN)
            .args(["gate", "--quiet", "--dlq"])
            .arg(&dlq)
            .arg("--contract")
            .arg(&contract)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{{\"order_id\":\"{bad_id}\",\"amount\":-1}}\n").as_bytes())
            .unwrap();
        child.wait_with_output().unwrap();
    }
    let dlq_content = std::fs::read_to_string(&dlq).unwrap();
    assert_eq!(dlq_content.lines().count(), 2, "{dlq_content}");
    assert!(dlq_content.contains("ord_zz99") && dlq_content.contains("ord_yy88"));
}

/// --max-violations budgets the whole run, not each file: two files with one
/// violation each must fail a budget of 1.
#[test]
fn check_budget_is_per_run_not_per_file() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let a = write(&dir, "a.ndjson", r#"{"order_id":"ord_ab12","amount":-1}"#);
    let b = write(&dir, "b.ndjson", r#"{"order_id":"ord_cd34","amount":-1}"#);

    let out = Command::new(BIN)
        .args(["check", "--max-violations", "1", "--contract"])
        .arg(&contract)
        .arg(&a)
        .arg(&b)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stdout).contains("run total: 2 violations"));

    // The same two files fit a budget of 2.
    let out = Command::new(BIN)
        .args(["check", "--max-violations", "2", "--contract"])
        .arg(&contract)
        .arg(&a)
        .arg(&b)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

/// CSV columns take their type from the CONTRACT, not from inference — a
/// digit-only string column (leading-zero IDs) must validate as a string.
#[test]
fn csv_leading_zero_strings_stay_strings() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(
        &dir,
        "c.yaml",
        r#"
covenant: 1
id: zips
version: 1.0.0
models:
  zips:
    fields:
      zip: { type: string, required: true, pattern: "^[0-9]{5}$" }
"#,
    );
    let csv = write(&dir, "zips.csv", "zip\n00123\n94110\n");
    let out = Command::new(BIN)
        .args(["check", "--contract"])
        .arg(&contract)
        .arg(&csv)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A header-only CSV missing a required column is not "clean" — the schema
/// promise fails even with zero data rows.
#[test]
fn header_only_csv_still_fails_schema_promises() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let csv = write(&dir, "empty.csv", "order_id\n");
    let out = Command::new(BIN)
        .args(["check", "--contract"])
        .arg(&contract)
        .arg(&csv)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("schema_missing_field"));
}

#[test]
fn gate_clean_stream_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let mut child = Command::new(BIN)
        .args(["gate", "--quiet", "--contract"])
        .arg(&contract)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"order_id\":\"ord_ab12\",\"amount\":10}\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 1);
}

#[test]
fn diff_fail_on_thresholds() {
    let dir = tempfile::tempdir().unwrap();
    let old = write(&dir, "old.yaml", CONTRACT);
    // Field removed with a proper major bump: breaking finding, no version finding.
    let new = write(
        &dir,
        "new.yaml",
        r#"
covenant: 1
id: orders
version: 2.0.0
owner: data@acme.io
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true, unique: true, pattern: "^ord_[a-z0-9]{4}$" }
      currency: { type: string, allowed: [USD, EUR] }
"#,
    );

    let out = Command::new(BIN)
        .args(["diff"])
        .arg(&old)
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("field removed"));

    let out = Command::new(BIN)
        .args(["diff", "--fail-on", "never"])
        .arg(&old)
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));

    // Identical contracts: clean under any threshold.
    let out = Command::new(BIN)
        .args(["diff", "--fail-on", "any"])
        .arg(&old)
        .arg(&old)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn init_writes_a_valid_contract_and_refuses_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("covenant.yaml");
    let out = Command::new(BIN).arg("init").arg(&path).output().unwrap();
    assert_eq!(out.status.code(), Some(0));

    // The starter contract must pass its own linter.
    let out = Command::new(BIN).arg("validate").arg(&path).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let out = Command::new(BIN).arg("init").arg(&path).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn model_ambiguity_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(
        &dir,
        "c.yaml",
        r#"
covenant: 1
id: multi
version: 1.0.0
models:
  a: { fields: { x: { type: string } } }
  b: { fields: { y: { type: string } } }
"#,
    );
    let data = write(&dir, "d.ndjson", r#"{"x":"1"}"#);
    let out = Command::new(BIN)
        .args(["check", "--contract"])
        .arg(&contract)
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--model"));

    let out = Command::new(BIN)
        .args(["check", "--model", "a", "--contract"])
        .arg(&contract)
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}
