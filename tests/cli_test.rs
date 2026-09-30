//! End-to-end CLI tests against the real binary: exit codes are the product
//! contract CI scripts key off, so they are pinned here.
// Arrow fixtures (Parquet files, RecordBatches) throughout: this suite runs
// in every build with the `arrow` feature, which is on by default.
#![cfg(feature = "arrow")]

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
    let out = Command::new(BIN)
        .arg("validate")
        .arg(&contract)
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
    let out = Command::new(BIN)
        .arg("validate")
        .arg(&contract)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("does not compile"));
}

#[test]
fn validate_unparseable_contract_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", "covenant: [not: valid");
    let out = Command::new(BIN)
        .arg("validate")
        .arg(&contract)
        .output()
        .unwrap();
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
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
    let mut writer = parquet::arrow::ArrowWriter::try_new(file, batch.schema(), None).unwrap();
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

/// `gate --subprocess` talks one line at a time: each line in is answered,
/// flushed, before the next is sent — the record on stdout if it goes on, its
/// dead-letter envelope on stderr if it is withheld, a blank line for a blank
/// one. A reply that never comes fails the test instead of hanging it.
#[test]
fn gate_subprocess_answers_every_line_as_it_arrives() {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let block = write(&dir, "c.yaml", CONTRACT);
    let warn = write(
        &dir,
        "warn.yaml",
        &CONTRACT.replace(
            "owner: data@acme.io",
            "owner: data@acme.io\npolicy:\n  on_violation: warn",
        ),
    );
    let good = r#"{"order_id":"ord_ab12","amount":10}"#;
    let bad = r#"{"order_id":"nope","amount":10}"#;

    // Spawn the gate; stdout and stderr lines arrive on one channel, tagged.
    let converse = |contract: &std::path::Path, dlq: &std::path::Path| {
        let mut child = Command::new(BIN)
            .args(["gate", "--subprocess", "--contract"])
            .arg(contract)
            .arg("--dlq")
            .arg(dlq)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel::<(&'static str, String)>();
        for (name, stream) in [
            (
                "stdout",
                Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
            ),
            ("stderr", Box::new(child.stderr.take().unwrap())),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines() {
                    let _ = tx.send((name, line.unwrap()));
                }
            });
        }
        (child, rx)
    };
    let ask = |child: &mut std::process::Child,
               rx: &mpsc::Receiver<(&'static str, String)>,
               line: &str| {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        rx.recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|_| panic!("no reply to {line:?}"))
    };

    let dlq = dir.path().join("blocked.dlq.ndjson");
    let (mut child, rx) = converse(&block, &dlq);
    assert_eq!(ask(&mut child, &rx, good), ("stdout", good.to_string()));
    let (stream, envelope) = ask(&mut child, &rx, bad);
    assert_eq!(stream, "stderr");
    let envelope: serde_json::Value = serde_json::from_str(&envelope).unwrap();
    assert_eq!(envelope["violations"][0]["rule"], "pattern");
    assert_eq!(envelope["row"], 1);
    assert_eq!(ask(&mut child, &rx, ""), ("stdout", String::new()));
    // Uniqueness spans the conversation, not one line.
    let (stream, envelope) = ask(&mut child, &rx, good);
    assert_eq!(stream, "stderr");
    assert!(envelope.contains(r#""rule":"unique""#), "{envelope}");
    drop(child.stdin.take());
    assert_eq!(child.wait().unwrap().code(), Some(1));
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "no summary after the stream"
    );
    assert_eq!(std::fs::read_to_string(&dlq).unwrap().lines().count(), 2);

    // Under `on_violation: warn` the record goes on: its only answer is on
    // stdout, and its dead letter goes to the DLQ file alone.
    let dlq = dir.path().join("warned.dlq.ndjson");
    let (mut child, rx) = converse(&warn, &dlq);
    assert_eq!(ask(&mut child, &rx, bad), ("stdout", bad.to_string()));
    assert_eq!(ask(&mut child, &rx, good), ("stdout", good.to_string()));
    drop(child.stdin.take());
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    assert_eq!(std::fs::read_to_string(&dlq).unwrap().lines().count(), 1);

    // The stats sidecar is for the batch gate.
    let out = Command::new(BIN)
        .args(["gate", "--subprocess", "--stats", "s.json", "--contract"])
        .arg(&block)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
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
    assert_eq!(
        out.status.code(),
        Some(1),
        "one blocked record fails the stream"
    );

    let snapshot: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&stats).unwrap()).unwrap();
    assert_eq!(snapshot["records"], 2);
    assert_eq!(snapshot["blocked"], 1);
    assert_eq!(snapshot["per_rule"][0]["rule"], "min");
    // Envelopes now carry a timestamp for the DLQ read endpoint.
    let envelope: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&dlq)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
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
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
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

/// A file the reader hands over in several batches reports what the same rows
/// report as one batch. A schema problem is a fact about the file, so it
/// counts once, not once per batch, and the verdict under a budget cannot
/// depend on how big the file is.
#[test]
fn a_file_read_in_several_batches_counts_a_schema_problem_once() {
    use std::collections::HashMap;

    use covenant::compile::CompiledContract;
    use covenant::engine::{arrow::validate_batch, UniqueTracker};
    use covenant::report::{Collector, ReportHeader};
    use covenant::spec::Contract;
    use parquet::file::properties::WriterProperties;

    // More rows than one reader batch (8192), in four row groups.
    const ROWS: usize = 20_000;
    let id = |i: usize| {
        const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let (mut n, mut s) = (i, [b'0'; 4]);
        for c in s.iter_mut().rev() {
            *c = DIGITS[n % 36];
            n /= 36;
        }
        format!("ord_{}", std::str::from_utf8(&s).unwrap())
    };
    // No `amount` (required) and an undeclared `note` (the model is strict);
    // a bad id in every row group, and the last row repeats the second.
    let ids: Vec<String> = (0..ROWS)
        .map(|i| match i {
            _ if i % 5000 == 7 => format!("BAD{i}"),
            _ if i == ROWS - 1 => id(1),
            _ => id(i),
        })
        .collect();
    let batch = RecordBatch::try_from_iter(vec![
        (
            "order_id".to_string(),
            Arc::new(StringArray::from(ids.clone())) as ArrayRef,
        ),
        (
            "note".to_string(),
            Arc::new(StringArray::from(vec!["n"; ROWS])) as ArrayRef,
        ),
    ])
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let contract = write(&dir, "c.yaml", CONTRACT);
    let parquet_path = dir.path().join("many.parquet");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(5000))
        .build();
    let file = std::fs::File::create(&parquet_path).unwrap();
    let mut writer =
        parquet::arrow::ArrowWriter::try_new(file, batch.schema(), Some(props)).unwrap();
    writer.write(&batch).unwrap();
    let meta = writer.close().unwrap();
    assert_eq!(meta.num_row_groups(), 4);
    let csv: String = std::iter::once("order_id,note\n".to_string())
        .chain(ids.iter().map(|i| format!("{i},n\n")))
        .collect();
    let csv_path = write(&dir, "many.csv", &csv);

    // The same rows as one batch, through the library.
    let compiled =
        CompiledContract::compile(&Contract::parse(CONTRACT, "c.yaml").unwrap()).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let mut collector = Collector::new(10);
    let mut unique = UniqueTracker::new(model);
    validate_batch(model, &batch, 0, Some(&mut unique), &mut collector);
    let one_batch = collector.into_report(
        ReportHeader {
            contract_id: "orders".into(),
            contract_version: "1.0.0".into(),
            owner: None,
            model: "orders".into(),
            source: "<one batch>".into(),
        },
        ROWS as u64,
    );
    let expected: HashMap<(String, String), u64> = one_batch
        .per_rule
        .iter()
        .map(|r| ((r.field.clone(), r.rule.to_string()), r.count))
        .collect();
    let at = |f: &str, r: &str| expected[&(f.to_string(), r.to_string())];
    assert_eq!(
        (
            at("amount", "schema_missing_field"),
            at("note", "unexpected_field"),
            at("order_id", "pattern"),
            at("order_id", "unique"),
        ),
        (1, 1, 4, 1)
    );

    for path in [&parquet_path, &csv_path] {
        let out = Command::new(BIN)
            .args(["check", "--format", "json", "--contract"])
            .arg(&contract)
            .arg(path)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", path.display());
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let report = &report[0];
        let counts: HashMap<(String, String), u64> = report["per_rule"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    (
                        r["field"].as_str().unwrap().to_string(),
                        r["rule"].as_str().unwrap().to_string(),
                    ),
                    r["count"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(counts, expected, "{}", path.display());
        assert_eq!(
            report["violations"],
            one_batch.violations,
            "{}",
            path.display()
        );
        assert_eq!(report["rows"], ROWS as u64, "{}", path.display());
    }
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
    let out = Command::new(BIN)
        .arg("validate")
        .arg(&path)
        .output()
        .unwrap();
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
