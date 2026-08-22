//! `covenant infer` — the on-ramp. The load-bearing property is the
//! ROUND TRIP: whatever infer drafts must parse, compile, validate clean,
//! and accept the very data it was drafted from. Everything else here is
//! about honesty — a draft may not promise something the sample cannot
//! support.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray, TimestampSecondArray};
use covenant::compile::CompiledContract;
use covenant::infer::{infer_paths, Confidence, InferOptions};
use covenant::spec::{Contract, FieldType, LintLevel, StringFormat};

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// 40 clean-ish order records: a keyed id, an enum, a uuid, a timestamp,
/// an always-present-but-sometimes-null column, and a sometimes-absent one.
fn orders_ndjson() -> String {
    let statuses = ["pending", "shipped", "cancelled"];
    let mut lines = Vec::new();
    for i in 0..40 {
        let mut fields = vec![
            format!("\"order_id\": \"ord_{i:04}\""),
            format!("\"email\": \"user{i}@acme.io\""),
            format!("\"amount\": {}.5", 10 + i),
            format!("\"status\": \"{}\"", statuses[i % 3]),
            format!("\"trace\": \"550e8400-e29b-41d4-a716-4466554400{:02}\"", i % 100),
            format!("\"created_at\": \"2026-08-{:02}T10:00:00Z\"", (i % 28) + 1),
            format!("\"note\": {}", if i % 5 == 0 { "null".into() } else { format!("\"n{i}\"") }),
        ];
        if i % 8 != 0 {
            fields.push(format!("\"qty\": {}", (i % 9) + 1));
        }
        lines.push(format!("{{{}}}", fields.join(", ")));
    }
    lines.join("\n") + "\n"
}

fn draft_from(dir: &Path, data: &str) -> covenant::infer::Draft {
    let path = write(dir, "orders.ndjson", data);
    infer_paths(&[path], &InferOptions::default()).unwrap()
}

/// The property the whole feature rests on: a draft must reparse, compile,
/// and accept the very data it was drafted from.
fn assert_round_trips(draft: &covenant::infer::Draft, data: &Path) {
    let yaml = draft.to_yaml();
    let reparsed = Contract::parse(&yaml, "<draft>").expect("draft must parse");
    let compiled = CompiledContract::compile(&reparsed).expect("draft must compile");
    let model = compiled.resolve_model(None).unwrap();
    let report = covenant::sources::check_path(&compiled, model, data, None).unwrap();
    assert_eq!(report.violations, 0, "{:#?}\n{yaml}", report.per_rule);
}

#[test]
fn radix_looking_strings_survive_the_yaml_round_trip() {
    // YAML resolves 0x/0o/0b scalars as integers, so an enum member like
    // "0x2A" emitted plain would come back as 42 and never match the data.
    let dir = tempfile::tempdir().unwrap();
    let values = ["0x2A", "0o17", "0b101", "0xZZ", "1_000"];
    let data: String = (0..40)
        .map(|i| format!("{{\"code\": \"{}\"}}\n", values[i % values.len()]))
        .collect();
    let path = write(dir.path(), "codes.ndjson", &data);
    let draft = infer_paths(std::slice::from_ref(&path), &InferOptions::default()).unwrap();

    let yaml = draft.to_yaml();
    let reparsed = Contract::parse(&yaml, "<draft>").expect("must reparse");
    let allowed = reparsed.models["codes"].fields["code"].allowed.as_ref().unwrap();
    for v in values {
        assert!(
            allowed.contains(&serde_json::Value::String(v.to_string())),
            "{v} must survive as a string:\n{yaml}"
        );
    }
    // And the data still passes its own draft.
    assert_round_trips(&draft, &path);
}

#[test]
fn a_draft_always_parses_compiles_and_accepts_its_own_sample() {
    // The whole feature is worthless if the draft needs hand-repair before
    // `validate` will even read it — so this is the first test.
    let dir = tempfile::tempdir().unwrap();
    let data = orders_ndjson();
    let data_path = write(dir.path(), "orders.ndjson", &data);
    let draft = infer_paths(std::slice::from_ref(&data_path), &InferOptions::default()).unwrap();

    let yaml = draft.to_yaml();
    let reparsed = Contract::parse(&yaml, "<draft>").expect("draft must parse");
    let compiled = CompiledContract::compile(&reparsed).expect("draft must compile");

    // The only lint finding permitted is the missing owner, which the draft
    // deliberately leaves as a TODO for a human. Asserting the WHOLE set (not
    // just "no errors") means a new warning cannot slip in unnoticed.
    let findings = reparsed.lint();
    assert_eq!(findings.len(), 1, "{findings:#?}\n{yaml}");
    assert_eq!(findings[0].level, LintLevel::Warning, "{findings:#?}");
    assert_eq!(findings[0].path, "owner", "{findings:#?}");

    // And the data it was drafted from must pass its own contract.
    let model = compiled.resolve_model(None).unwrap();
    let report =
        covenant::sources::check_path(&compiled, model, &data_path, None).unwrap();
    assert_eq!(report.violations, 0, "{:#?}\n{yaml}", report.per_rule);
}

#[test]
fn types_come_from_what_the_values_actually_are() {
    let dir = tempfile::tempdir().unwrap();
    let draft = draft_from(dir.path(), &orders_ndjson());
    let model = &draft.contract.models["orders"];

    assert_eq!(model.fields["order_id"].ty, FieldType::String);
    assert_eq!(model.fields["amount"].ty, FieldType::Float);
    assert_eq!(model.fields["qty"].ty, FieldType::Integer);
    assert_eq!(model.fields["trace"].ty, FieldType::Uuid);
    assert_eq!(model.fields["created_at"].ty, FieldType::Timestamp);
    assert_eq!(model.fields["email"].ty, FieldType::String);
    assert_eq!(model.fields["email"].format, Some(StringFormat::Email));
}

#[test]
fn presence_and_nullability_are_orthogonal() {
    let dir = tempfile::tempdir().unwrap();
    let draft = draft_from(dir.path(), &orders_ndjson());
    let model = &draft.contract.models["orders"];

    // Always present, never null.
    assert!(model.fields["order_id"].required);
    assert!(!model.fields["order_id"].nullable);
    // Always present, sometimes null — BOTH, since the spec treats presence
    // and nullness as separate promises.
    assert!(model.fields["note"].required, "present in every record");
    assert!(model.fields["note"].nullable, "but sometimes null");
    // Sometimes absent — optional, and the draft says how often.
    assert!(!model.fields["qty"].required);
    assert!(draft
        .notes
        .iter()
        .any(|n| n.path.ends_with("fields.qty") && n.message.contains("absent from")));
}

#[test]
fn guesses_from_a_window_are_marked_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let draft = draft_from(dir.path(), &orders_ndjson());
    let model = &draft.contract.models["orders"];

    // The enum, the pattern, and the range are all drafted…
    assert_eq!(
        model.fields["status"].allowed.as_ref().unwrap().len(),
        3,
        "three statuses in the sample"
    );
    assert_eq!(model.fields["order_id"].pattern.as_deref(), Some("^ord_[0-9]{4}$"));
    assert!(model.fields["amount"].min.is_some() && model.fields["amount"].max.is_some());

    // …and every one of them carries a confirm note naming its evidence.
    for suffix in ["status.allowed", "order_id.pattern", "amount.min/max"] {
        let note = draft
            .notes
            .iter()
            .find(|n| n.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("no note for {suffix}: {:#?}", draft.notes));
        assert_eq!(note.confidence, Confidence::Confirm, "{suffix}");
    }
    let yaml = draft.to_yaml();
    assert!(yaml.contains("# confirm:"), "{yaml}");
}

#[test]
fn uniqueness_is_suggested_but_never_promised() {
    // A sample can show every value was distinct; it can never prove the
    // next one will be. Drafting `unique: true` from that would fail clean
    // data in production, so it stays a note.
    let dir = tempfile::tempdir().unwrap();
    let draft = draft_from(dir.path(), &orders_ndjson());
    assert!(!draft.contract.models["orders"].fields["order_id"].unique);
    let note = draft
        .notes
        .iter()
        .find(|n| n.path.ends_with("order_id.unique"))
        .expect("a distinct-values note");
    assert!(note.message.contains("cannot prove uniqueness"), "{}", note.message);
}

#[test]
fn a_mixed_column_widens_to_string_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let data = "{\"v\": 1}\n{\"v\": \"two\"}\n{\"v\": 3}\n";
    let draft = draft_from(dir.path(), data);
    assert_eq!(draft.contract.models["orders"].fields["v"].ty, FieldType::String);
    let note = draft
        .notes
        .iter()
        .find(|n| n.message.contains("mixed value types"))
        .expect("mixed-type note");
    assert!(note.message.contains("number×2"), "{}", note.message);
    assert!(note.message.contains("string×1"), "{}", note.message);
}

#[test]
fn integral_floats_draft_as_integer_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    // JSON writes 2.0 as a float token, but nothing here has a fraction.
    let draft = draft_from(dir.path(), "{\"n\": 2.0}\n{\"n\": 3.0}\n");
    assert_eq!(draft.contract.models["orders"].fields["n"].ty, FieldType::Integer);
    assert!(draft.notes.iter().any(|n| n.message.contains("written as floats")));

    // A real fraction anywhere makes it a float.
    let draft = draft_from(dir.path(), "{\"n\": 2.0}\n{\"n\": 3.5}\n");
    assert_eq!(draft.contract.models["orders"].fields["n"].ty, FieldType::Float);
}

#[test]
fn an_all_null_column_is_a_guess_and_admits_it() {
    let dir = tempfile::tempdir().unwrap();
    let draft = draft_from(dir.path(), "{\"x\": null}\n{\"x\": null}\n");
    let field = &draft.contract.models["orders"].fields["x"];
    assert_eq!(field.ty, FieldType::String, "nothing to go on — widest type");
    assert!(field.nullable);
    assert!(draft
        .notes
        .iter()
        .any(|n| n.message.contains("null or absent") && n.confidence == Confidence::Confirm));
}

#[test]
fn nested_data_is_flagged_rather_than_silently_flattened() {
    let dir = tempfile::tempdir().unwrap();
    let draft = draft_from(dir.path(), "{\"payment\": {\"method\": \"card\"}}\n");
    let note = draft
        .notes
        .iter()
        .find(|n| n.message.contains("nested data"))
        .expect("nested note");
    assert!(note.message.contains("objects"), "{}", note.message);
    // Still round-trips: a flagged field is a string, not a parse error.
    Contract::parse(&draft.to_yaml(), "<draft>").unwrap();
}

#[test]
fn a_stable_shape_drafts_strict_and_a_moving_one_does_not() {
    let dir = tempfile::tempdir().unwrap();
    // Same keys every record → strict on (this is what catches a producer
    // silently adding a field).
    let stable = draft_from(dir.path(), "{\"a\": 1}\n{\"a\": 2}\n");
    assert!(stable.contract.models["orders"].strict);

    // Optional fields coming and going → strict off, with the count of
    // distinct shapes (NOT "differs from the first record", which is
    // meaningless when record one is the minority shape).
    let moving = draft_from(dir.path(), "{\"a\": 1}\n{\"a\": 2, \"b\": 3}\n");
    assert!(!moving.contract.models["orders"].strict);
    let note = moving
        .notes
        .iter()
        .find(|n| n.path.ends_with(".strict"))
        .expect("strict note");
    assert!(note.message.contains("2 different field sets"), "{}", note.message);
}

#[test]
fn enums_need_repetition_not_just_few_values() {
    let dir = tempfile::tempdir().unwrap();
    // 3 records, 3 distinct values: too thin to call a closed set.
    let thin = draft_from(dir.path(), "{\"s\": \"a\"}\n{\"s\": \"b\"}\n{\"s\": \"c\"}\n");
    assert!(thin.contract.models["orders"].fields["s"].allowed.is_none());

    // 40 records over 2 values: now the set looks real.
    let repeated: String = (0..40)
        .map(|i| format!("{{\"s\": \"{}\"}}\n", if i % 2 == 0 { "on" } else { "off" }))
        .collect();
    let thick = draft_from(dir.path(), &repeated);
    assert_eq!(
        thick.contract.models["orders"].fields["s"].allowed.as_ref().unwrap().len(),
        2
    );
}

#[test]
fn dirty_input_teaches_nothing_but_never_kills_the_run() {
    // Inference over messy data is the NORMAL case — a bad line is skipped
    // and counted, not a crash.
    let dir = tempfile::tempdir().unwrap();
    let draft = draft_from(
        dir.path(),
        "{\"a\": 1}\nnot json at all\n[1,2,3]\n\n{\"a\": 2}\n",
    );
    assert_eq!(draft.sampled, 2, "only the two objects counted");
    assert!(draft.notes.iter().any(|n| n.message.contains("not JSON objects")));
}

#[test]
fn the_sample_window_is_honoured_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let data: String = (0..100).map(|i| format!("{{\"a\": {i}}}\n")).collect();
    let path = write(dir.path(), "orders.ndjson", &data);
    let opts = InferOptions { sample: 10, ..InferOptions::default() };
    let draft = infer_paths(std::slice::from_ref(&path), &opts).unwrap();
    assert_eq!(draft.sampled, 10);
    assert!(draft.truncated);
    assert!(draft.to_yaml().contains("sample truncated"));

    // sample: 0 means everything.
    let all = infer_paths(&[path], &InferOptions { sample: 0, ..InferOptions::default() }).unwrap();
    assert_eq!(all.sampled, 100);
    assert!(!all.truncated);
}

#[test]
fn hostile_field_names_and_values_stay_quoted() {
    // Field names and enum members come from DATA, so the emitter must
    // never let one reparse as something else.
    let dir = tempfile::tempdir().unwrap();
    let mut lines = String::new();
    for _ in 0..25 {
        lines.push_str("{\"weird key: yes\": \"true\", \"n\": \"12345\"}\n");
    }
    let draft = draft_from(dir.path(), &lines);
    let yaml = draft.to_yaml();
    let reparsed = Contract::parse(&yaml, "<draft>").expect("must reparse");
    let model = &reparsed.models["orders"];
    assert!(model.fields.contains_key("weird key: yes"), "{yaml}");
    // "true" and "12345" must come back as STRINGS, not bool/int.
    let allowed = model.fields["weird key: yes"].allowed.as_ref().unwrap();
    assert_eq!(allowed[0], serde_json::Value::String("true".into()), "{yaml}");
    let numeric = model.fields["n"].allowed.as_ref().unwrap();
    assert_eq!(numeric[0], serde_json::Value::String("12345".into()), "{yaml}");
}

#[test]
fn csv_and_parquet_infer_through_the_same_accumulator() {
    let dir = tempfile::tempdir().unwrap();

    // CSV: arrow sniffs the numeric column; the text column still goes
    // through string-shape analysis (that is where uuid/date/enum come from).
    let mut csv = String::from("order_id,amount,status\n");
    for i in 0..30 {
        csv.push_str(&format!(
            "ord_{i:04},{}, {}\n",
            i + 1,
            if i % 2 == 0 { "pending" } else { "shipped" }
        ));
    }
    let csv_path = write(dir.path(), "orders.csv", &csv);
    let draft = infer_paths(std::slice::from_ref(&csv_path), &InferOptions::default()).unwrap();
    assert_eq!(draft.sampled, 30);
    let model = &draft.contract.models["orders"];
    assert_eq!(model.fields["amount"].ty, FieldType::Integer);
    assert_eq!(model.fields["order_id"].ty, FieldType::String);
    assert_round_trips(&draft, &csv_path);

    // Parquet: the file's own schema is authoritative for types.
    let parquet_path = dir.path().join("orders.parquet");
    let ids: Vec<String> = (0..30).map(|i| format!("ord_{i:04}")).collect();
    let batch = RecordBatch::try_from_iter(vec![
        (
            "order_id".to_string(),
            Arc::new(StringArray::from(ids)) as ArrayRef,
        ),
        (
            "amount".to_string(),
            Arc::new(Int64Array::from((0..30).collect::<Vec<i64>>())) as ArrayRef,
        ),
    ])
    .unwrap();
    let file = std::fs::File::create(&parquet_path).unwrap();
    let mut writer = parquet::arrow::ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let draft =
        infer_paths(std::slice::from_ref(&parquet_path), &InferOptions::default()).unwrap();
    assert_eq!(draft.sampled, 30);
    assert_eq!(
        draft.contract.models["orders"].fields["amount"].ty,
        FieldType::Integer
    );
    assert_round_trips(&draft, &parquet_path);
}

#[test]
fn temporal_columns_keep_their_place_in_the_schema() {
    // Arrow temporal cells render to owned strings; if that rendering is
    // deferred to the end of the row, every date/timestamp column sinks to
    // the bottom of the drafted model and the contract stops reading like
    // the data.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.parquet");
    let batch = RecordBatch::try_from_iter(vec![
        (
            "created_at".to_string(),
            Arc::new(TimestampSecondArray::from(vec![1_760_000_000i64; 4])) as ArrayRef,
        ),
        (
            "id".to_string(),
            Arc::new(StringArray::from(vec!["a", "b", "c", "d"])) as ArrayRef,
        ),
    ])
    .unwrap();
    let file = std::fs::File::create(&path).unwrap();
    let mut writer = parquet::arrow::ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let draft = infer_paths(std::slice::from_ref(&path), &InferOptions::default()).unwrap();
    let names: Vec<&str> = draft.contract.models["events"]
        .fields
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, ["created_at", "id"], "schema order must survive");
    assert_eq!(
        draft.contract.models["events"].fields["created_at"].ty,
        FieldType::Timestamp
    );
}

#[test]
fn an_out_of_range_timestamp_does_not_panic_or_invent_a_date() {
    // Inference reads untrusted files: a second-unit timestamp past ~2262
    // overflows i64 on the way to nanoseconds. That must not panic (debug)
    // or wrap into a plausible-looking wrong date (release).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("far_future.parquet");
    let batch = RecordBatch::try_from_iter(vec![(
        "t".to_string(),
        Arc::new(TimestampSecondArray::from(vec![i64::MAX, 1_760_000_000])) as ArrayRef,
    )])
    .unwrap();
    let file = std::fs::File::create(&path).unwrap();
    let mut writer = parquet::arrow::ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let draft = infer_paths(std::slice::from_ref(&path), &InferOptions::default()).unwrap();
    assert_eq!(draft.sampled, 2);
    // The overflowing cell renders empty, so the column is no longer a clean
    // timestamp — it widens to string rather than promising a shape half the
    // values do not have.
    assert_eq!(draft.contract.models["far_future"].fields["t"].ty, FieldType::String);
    Contract::parse(&draft.to_yaml(), "<draft>").unwrap();
}

#[test]
fn cli_drafts_to_stdout_and_to_a_file_it_will_not_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let data = write(dir.path(), "orders.ndjson", &orders_ndjson());

    // stdout, exit 0 — a draft is not a verdict, nothing can have failed.
    let out = Command::new(BIN).args(["infer"]).arg(&data).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let yaml = String::from_utf8_lossy(&out.stdout);
    assert!(yaml.starts_with("# DRAFT contract inferred by `covenant infer`"), "{yaml}");
    Contract::parse(&yaml, "<stdout>").unwrap();

    // --out writes the file…
    let target = dir.path().join("draft.yaml");
    let out = Command::new(BIN)
        .args(["infer"])
        .arg(&data)
        .arg("--out")
        .arg(&target)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(target.exists());

    // …and refuses to clobber it on a second run (same discipline as init).
    let out = Command::new(BIN)
        .args(["infer"])
        .arg(&data)
        .arg("--out")
        .arg(&target)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("refusing to overwrite"));
}

#[test]
fn json_output_carries_the_uncertainty_as_data() {
    let dir = tempfile::tempdir().unwrap();
    let data = write(dir.path(), "orders.ndjson", &orders_ndjson());
    let out = Command::new(BIN)
        .args(["infer", "--format", "json"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(j["contract"]["id"], "orders");
    assert_eq!(j["sampled"], 40);
    let notes = j["notes"].as_array().unwrap();
    assert!(notes.iter().any(|n| n["confidence"] == "confirm"));
    // The embedded contract is a real contract, not a lookalike.
    let contract: Contract = serde_json::from_value(j["contract"].clone()).unwrap();
    CompiledContract::compile(&contract).unwrap();
}

#[test]
fn inferring_from_nothing_is_an_error_not_an_empty_contract() {
    // Silently drafting a contract with no fields would be the worst
    // outcome: it passes every check and guards nothing.
    let dir = tempfile::tempdir().unwrap();
    let empty = write(dir.path(), "empty.ndjson", "\n\n");
    let err = infer_paths(&[empty], &InferOptions::default()).unwrap_err();
    assert!(err.to_string().contains("no records found"), "{err}");

    let err = infer_paths(&[], &InferOptions::default()).unwrap_err();
    assert!(err.to_string().contains("no data files"), "{err}");
}

#[test]
fn options_name_the_contract_and_can_switch_guesses_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "weird-name.ndjson", &orders_ndjson());

    // Default id is the sanitized file stem.
    let draft = infer_paths(std::slice::from_ref(&path), &InferOptions::default()).unwrap();
    assert_eq!(draft.contract.id, "weird_name");

    let opts = InferOptions {
        id: Some("payments".into()),
        model: Some("txn".into()),
        max_enum: 0,
        ranges: false,
        ..InferOptions::default()
    };
    let draft = infer_paths(&[path], &opts).unwrap();
    assert_eq!(draft.contract.id, "payments");
    assert!(draft.contract.models.contains_key("txn"));
    let model = &draft.contract.models["txn"];
    assert!(model.fields["status"].allowed.is_none(), "enums off");
    assert!(model.fields["amount"].min.is_none(), "ranges off");
    Contract::parse(&draft.to_yaml(), "<draft>").unwrap();
}
