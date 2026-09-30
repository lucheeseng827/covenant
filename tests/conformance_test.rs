//! The ODCS conformance vectors, run through this engine, and the runner's
//! own guarantees: a wrong expectation is caught, a malformed vector refused.

use std::path::{Path, PathBuf};
use std::process::Command;

use covenant::conformance::{load, load_all, run, Outcome, SuiteReport};

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

fn suite() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/odcs")
}

#[test]
fn covenant_conforms_on_every_vector_it_supports() {
    let report = SuiteReport::run(&load_all(&[suite()]).unwrap());
    assert!(report.vectors >= 36, "{}", report.vectors);
    assert!(report.conforms(), "{}", report.render_human());
    // What Covenant cannot enforce yet, tracked: a new entry is a regression,
    // a vanished one is progress to record here and in docs/ODCS.md.
    let unsupported: Vec<&str> = report
        .results
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Unsupported { .. }))
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(
        unsupported,
        vec![
            "options/integer-multiple-of",
            "options/number-exclusive-maximum",
            "quality/duplicate-values-composite",
            "quality/null-values-below-threshold-passes",
            "quality/null-values-percent",
            "quality/row-count",
        ]
    );
}

#[test]
fn every_vector_is_named_after_its_path() {
    for (path, v) in load_all(&[suite()]).unwrap() {
        let rel = path.strip_prefix(suite()).unwrap().with_extension("");
        assert_eq!(v.id, rel.to_string_lossy(), "{}", path.display());
        assert_eq!(v.standard, "ODCS v3.1.0", "{}", path.display());
    }
}

#[test]
fn a_wrong_expectation_is_caught() {
    let mut v = load(&suite().join("types/integer-values.yaml")).unwrap();
    v.expect.failing = Some(vec![2]);
    match run(&v) {
        Outcome::Fail {
            got_failing,
            expected_failing,
            ..
        } => {
            assert_eq!(got_failing, vec![2, 3]);
            assert_eq!(expected_failing, Some(vec![2]));
        }
        other => panic!("{other:?}"),
    }
    let mut v = load(&suite().join("properties/unique-distinct-passes.yaml")).unwrap();
    v.records.push(serde_json::json!({ "v": "a" }));
    assert_eq!(run(&v).label(), "FAIL");
}

#[test]
fn malformed_vectors_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let good = std::fs::read_to_string(suite().join("nulls/required-forbids-null.yaml")).unwrap();
    for (name, text, says) in [
        (
            "format.yaml",
            good.replace("vector: 1", "vector: 2"),
            "vector format 2",
        ),
        (
            "range.yaml",
            good.replace("failing: [1]", "failing: [9]"),
            "record 9, which does not exist",
        ),
        (
            "pass.yaml",
            good.replace("verdict: fail", "verdict: pass"),
            "a passing verdict cannot name failing records",
        ),
        (
            "extra.yaml",
            format!("{good}surprise: true\n"),
            "unknown field",
        ),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(err.contains(says), "{name}: {err}");
    }
}

#[test]
fn the_cli_exits_by_conformance() {
    let ok = Command::new(BIN)
        .arg("conformance")
        .arg(suite())
        .output()
        .unwrap();
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ok.stdout)
    );
    let text = String::from_utf8_lossy(&ok.stdout);
    assert!(
        text.contains("UNSUPPORTED  quality/row-count — schema.t.quality[0] (library rowCount)"),
        "{text}"
    );
    assert!(text.trim_end().ends_with("0 fail, 6 unsupported"), "{text}");

    let dir = tempfile::tempdir().unwrap();
    let wrong = std::fs::read_to_string(suite().join("types/boolean-values.yaml"))
        .unwrap()
        .replace("failing: [2, 3]", "failing: [2]");
    std::fs::write(dir.path().join("wrong.yaml"), wrong).unwrap();
    let failed = Command::new(BIN)
        .args(["conformance", "--format", "json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(report["results"][0]["outcome"], "fail");
    assert_eq!(
        report["results"][0]["got_failing"],
        serde_json::json!([2, 3])
    );

    std::fs::write(dir.path().join("broken.yaml"), "vector: 1\n").unwrap();
    let broken = Command::new(BIN)
        .arg("conformance")
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(broken.status.code(), Some(2));
}
