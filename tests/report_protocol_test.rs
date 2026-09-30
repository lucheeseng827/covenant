//! The report protocol end to end: what `check --report-json` writes is pinned
//! by golden files (`tests/golden/report/`) and held to the published schema
//! (`schema/covenant-report.v1.json`). `COVENANT_BLESS=1` rewrites the golden
//! files; review the diff, since revision 1 may add fields but never rename or
//! remove one.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

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

const CLEAN: &str = r#"{"order_id":"ord_aaaa","amount":5,"currency":"USD"}
{"order_id":"ord_bbbb","amount":7,"currency":"EUR"}
"#;

// Row 1 repeats a key, goes below the minimum and names a currency outside the
// set; row 2's key breaks the pattern. None of these values may reach a report.
const DIRTY: &str = r#"{"order_id":"ord_aaaa","amount":5,"currency":"USD"}
{"order_id":"ord_aaaa","amount":-3,"currency":"BTC"}
{"order_id":"BAD-KEY","amount":7}
"#;

struct Run {
    code: i32,
    doc: Value,
    raw: String,
    stdout: String,
}

/// Runs `covenant check` in a fresh directory, with no CI in its environment
/// unless `env` adds one, and reads back the document it wrote.
fn check(contract: &str, data: &str, extra: &[&str], env: &[(&str, &str)]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("contract.yaml"), contract).unwrap();
    std::fs::write(dir.path().join("orders.ndjson"), data).unwrap();
    let mut cmd = Command::new(BIN);
    cmd.current_dir(dir.path())
        .args(["check", "orders.ndjson", "-c", "contract.yaml"])
        .args(["--report-json", "report.json"])
        .args(extra)
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("CI");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    let raw = std::fs::read_to_string(dir.path().join("report.json"))
        .unwrap_or_else(|e| panic!("no report written ({e}): {out:?}"));
    Run {
        code: out.status.code().unwrap(),
        doc: serde_json::from_str(&raw).unwrap(),
        raw,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    }
}

/// The document as written, field order kept, with what changes on every run
/// and every release (the timings, the engine's version) stamped out.
fn normalized(raw: &str) -> String {
    let stamp = |text: &str, pattern: &str, with: &str| {
        regex::Regex::new(pattern)
            .unwrap()
            .replace_all(text, with)
            .into_owned()
    };
    let text = stamp(
        raw,
        r#""started_at": "[^"]*""#,
        r#""started_at": "<started_at>""#,
    );
    let text = stamp(&text, r#""duration_ms": \d+"#, r#""duration_ms": 0"#);
    stamp(&text, r#""version": "[^"]*""#, r#""version": "<version>""#)
}

fn assert_golden(name: &str, raw: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/report")
        .join(format!("{name}.json"));
    let actual = normalized(raw);
    if std::env::var_os("COVENANT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing: run with COVENANT_BLESS=1", path.display()));
    assert_eq!(
        actual, expected,
        "{name}: the document changed. If that was meant, rerun with COVENANT_BLESS=1 and \
         review the diff: revision 1 may add fields, never rename or remove one"
    );
}

fn schema_errors(doc: &Value) -> Vec<String> {
    let schema: Value =
        serde_json::from_str(include_str!("../schema/covenant-report.v1.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    validator.iter_errors(doc).map(|e| e.to_string()).collect()
}

fn assert_valid(doc: &Value) {
    let errors = schema_errors(doc);
    assert!(
        errors.is_empty(),
        "invalid against the schema: {errors:?}\n{doc:#}"
    );
}

fn samples(doc: &Value) -> Vec<&Value> {
    doc["checks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["samples"].as_array().unwrap())
        .collect()
}

#[test]
fn a_clean_run_writes_a_passing_document() {
    let run = check(CONTRACT, CLEAN, &[], &[]);
    assert_eq!(run.code, 0);
    assert_eq!(run.doc["verdict"], "pass");
    assert_eq!(run.doc["run"]["plane"], "cli");
    assert!(run.doc["run"].get("ci").is_none());
    assert_valid(&run.doc);
    assert_golden("check_pass", &run.raw);
    assert!(run
        .stdout
        .contains("report: covenant-report/v1 → report.json"));
}

#[test]
fn a_failing_run_says_where_each_violation_was_and_never_the_value() {
    let run = check(CONTRACT, DIRTY, &[], &[]);
    assert_eq!(
        run.code, 1,
        "the report changes nothing about the exit code"
    );
    assert_eq!(run.doc["verdict"], "fail");
    assert_eq!(run.doc["sample_mode"], "masked");
    assert_valid(&run.doc);
    assert_golden("check_fail_masked", &run.raw);
    for s in samples(&run.doc) {
        let keys: Vec<&str> = s.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(
            keys.iter()
                .all(|k| ["field", "rule", "row", "type", "length"].contains(k)),
            "{keys:?}"
        );
    }
    // The terminal shows the values; the document, made to leave the machine,
    // shows none of them. Its clock is stamped out first: a date such as
    // 2026-09-30 holds "-3".
    assert!(run.stdout.contains("BTC"));
    let text = normalized(&run.raw);
    for value in ["BTC", "-3", "BAD-KEY"] {
        assert!(!text.contains(value), "{value} leaked into the report");
    }
}

#[test]
fn samples_none_keeps_the_counts_only() {
    let masked = check(CONTRACT, DIRTY, &[], &[]);
    let none = check(CONTRACT, DIRTY, &["--report-samples", "none"], &[]);
    assert_eq!(none.code, 1);
    assert_eq!(none.doc["sample_mode"], "none");
    assert!(samples(&none.doc).is_empty());
    assert_eq!(
        none.doc["checks"][0]["per_rule"],
        masked.doc["checks"][0]["per_rule"]
    );
    assert_valid(&none.doc);
}

#[test]
fn a_warn_policy_reports_warn_and_exits_clean() {
    let contract = format!("{CONTRACT}policy:\n  on_violation: warn\n");
    let run = check(&contract, DIRTY, &[], &[]);
    assert_eq!(run.code, 0);
    assert_eq!(run.doc["verdict"], "warn");
    assert_valid(&run.doc);
}

#[test]
fn the_budget_and_the_verdict_cover_every_source_together() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("contract.yaml"), CONTRACT).unwrap();
    std::fs::write(dir.path().join("a.ndjson"), CLEAN).unwrap();
    std::fs::write(dir.path().join("b.ndjson"), DIRTY).unwrap();
    let out = Command::new(BIN)
        .current_dir(dir.path())
        .args(["check", "a.ndjson", "b.ndjson", "-c", "contract.yaml"])
        .args(["--max-violations", "4", "--report-json", "r.json"])
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("CI")
        .output()
        .unwrap();
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("r.json")).unwrap()).unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(doc["checks"].as_array().unwrap().len(), 2);
    assert_eq!(doc["checks"][0]["source"], "a.ndjson");
    assert_eq!(doc["budget"], 4);
    assert_eq!(doc["violations"], 4);
    assert_eq!(doc["verdict"], "pass");
    assert_valid(&doc);
}

#[test]
fn in_github_actions_the_run_names_its_repository_commit_ref_and_url() {
    let run = check(
        CONTRACT,
        CLEAN,
        &[],
        &[
            ("GITHUB_ACTIONS", "true"),
            ("CI", "true"),
            ("GITHUB_REPOSITORY", "acme/shop"),
            ("GITHUB_SHA", "0123abcd"),
            ("GITHUB_REF", "refs/heads/main"),
            ("GITHUB_SERVER_URL", "https://github.com"),
            ("GITHUB_RUN_ID", "42"),
        ],
    );
    assert_eq!(run.doc["run"]["plane"], "ci");
    assert_eq!(
        run.doc["run"]["ci"],
        serde_json::json!({
            "provider": "github-actions",
            "repository": "acme/shop",
            "sha": "0123abcd",
            "ref": "refs/heads/main",
            "run_url": "https://github.com/acme/shop/actions/runs/42"
        })
    );
    assert_valid(&run.doc);
}

#[test]
fn the_schema_refuses_what_a_document_must_never_hold() {
    let run = check(CONTRACT, DIRTY, &[], &[]);
    // A value in a sample.
    let mut leaked = run.doc.clone();
    leaked["checks"][0]["samples"][0]["value"] = "BTC".into();
    assert!(!schema_errors(&leaked).is_empty());
    // A message, which quotes the value.
    let mut quoted = run.doc.clone();
    quoted["checks"][0]["samples"][0]["message"] = "value BTC not allowed".into();
    assert!(!schema_errors(&quoted).is_empty());
    // Samples under `none`.
    let mut none = run.doc.clone();
    none["sample_mode"] = "none".into();
    assert!(!schema_errors(&none).is_empty());
    // A CI context on a run that says it was not in CI.
    let mut plane = run.doc.clone();
    plane["run"]["ci"] = serde_json::json!({ "provider": "unknown" });
    assert!(!schema_errors(&plane).is_empty());
}

#[test]
fn a_report_that_cannot_be_written_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("contract.yaml"), CONTRACT).unwrap();
    std::fs::write(dir.path().join("orders.ndjson"), CLEAN).unwrap();
    let out = Command::new(BIN)
        .current_dir(dir.path())
        .args(["check", "orders.ndjson", "-c", "contract.yaml"])
        .args(["--report-json", "no-such-dir/report.json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no-such-dir/report.json"));
}

/// The result of a `covenant gate` run, and what it wrote.
struct GateRun {
    code: i32,
    stdout: String,
    stderr: String,
    /// The report, if one was written: parsed, and as written.
    doc: Option<(Value, String)>,
    /// The dead letters, one envelope per line.
    dlq: Vec<Value>,
}

/// Runs `covenant gate` over `data` in a fresh directory, dead letters to
/// `dlq.ndjson`. The stream is a file, so a gate that stops before reading it
/// never races the test's write.
fn gate(contract: &str, data: &str, extra: &[&str]) -> GateRun {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("contract.yaml"), contract).unwrap();
    std::fs::write(dir.path().join("in.ndjson"), data).unwrap();
    let out = Command::new(BIN)
        .current_dir(dir.path())
        .args(["gate", "-c", "contract.yaml", "--dlq", "dlq.ndjson"])
        .args(extra)
        .stdin(std::fs::File::open(dir.path().join("in.ndjson")).unwrap())
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("CI")
        .output()
        .unwrap();
    let doc = std::fs::read_to_string(dir.path().join("report.json"))
        .ok()
        .map(|raw| (serde_json::from_str(&raw).unwrap(), raw));
    let dlq = std::fs::read_to_string(dir.path().join("dlq.ndjson"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    GateRun {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        doc,
        dlq,
    }
}

#[test]
fn a_gate_run_reports_its_stream_and_what_became_of_the_records() {
    let run = gate(CONTRACT, DIRTY, &["--report-json", "report.json"]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    let (doc, raw) = run.doc.expect("the gate wrote its report");
    assert_eq!(doc["kind"], "gate");
    assert_eq!(doc["verdict"], "fail");
    assert_eq!(doc["run"]["plane"], "gate");
    assert_eq!(doc["checks"][0]["source"], "stdin");
    assert_eq!(doc["checks"][0]["rows"], 3);
    assert_eq!(
        doc["gate"],
        serde_json::json!({ "passed": 1, "blocked": 2, "warned": 0 })
    );
    assert_valid(&doc);
    assert_golden("gate_block", &raw);
    // A sample's row is the row of the dead letter that holds the record, so
    // the document and the DLQ can be joined; only the DLQ holds the value.
    let dead: Vec<u64> = run.dlq.iter().map(|e| e["row"].as_u64().unwrap()).collect();
    for s in samples(&doc) {
        assert!(dead.contains(&s["row"].as_u64().unwrap()), "{s} {dead:?}");
    }
    let text = normalized(&raw);
    for value in ["BTC", "-3", "BAD-KEY"] {
        assert!(!text.contains(value), "{value} leaked into the report");
    }
    // The stream still flows as it did: the clean record, and nothing else;
    // stdout is the stream, so the gate says where the report went on stderr.
    assert_eq!(run.stdout.lines().count(), 1);
    assert!(run
        .stderr
        .contains("report: covenant-report/v1 → report.json"));
}

#[test]
fn a_gate_under_warn_passes_every_record_and_says_warn() {
    let contract = format!("{CONTRACT}policy:\n  on_violation: warn\n");
    let run = gate(&contract, DIRTY, &["--report-json", "report.json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let (doc, _) = run.doc.unwrap();
    assert_eq!(doc["verdict"], "warn");
    assert_eq!(
        doc["gate"],
        serde_json::json!({ "passed": 3, "blocked": 0, "warned": 2 })
    );
    assert_valid(&doc);
}

#[test]
fn a_subprocess_gate_writes_its_report_and_keeps_stderr_for_its_replies() {
    let run = gate(
        CONTRACT,
        DIRTY,
        &[
            "--subprocess",
            "--report-json",
            "report.json",
            "--report-samples",
            "none",
        ],
    );
    assert_eq!(run.code, 1);
    let (doc, _) = run.doc.unwrap();
    assert_eq!(doc["kind"], "gate");
    assert_eq!(doc["sample_mode"], "none");
    assert!(samples(&doc).is_empty());
    assert_valid(&doc);
    // Every stderr line is the reply to a withheld record: the report adds
    // none, since stderr is the processor's channel.
    assert_eq!(run.stderr.lines().count(), 2, "{}", run.stderr);
}

#[test]
fn a_gate_refuses_a_report_with_nowhere_to_go_before_the_first_record() {
    let run = gate(
        CONTRACT,
        CLEAN,
        &["--report-json", "no-such-dir/report.json"],
    );
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("no-such-dir"), "{}", run.stderr);
    assert!(
        run.stdout.is_empty(),
        "no record may pass a gate that refused to start"
    );
}

const NEW_CONTRACT: &str = r#"
covenant: 1
id: orders
version: 2.0.0
owner: data@acme.io
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true, unique: true, pattern: "^ord_[a-z0-9]{4}$" }
      amount:   { type: integer, required: true, min: 0, max: 50 }
"#;

/// Runs `covenant diff` from CONTRACT to NEW_CONTRACT, with two consumers.
fn diff(extra: &[&str]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("old.yaml"), CONTRACT).unwrap();
    std::fs::write(dir.path().join("new.yaml"), NEW_CONTRACT).unwrap();
    let consumers = dir.path().join("consumers");
    std::fs::create_dir(&consumers).unwrap();
    std::fs::write(
        consumers.join("billing.yaml"),
        "consumer: 1\nid: billing\nowner: fin@acme.io\nconsumes:\n  - contract: orders\n    verified: 1.0.0\n    fields: [order_id, currency]\n",
    )
    .unwrap();
    std::fs::write(
        consumers.join("ops.yaml"),
        "consumer: 1\nid: ops\nconsumes:\n  - contract: orders\n    verified: 1.0.0\n    fields: [order_id]\n",
    )
    .unwrap();
    let out = Command::new(BIN)
        .current_dir(dir.path())
        .args(["diff", "old.yaml", "new.yaml", "--consumers", "consumers"])
        .args(["--report-json", "report.json"])
        .args(extra)
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("CI")
        .output()
        .unwrap();
    let raw = std::fs::read_to_string(dir.path().join("report.json"))
        .unwrap_or_else(|e| panic!("no report written ({e}): {out:?}"));
    Run {
        code: out.status.code().unwrap(),
        doc: serde_json::from_str(&raw).unwrap(),
        raw,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    }
}

#[test]
fn a_diff_run_reports_its_changes_and_whom_they_break() {
    let run = diff(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(run.doc["kind"], "diff");
    assert_eq!(run.doc["verdict"], "fail");
    assert_eq!(run.doc["run"]["plane"], "cli");
    let d = &run.doc["diff"];
    assert_eq!(d["contract_id"], "orders");
    assert_eq!(d["fail_on"], "breaking");
    assert_eq!(d["max_severity"], "breaking");
    assert_eq!(d["consumer_impact"]["impacted"][0]["consumer"], "billing");
    assert_eq!(
        d["consumer_impact"]["unaffected"],
        serde_json::json!(["ops"])
    );
    assert!(
        run.doc.get("checks").is_none(),
        "a diff has no data to count"
    );
    assert_valid(&run.doc);
    assert_golden("diff_breaking", &run.raw);
    assert!(run
        .stdout
        .contains("report: covenant-report/v1 → report.json"));
}

#[test]
fn a_diff_that_fail_on_lets_through_passes() {
    let run = diff(&["--fail-on", "never"]);
    assert_eq!(run.code, 0);
    assert_eq!(run.doc["verdict"], "pass");
    assert_eq!(run.doc["diff"]["fail_on"], "never");
    assert_eq!(run.doc["diff"]["max_severity"], "breaking");
    assert_valid(&run.doc);
}

#[test]
fn the_python_plane_keeps_its_name_in_ci_and_the_schema_holds_it() {
    use covenant::compile::CompiledContract;
    use covenant::protocol::{
        CiContext, Outcome, Producer, ReportDocument, RunClock, RunContext, SampleMode,
    };
    use covenant::spec::Contract;

    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("orders.ndjson");
    std::fs::write(&data, DIRTY).unwrap();
    let compiled =
        CompiledContract::compile(&Contract::parse(CONTRACT, "<test>").unwrap()).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let report = covenant::sources::check_path(&compiled, model, &data, None).unwrap();
    let ci = CiContext::from_lookup(|k| (k == "CI").then(|| "true".to_string()));
    let run = RunContext::new(Producer::Python, RunClock::start().stop(), ci);
    let doc = ReportDocument::for_check(
        std::slice::from_ref(&report),
        0,
        Outcome::judge(report.violations, 0, false),
        &SampleMode::Masked,
        run,
    );
    let doc = serde_json::to_value(&doc).unwrap();
    assert_eq!(doc["run"]["plane"], "python");
    assert_eq!(doc["run"]["ci"]["provider"], "unknown");
    assert_eq!(doc["verdict"], "fail");
    assert_valid(&doc);
}

#[test]
fn the_schema_holds_each_kind_to_its_body() {
    let check_doc = check(CONTRACT, DIRTY, &[], &[]).doc;
    let gate_doc = gate(CONTRACT, DIRTY, &["--report-json", "report.json"])
        .doc
        .unwrap()
        .0;
    let diff_doc = diff(&[]).doc;
    for doc in [&check_doc, &gate_doc, &diff_doc] {
        assert_valid(doc);
    }
    let refused = |doc: &Value, change: &dyn Fn(&mut Value)| {
        let mut doc = doc.clone();
        change(&mut doc);
        assert!(!schema_errors(&doc).is_empty(), "accepted: {doc:#}");
    };
    // A check without its counts, a gate without what became of its
    // records, a diff without its changes.
    refused(&check_doc, &|d| {
        d.as_object_mut().unwrap().remove("checks");
    });
    refused(&gate_doc, &|d| {
        d.as_object_mut().unwrap().remove("gate");
    });
    refused(&diff_doc, &|d| {
        d.as_object_mut().unwrap().remove("diff");
    });
    // A diff decides pass or fail; warn is a data policy.
    refused(&diff_doc, &|d| d["verdict"] = "warn".into());
    // The ci plane names its CI run.
    refused(&check_doc, &|d| d["run"]["plane"] = "ci".into());
    // A gate's samples hold no values either.
    refused(&gate_doc, &|d| {
        d["checks"][0]["samples"][0]["value"] = "BTC".into();
    });
}

/// Runs `covenant check` with `--report-samples hashed`, the key (if any) in
/// the environment, over `data` written as `name`.
fn hashed(data: &str, name: &str, key: Option<&str>) -> (i32, Option<String>, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("contract.yaml"), CONTRACT).unwrap();
    std::fs::write(dir.path().join(name), data).unwrap();
    let mut cmd = Command::new(BIN);
    cmd.current_dir(dir.path())
        .args(["check", name, "-c", "contract.yaml"])
        .args(["--report-json", "report.json", "--report-samples", "hashed"])
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("CI")
        .env_remove("COVENANT_SAMPLE_KEY");
    if let Some(key) = key {
        cmd.env("COVENANT_SAMPLE_KEY", key);
    }
    let out = cmd.output().unwrap();
    let raw = std::fs::read_to_string(dir.path().join("report.json")).ok();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap(), raw, said)
}

const KEY_A: &str = "4f1c0d9e8b7a6f5e4d3c2b1a0f9e8d7c";
const KEY_B: &str = "a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5";

#[test]
fn hashed_samples_say_the_same_bad_value_without_saying_it() {
    // BTC twice, XAU once: three `allowed` violations, two of one value.
    let data = r#"{"order_id":"ord_aaa1","amount":5,"currency":"BTC"}
{"order_id":"ord_aaa2","amount":5,"currency":"BTC"}
{"order_id":"ord_aaa3","amount":5,"currency":"XAU"}
"#;
    let (code, raw, said) = hashed(data, "orders.ndjson", Some(KEY_A));
    assert_eq!(code, 1, "{said}");
    let raw = raw.unwrap();
    let doc: Value = serde_json::from_str(&raw).unwrap();
    assert_valid(&doc);
    assert_eq!(doc["sample_mode"], "hashed");
    let key_id = doc["sample_key"].as_str().unwrap().to_string();
    assert_eq!(key_id.len(), 16);
    let found: Vec<(u64, &str, &str, u64)> = samples(&doc)
        .iter()
        .map(|s| {
            (
                s["row"].as_u64().unwrap(),
                s["type"].as_str().unwrap(),
                s["hash"].as_str().unwrap(),
                s["length"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(found.len(), 3);
    assert!(found
        .iter()
        .all(|(_, ty, _, len)| *ty == "string" && *len == 3));
    assert_eq!(found[0].2, found[1].2, "the same value, the same hash");
    assert_ne!(found[0].2, found[2].2, "another value, another hash");
    for secret in ["BTC", "XAU", KEY_A] {
        assert!(!raw.contains(secret), "{secret} is in the report");
    }
    // The terminal shows the values, as it always has; never the key.
    assert!(said.contains("BTC"));
    assert!(!said.contains(KEY_A), "the key was printed");
    // Another key: another id, and hashes that cannot be compared with these.
    let (_, other, _) = hashed(data, "orders.ndjson", Some(KEY_B));
    let other: Value = serde_json::from_str(&other.unwrap()).unwrap();
    assert_ne!(other["sample_key"].as_str().unwrap(), key_id);
    assert_ne!(samples(&other)[0]["hash"].as_str().unwrap(), found[0].2);
}

#[test]
fn hashed_samples_need_a_key_from_the_environment() {
    let (code, raw, said) = hashed(DIRTY, "orders.ndjson", None);
    assert_eq!(code, 2);
    assert!(raw.is_none(), "no key, no report");
    assert!(said.contains("COVENANT_SAMPLE_KEY"), "{said}");
    let (code, raw, said) = hashed(DIRTY, "orders.ndjson", Some("short"));
    assert_eq!(code, 2);
    assert!(raw.is_none());
    assert!(said.contains("at least 16 bytes"), "{said}");
    assert!(!said.contains("short\n"), "the key is not repeated back");
}

// CSV is read by the Arrow engine.
#[cfg(feature = "arrow")]
#[test]
fn a_value_has_the_same_type_length_and_hash_in_ndjson_and_csv() {
    let csv = "order_id,amount,currency\nord_aaaa,5,USD\nord_aaaa,-3,BTC\nBAD-KEY,7,\n";
    let from = |data: &str, name: &str| -> Vec<String> {
        let (code, raw, said) = hashed(data, name, Some(KEY_A));
        assert_eq!(code, 1, "{said}");
        let doc: Value = serde_json::from_str(&raw.unwrap()).unwrap();
        assert_valid(&doc);
        // The row engine reports record by record, the Arrow engine column
        // by column: the same samples, in another order.
        let mut found: Vec<String> = samples(&doc).iter().map(|s| s.to_string()).collect();
        found.sort();
        found
    };
    let ndjson = from(DIRTY, "orders.ndjson");
    assert_eq!(ndjson.len(), 4);
    assert_eq!(ndjson, from(csv, "orders.csv"));
}
