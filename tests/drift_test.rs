//! Drift: two profiles of one contract model, compared metric by metric —
//! each finding a number, a threshold and a sentence.

use std::process::Command;

use covenant::compile::CompiledContract;
use covenant::drift::{drift, DriftThresholds, Metric};
use covenant::profile::{Profile, Profiler};
use covenant::spec::Contract;
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

const CONTRACT: &str = r#"
covenant: 1
id: orders
version: 1.0.0
models:
  orders:
    fields:
      id:       { type: string }
      amount:   { type: integer }
      currency: { type: string, allowed: [USD, EUR, GBP] }
      email:    { type: string, nullable: true }
"#;

/// Orders `range`, shaped by the knobs: `shift` moves amounts, `usd` and
/// `eur` are shares out of ten (GBP takes the rest), `null_every` puts a null
/// email every n orders, `id_mod` makes ids repeat.
struct Shape {
    shift: i64,
    usd: usize,
    eur: usize,
    null_every: usize,
    id_mod: usize,
}

const NORMAL: Shape = Shape {
    shift: 0,
    usd: 6,
    eur: 3,
    null_every: 100,
    id_mod: usize::MAX,
};

fn orders(range: std::ops::Range<usize>, s: &Shape) -> Vec<Value> {
    range
        .map(|i| {
            let currency = if i % 10 < s.usd {
                "USD"
            } else if i % 10 < s.usd + s.eur {
                "EUR"
            } else {
                "GBP"
            };
            json!({
                "id": format!("o{}", i % s.id_mod),
                "amount": (i as i64 * 37) % 500 + s.shift,
                "currency": currency,
                "email": if i % s.null_every == 0 { Value::Null } else { json!(format!("u{i}@example.com")) },
            })
        })
        .collect()
}

fn profile_of(contract: &str, records: &[Value]) -> Profile {
    let doc = Contract::parse(contract, "c.yaml").unwrap();
    let compiled = CompiledContract::compile(&doc).unwrap();
    let model = compiled.resolve_model(None).unwrap();
    let mut p = Profiler::new(&compiled, model);
    for r in records {
        p.observe_record(model, r);
    }
    p.finish()
}

fn findings(base: &Profile, cur: &Profile) -> Vec<(Option<String>, Metric, String)> {
    drift(base, cur, &DriftThresholds::default())
        .unwrap()
        .findings
        .into_iter()
        .map(|f| (f.field, f.metric, f.message))
        .collect()
}

#[test]
fn the_same_kind_of_data_does_not_drift() {
    let base = profile_of(CONTRACT, &orders(0..2_000, &NORMAL));
    let cur = profile_of(CONTRACT, &orders(2_000..4_000, &NORMAL));
    let report = drift(&base, &cur, &DriftThresholds::default()).unwrap();
    assert!(!report.drifted(), "{:#?}", report.findings);
    assert_eq!(report.compared, vec!["id", "amount", "currency", "email"]);
    assert!(report.render_human().starts_with("STABLE orders/orders"));
}

#[test]
fn a_shifted_numeric_distribution_is_caught_and_explained() {
    let base = profile_of(CONTRACT, &orders(0..2_000, &NORMAL));
    let cur = profile_of(
        CONTRACT,
        &orders(
            0..2_000,
            &Shape {
                shift: 400,
                ..NORMAL
            },
        ),
    );
    let f = findings(&base, &cur);
    assert_eq!(f.len(), 1, "{f:#?}");
    let (field, metric, message) = &f[0];
    assert_eq!(
        (field.as_deref(), *metric),
        (Some("amount"), Metric::Distribution)
    );
    assert!(message.starts_with("values shifted: median "), "{message}");
    assert!(message.contains("PSI"), "{message}");
}

#[test]
fn a_change_in_value_shares_names_the_value_that_moved() {
    let base = profile_of(CONTRACT, &orders(0..2_000, &NORMAL));
    // USD 60 → 20 and EUR 30 → 20: GBP, 10 → 60, moved most.
    let cur = profile_of(
        CONTRACT,
        &orders(
            0..2_000,
            &Shape {
                usd: 2,
                eur: 2,
                ..NORMAL
            },
        ),
    );
    let f = findings(&base, &cur);
    assert_eq!(f.len(), 1, "{f:#?}");
    let (field, metric, message) = &f[0];
    assert_eq!(
        (field.as_deref(), *metric),
        (Some("currency"), Metric::Distribution)
    );
    assert!(
        message.starts_with("\"GBP\" rose from 10% to 60% of values"),
        "{message}"
    );
}

#[test]
fn a_rising_null_rate_is_caught() {
    let base = profile_of(CONTRACT, &orders(0..2_000, &NORMAL));
    let cur = profile_of(
        CONTRACT,
        &orders(
            0..2_000,
            &Shape {
                null_every: 4,
                ..NORMAL
            },
        ),
    );
    let f = findings(&base, &cur);
    assert_eq!(f.len(), 1, "{f:#?}");
    let (field, metric, message) = &f[0];
    assert_eq!(
        (field.as_deref(), *metric),
        (Some("email"), Metric::NullRate)
    );
    assert_eq!(
        message,
        "null rate rose from 1% to 25% (threshold ±5 points)"
    );
}

#[test]
fn repeating_ids_lower_the_distinct_share() {
    let base = profile_of(CONTRACT, &orders(0..5_000, &NORMAL));
    let cur = profile_of(
        CONTRACT,
        &orders(
            0..5_000,
            &Shape {
                id_mod: 2_000,
                ..NORMAL
            },
        ),
    );
    let report = drift(&base, &cur, &DriftThresholds::default()).unwrap();
    assert_eq!(report.findings.len(), 1, "{:#?}", report.findings);
    let f = &report.findings[0];
    assert_eq!(
        (f.field.as_deref(), f.metric),
        (Some("id"), Metric::Distinct)
    );
    // 5,000 of 5,000 distinct, then 2,000 of 5,000, within the sketch's error.
    assert!(
        f.baseline > 0.95 && (0.37..0.43).contains(&f.current),
        "{f:?}"
    );
    assert!(
        f.message.starts_with("distinct values went from "),
        "{}",
        f.message
    );
}

#[test]
fn volume_is_compared_per_run() {
    // A baseline merged from three daily runs of 1,000 rows each.
    let mut base = profile_of(CONTRACT, &orders(0..1_000, &NORMAL));
    for day in 1..3 {
        base.merge(&profile_of(
            CONTRACT,
            &orders(day * 1_000..(day + 1) * 1_000, &NORMAL),
        ))
        .unwrap();
    }
    assert_eq!((base.runs, base.rows), (3, 3_000));
    let same = profile_of(CONTRACT, &orders(3_000..4_000, &NORMAL));
    assert!(findings(&base, &same).is_empty());

    let small = profile_of(CONTRACT, &orders(3_000..3_300, &NORMAL));
    let f = findings(&base, &small);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].1, Metric::Volume);
    assert_eq!(
        f[0].2,
        "rows per run fell from 1,000 to 300 (−70%; threshold ±50%)"
    );
}

#[test]
fn thresholds_decide() {
    let base = profile_of(CONTRACT, &orders(0..2_000, &NORMAL));
    let cur = profile_of(
        CONTRACT,
        &orders(
            0..2_000,
            &Shape {
                shift: 400,
                ..NORMAL
            },
        ),
    );
    let lax = DriftThresholds {
        psi: 100.0,
        ..DriftThresholds::default()
    };
    assert!(!drift(&base, &cur, &lax).unwrap().drifted());
}

#[test]
fn what_cannot_be_judged_is_listed_not_guessed() {
    let base = profile_of(CONTRACT, &orders(0..2_000, &NORMAL));
    // A newer contract adds a field and retypes another.
    let v2 = CONTRACT
        .replace("version: 1.0.0", "version: 1.1.0")
        .replace("amount:   { type: integer }", "amount:   { type: float }")
        .replace(
            "      email:    { type: string, nullable: true }\n",
            "      email:    { type: string, nullable: true }\n      note:     { type: string }\n",
        );
    let cur = profile_of(&v2, &orders(0..50, &NORMAL));
    let report = drift(&base, &cur, &DriftThresholds::default()).unwrap();
    let reasons: Vec<(String, String)> = report
        .not_compared
        .iter()
        .map(|n| (n.field.clone(), n.reason.clone()))
        .collect();
    assert!(reasons.contains(&("amount".into(), "type changed from integer to float".into())));
    assert!(reasons.contains(&("note".into(), "not in the baseline".into())));
    assert!(reasons.contains(&(
        "id".into(),
        "2000 and 50 values: distribution and distinct values need 100 on each side".into()
    )));
    // 50 rows against 2,000 per run is a volume finding; nothing else is judged.
    assert!(
        report.findings.iter().all(|f| f.metric == Metric::Volume),
        "{:#?}",
        report.findings
    );
}

#[test]
fn profiles_of_different_models_do_not_compare() {
    let base = profile_of(CONTRACT, &orders(0..200, &NORMAL));
    let other = profile_of(
        &CONTRACT.replace("id: orders", "id: refunds"),
        &orders(0..200, &NORMAL),
    );
    let err = drift(&base, &other, &DriftThresholds::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("drift compares profiles of one contract model"),
        "{err}"
    );
}

#[test]
fn the_cli_exits_by_the_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, p: &Profile| {
        let path = dir.path().join(name);
        p.write(&path).unwrap();
        path.display().to_string()
    };
    let base = write(
        "base.json",
        &profile_of(CONTRACT, &orders(0..2_000, &NORMAL)),
    );
    let same = write(
        "same.json",
        &profile_of(CONTRACT, &orders(2_000..4_000, &NORMAL)),
    );
    let shifted = write(
        "shifted.json",
        &profile_of(
            CONTRACT,
            &orders(
                0..2_000,
                &Shape {
                    shift: 400,
                    ..NORMAL
                },
            ),
        ),
    );
    let run = |args: &[&str]| Command::new(BIN).args(args).output().unwrap();

    let ok = run(&["drift", &base, &same]);
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ok.stdout)
    );

    let drifted = run(&["drift", &base, &shifted, "--format", "json"]);
    assert_eq!(drifted.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&drifted.stdout).unwrap();
    assert_eq!(report["findings"][0]["field"], "amount");
    assert_eq!(report["findings"][0]["metric"], "distribution");
    assert!(report["findings"][0]["score"].as_f64().unwrap() > 0.25);

    let text = String::from_utf8_lossy(&run(&["drift", &base, &shifted]).stdout).to_string();
    assert!(text.starts_with("DRIFT orders/orders"), "{text}");
    assert!(text.contains("1 finding over 4 fields"), "{text}");

    assert_eq!(
        run(&["drift", &base, &shifted, "--psi", "100"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["drift", &base, &shifted, "--psi", "-1"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&["drift", &base, "/nonexistent.json"]).status.code(),
        Some(2)
    );
}

#[test]
fn new_values_in_a_low_cardinality_field_are_caught() {
    let contract = "covenant: 1\nid: visits\nversion: 1.0.0\nmodels:\n  visits:\n    fields:\n      country: { type: string }\n";
    let visits = |countries: &[&str]| -> Vec<Value> {
        (0..2_000)
            .map(|i| json!({ "country": countries[i % countries.len()] }))
            .collect()
    };
    let base = profile_of(contract, &visits(&["DE", "FR", "US", "GB", "JP", "BR"]));
    let same = profile_of(contract, &visits(&["BR", "JP", "GB", "US", "FR", "DE"]));
    assert!(findings(&base, &same).is_empty());
    let more = profile_of(
        contract,
        &visits(&["DE", "FR", "US", "GB", "JP", "BR", "de", "fr", "Germany"]),
    );
    let f = findings(&base, &more);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].1, Metric::Distinct);
    assert_eq!(
        f[0].2,
        "distinct values rose from 6 to 9 (+50%; threshold ±10%)"
    );
}
