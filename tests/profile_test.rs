//! Profiles: per-field sketches collected in the check's own read, identical
//! across data formats, and mergeable across runs.
// Arrow fixtures (Parquet files, RecordBatches) throughout: this suite runs
// in every build with the `arrow` feature, which is on by default.
#![cfg(feature = "arrow")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, FixedSizeBinaryArray, Float32Array, Float64Array,
    Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
};
use covenant::compile::CompiledContract;
use covenant::profile::{Profile, Profiler};
use covenant::sources::check_path_profiled;
use covenant::spec::Contract;
use serde_json::json;

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

const CONTRACT: &str = r#"
covenant: 1
id: events
version: 1.0.0
models:
  events:
    fields:
      id:       { type: string, required: true }
      amount:   { type: integer, nullable: true }
      score:    { type: float }
      ok:       { type: boolean }
      currency: { type: string, allowed: [USD, EUR] }
      at:       { type: timestamp }
      day:      { type: date }
      ref:      { type: uuid }
"#;

/// One synthetic record, in every representation the tests need.
struct Rec {
    id: String,
    amount: Option<i64>,
    score: f64,
    ok: bool,
    currency: &'static str,
    at_ms: i64,
    day: i32,
    reference: u128,
}

fn epoch_ms(rfc3339: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .timestamp_millis()
}

fn records(n: usize) -> Vec<Rec> {
    let start = epoch_ms("2026-09-01T00:00:00Z");
    // 2026-09-01 as days since the Unix epoch.
    let day0 = 20_697;
    (0..n)
        .map(|i| Rec {
            id: format!("e{i:04}"),
            amount: (i % 10 != 0).then_some((i * 37 % 1000) as i64),
            // Quarters sum exactly in binary, so every summation order agrees.
            score: (i % 17) as f64 * 0.25,
            ok: i % 3 == 0,
            currency: if i % 4 == 0 { "EUR" } else { "USD" },
            at_ms: start + i as i64 * 60_000,
            day: day0 + (i % 30) as i32,
            reference: i as u128 * 7_919,
        })
        .collect()
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

fn date(days: i32) -> String {
    chrono::NaiveDate::from_num_days_from_ce_opt(days + 719_163)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string()
}

fn uuid(u: u128) -> String {
    let h = format!("{u:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn write_ndjson(path: &Path, recs: &[Rec]) {
    let lines: Vec<String> = recs
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "amount": r.amount,
                "score": r.score,
                "ok": r.ok,
                "currency": r.currency,
                "at": rfc3339(r.at_ms),
                "day": date(r.day),
                "ref": uuid(r.reference),
            })
            .to_string()
        })
        .collect();
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn write_csv(path: &Path, recs: &[Rec]) {
    let mut out = String::from("id,amount,score,ok,currency,at,day,ref\n");
    for r in recs {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            r.id,
            r.amount.map(|a| a.to_string()).unwrap_or_default(),
            r.score,
            r.ok,
            r.currency,
            rfc3339(r.at_ms),
            date(r.day),
            uuid(r.reference)
        ));
    }
    std::fs::write(path, out).unwrap();
}

fn write_parquet(path: &Path, recs: &[Rec]) {
    let refs: Vec<[u8; 16]> = recs.iter().map(|r| r.reference.to_be_bytes()).collect();
    let batch = RecordBatch::try_from_iter(vec![
        (
            "id",
            Arc::new(StringArray::from_iter_values(
                recs.iter().map(|r| r.id.clone()),
            )) as ArrayRef,
        ),
        (
            "amount",
            Arc::new(Int64Array::from_iter(recs.iter().map(|r| r.amount))) as ArrayRef,
        ),
        (
            "score",
            Arc::new(Float64Array::from_iter_values(recs.iter().map(|r| r.score))) as ArrayRef,
        ),
        (
            "ok",
            Arc::new(BooleanArray::from_iter(recs.iter().map(|r| Some(r.ok)))) as ArrayRef,
        ),
        (
            "currency",
            Arc::new(StringArray::from_iter_values(
                recs.iter().map(|r| r.currency),
            )) as ArrayRef,
        ),
        (
            "at",
            Arc::new(
                TimestampMillisecondArray::from_iter_values(recs.iter().map(|r| r.at_ms))
                    .with_timezone("UTC"),
            ) as ArrayRef,
        ),
        (
            "day",
            Arc::new(Date32Array::from_iter_values(recs.iter().map(|r| r.day))) as ArrayRef,
        ),
        (
            "ref",
            Arc::new(FixedSizeBinaryArray::try_from_iter(refs.into_iter()).unwrap()) as ArrayRef,
        ),
    ])
    .unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut writer = parquet::arrow::ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

fn profile_of(contract: &str, paths: &[PathBuf]) -> Profile {
    let doc = Contract::parse(contract, "c.yaml").unwrap();
    let compiled = CompiledContract::compile(&doc).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let mut profiler = Profiler::new(&compiled, model);
    for p in paths {
        check_path_profiled(&compiled, model, p, None, Some(&mut profiler)).unwrap();
    }
    profiler.finish()
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(BIN).args(args).output().expect("binary runs")
}

#[test]
fn ndjson_csv_and_parquet_of_the_same_records_profile_identically() {
    let dir = tempfile::tempdir().unwrap();
    // Enough rows that the quantile sketch compacts.
    let recs = records(1_500);
    let (nd, csv, pq) = (
        dir.path().join("e.ndjson"),
        dir.path().join("e.csv"),
        dir.path().join("e.parquet"),
    );
    write_ndjson(&nd, &recs);
    write_csv(&csv, &recs);
    write_parquet(&pq, &recs);

    let from_ndjson = profile_of(CONTRACT, std::slice::from_ref(&nd));
    for other in [&csv, &pq] {
        let p = profile_of(CONTRACT, std::slice::from_ref(other));
        assert_eq!(p.rows, from_ndjson.rows);
        for (name, f) in &from_ndjson.fields {
            assert_eq!(
                &p.fields[name],
                f,
                "field {name} differs for {}",
                other.display()
            );
        }
    }

    let f = &from_ndjson.fields;
    assert_eq!(f["amount"].nulls, 150);
    assert_eq!(f["currency"].values.as_ref().unwrap().counts["EUR"], 375);
    assert_eq!(f["ok"].values.as_ref().unwrap().counts["true"], 500);
    let span = f["at"].span.as_ref().unwrap();
    assert_eq!(
        (span.min.as_str(), span.max.as_str()),
        ("2026-09-01T00:00:00Z", "2026-09-02T00:59:00Z")
    );
    let distinct = f["id"].distinct_estimate().unwrap();
    assert!((distinct - 1_500.0).abs() < 1_500.0 * 0.05, "{distinct}");
    assert_eq!(f["id"].lengths.as_ref().unwrap().max, 5);
}

#[test]
fn a_merged_profile_equals_one_run_over_both_files() {
    let dir = tempfile::tempdir().unwrap();
    let recs = records(120);
    let (a, b) = (dir.path().join("a.ndjson"), dir.path().join("b.ndjson"));
    write_ndjson(&a, &recs[..70]);
    write_ndjson(&b, &recs[70..]);
    let contract = dir.path().join("c.yaml");
    std::fs::write(&contract, CONTRACT).unwrap();
    let p = |name: &str| dir.path().join(name).display().to_string();
    let (c, a, b) = (
        contract.display().to_string(),
        a.display().to_string(),
        b.display().to_string(),
    );

    let both = run(&["check", &a, &b, "-c", &c, "--profile", &p("ab.json")]);
    assert_eq!(
        both.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&both.stdout)
    );
    assert!(String::from_utf8_lossy(&both.stdout).contains("profile: 8 fields over 120 rows"));
    assert_eq!(
        run(&["check", &a, "-c", &c, "--profile", &p("a.json")])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["check", &b, "-c", &c, "--profile", &p("b.json")])
            .status
            .code(),
        Some(0)
    );
    let merged = run(&[
        "profile",
        "merge",
        &p("a.json"),
        &p("b.json"),
        "-o",
        &p("m.json"),
    ]);
    assert_eq!(
        merged.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&merged.stderr)
    );

    let one = Profile::from_path(Path::new(&p("ab.json"))).unwrap();
    let two = Profile::from_path(Path::new(&p("m.json"))).unwrap();
    assert_eq!((one.runs, two.runs), (1, 2));
    assert_eq!(one.rows, two.rows);
    assert_eq!(one.fields, two.fields);

    // Merging to stdout gives the same profile.
    let out = run(&["profile", "merge", &p("a.json"), &p("b.json")]);
    let from_stdout = Profile::parse(&String::from_utf8_lossy(&out.stdout), "stdout").unwrap();
    assert_eq!(from_stdout, two);
}

#[test]
fn show_summarizes_each_field() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("e.ndjson");
    write_ndjson(&data, &records(400));
    let contract = dir.path().join("c.yaml");
    std::fs::write(&contract, CONTRACT).unwrap();
    let prof = dir.path().join("p.json");
    let out = Command::new(BIN)
        .arg("check")
        .arg(&data)
        .arg("-c")
        .arg(&contract)
        .arg("--profile")
        .arg(&prof)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));

    let human = Command::new(BIN)
        .args(["profile", "show"])
        .arg(&prof)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(
        text.starts_with("events v1.0.0 · model events · 400 rows in 1 run"),
        "{text}"
    );
    let line = |name: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("no {name} line in:\n{text}"))
            .to_string()
    };
    assert!(line("amount").contains("10%"), "{text}");
    assert!(line("currency").contains("USD 75% · EUR 25%"), "{text}");
    assert!(line("day").contains("2026-09-01 → 2026-09-30"), "{text}");

    let json = Command::new(BIN)
        .args(["profile", "show", "--format", "json"])
        .arg(&prof)
        .output()
        .unwrap();
    let s: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let score = s["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "score")
        .unwrap();
    // score cycles through 0, 0.25, …, 4.0.
    assert_eq!(score["numbers"]["min"], 0.0);
    assert_eq!(score["numbers"]["max"], 4.0);
    // The median sits within the sketch's rank error of 2.0.
    let p50 = score["numbers"]["p50"].as_f64().unwrap();
    assert!((1.75..=2.25).contains(&p50), "{p50}");
}

#[test]
fn a_profile_holds_no_string_value() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("people.ndjson");
    std::fs::write(
        &data,
        "{\"email\":\"ada@example.com\",\"name\":\"Ada Lovelace\"}\n\
         {\"email\":\"grace@example.com\",\"name\":\"Grace Hopper\"}\n",
    )
    .unwrap();
    let contract = "covenant: 1\nid: people\nversion: 1.0.0\nmodels:\n  people:\n    fields:\n      email: { type: string, format: email }\n      name: { type: string }\n      tier: { type: string, allowed: [gold, silver] }\n";
    let with_tier = dir.path().join("tiers.ndjson");
    std::fs::write(
        &with_tier,
        "{\"tier\":\"gold\"}\n{\"tier\":\"ada.secret@example.com\"}\n",
    )
    .unwrap();
    let p = profile_of(contract, &[data, with_tier]);
    let json = p.to_json();
    assert!(
        json.contains("gold"),
        "the contract's own values are counted"
    );
    assert_eq!(p.fields["tier"].values.as_ref().unwrap().other, 1);
    for secret in ["ada", "Ada", "grace", "Grace", "Hopper", "example.com"] {
        assert!(!json.contains(secret), "{secret} leaked into the profile");
    }
    assert_eq!(p.fields["name"].lengths.as_ref().unwrap().max, 12);
}

#[test]
fn a_span_outside_years_0000_to_9999_is_left_out_and_the_profile_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("t.ndjson");
    // Valid RFC 3339, but year -1 in UTC, which no RFC 3339 text can hold.
    std::fs::write(
        &data,
        "{\"t\":\"0000-01-01T00:30:00+01:00\",\"u\":\"2024-01-01T00:00:00Z\"}\n\
         {\"t\":\"2024-01-01T00:00:00Z\",\"u\":\"2024-01-02T00:00:00Z\"}\n",
    )
    .unwrap();
    let contract = "covenant: 1\nid: s\nversion: 1.0.0\nmodels:\n  m:\n    fields:\n      t: { type: timestamp }\n      u: { type: timestamp }\n";
    let p = profile_of(contract, &[data]);
    let back = Profile::parse(&p.to_json(), "p.json").expect("a written profile reads back");
    assert!(back.fields["t"].span.is_none());
    assert_eq!(back.fields["t"].present, 2);
    let u = back.fields["u"]
        .span
        .as_ref()
        .expect("an ordinary span is kept");
    assert_eq!(
        (u.min.as_str(), u.max.as_str()),
        ("2024-01-01T00:00:00Z", "2024-01-02T00:00:00Z")
    );

    // Merges stay exact: a profile without the span, merged with one that
    // has it, is the profile of one run over both files.
    let more = dir.path().join("more.ndjson");
    std::fs::write(
        &more,
        "{\"t\":\"2025-06-01T00:00:00Z\",\"u\":\"2025-06-01T00:00:00Z\"}\n",
    )
    .unwrap();
    let mut merged = p.clone();
    merged
        .merge(&profile_of(contract, std::slice::from_ref(&more)))
        .unwrap();
    let one_run = profile_of(contract, &[dir.path().join("t.ndjson"), more]);
    assert!(merged.fields["t"].span.is_none());
    assert_eq!(merged.fields, one_run.fields);
}

#[test]
fn unreadable_lines_and_wrong_types_are_counted_apart() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("e.ndjson");
    std::fs::write(
        &data,
        "{\"id\":\"a\",\"amount\":5}\nnot json\n{\"id\":\"b\",\"amount\":\"seven\"}\n[1,2]\n",
    )
    .unwrap();
    let p = profile_of(CONTRACT, &[data]);
    assert_eq!((p.rows, p.unreadable), (4, 2));
    let amount = &p.fields["amount"];
    assert_eq!((amount.rows, amount.present, amount.invalid), (2, 2, 1));
    let numbers = amount.numbers.as_ref().unwrap();
    assert_eq!((numbers.count, numbers.min, numbers.max), (1, 5.0, 5.0));
    // Absent keys are missing, not null.
    assert_eq!(p.fields["score"].missing_rate(), Some(1.0));
    assert_eq!(p.fields["score"].nulls, 0);
}

#[test]
fn float32_columns_profile_as_their_decimal_values() {
    let dir = tempfile::tempdir().unwrap();
    let pq = dir.path().join("f.parquet");
    let batch = RecordBatch::try_from_iter(vec![(
        "score",
        Arc::new(Float32Array::from(vec![0.1f32, 0.2, f32::NAN])) as ArrayRef,
    )])
    .unwrap();
    let file = std::fs::File::create(&pq).unwrap();
    let mut w = parquet::arrow::ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let contract = "covenant: 1\nid: f\nversion: 1.0.0\nmodels:\n  f:\n    fields:\n      score: { type: float }\n";
    let p = profile_of(contract, &[pq]);
    let score = &p.fields["score"];
    let n = score.numbers.as_ref().unwrap();
    assert_eq!((n.min, n.max), (0.1, 0.2));
    assert_eq!(score.non_finite, 1);
}

#[test]
fn merging_refuses_other_models_and_changed_types() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("e.ndjson");
    write_ndjson(&data, &records(10));
    let base = profile_of(CONTRACT, std::slice::from_ref(&data));

    let other_id = profile_of(
        &CONTRACT.replace("id: events", "id: other"),
        std::slice::from_ref(&data),
    );
    let err = base.clone().merge(&other_id).unwrap_err().to_string();
    assert!(
        err.contains("profiles merge only within one contract model"),
        "{err}"
    );

    let retyped = profile_of(
        &CONTRACT.replace("score:    { type: float }", "score:    { type: string }"),
        std::slice::from_ref(&data),
    );
    let err = base.clone().merge(&retyped).unwrap_err().to_string();
    assert!(
        err.contains("\"score\" is float in one profile and string in the other"),
        "{err}"
    );

    // A later contract version wins, and a field it added is carried over.
    let v2 = profile_of(
        &CONTRACT
            .replace("version: 1.0.0", "version: 1.1.0")
            .replace(
                "      ref:      { type: uuid }\n",
                "      ref:      { type: uuid }\n      note:     { type: string }\n",
            ),
        std::slice::from_ref(&data),
    );
    let mut merged = base.clone();
    merged.merge(&v2).unwrap();
    assert_eq!(merged.contract_version, "1.1.0");
    assert_eq!(merged.fields["note"].rows, 10);
    assert_eq!(merged.fields["id"].rows, 20);
}

#[test]
fn unreadable_profiles_are_refused() {
    let err = |text: &str| Profile::parse(text, "p.json").unwrap_err().to_string();
    assert!(err("{}").contains("no `profile:` format key"));
    assert!(err("{\"profile\": 2}").contains("profile format 2 is not supported"));
    let impossible = r#"{"profile":1,"contract":"c","contract_version":"1.0.0","model":"m","runs":1,"rows":1,
        "fields":{"a":{"type":"string","rows":1,"present":3,"nulls":0}}}"#;
    assert!(err(impossible).contains("present exceeds rows"));
    let e = run(&["profile", "show", "/nonexistent/p.json"]);
    assert_eq!(e.status.code(), Some(2));
}
