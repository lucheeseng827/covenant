//! `check --openlineage`: a run as OpenLineage START and COMPLETE events,
//! held to the published OpenLineage schemas (copies in
//! `tests/openlineage/`) and sent to a server on this machine.
#![cfg(feature = "send")]

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::server;
use jsonschema::Registry;
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

const CONTRACT: &str = r#"
covenant: 1
id: orders
version: 1.0.0
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true, unique: true, pattern: "^ord_[a-z]+$" }
      currency: { type: string, allowed: [USD, EUR], nullable: true }
"#;

const DIRTY: &str = r#"{"order_id":"ord_a","currency":"USD"}
{"order_id":"ord_b","currency":"BTC"}
"#;

const KEY: &str = "ol_api_key_58c2e1";

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .current_dir(dir)
        .args(args)
        .env("OPENLINEAGE_API_KEY", KEY)
        .env_remove("COVENANT_TOKEN")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("CI")
        .output()
        .unwrap()
}

fn setup(contract: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("contract.yaml"), contract).unwrap();
    std::fs::write(dir.path().join("orders.ndjson"), DIRTY).unwrap();
    dir
}

/// Errors from validating `value` against `reference` (a URI into one of
/// the published schemas).
fn openlineage_errors(value: &Value, reference: &str) -> Vec<String> {
    let load = |name: &str| -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/openlineage")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let registry = Registry::new()
        .add(
            "https://openlineage.io/spec/2-0-2/OpenLineage.json",
            load("OpenLineage-2-0-2.json"),
        )
        .unwrap()
        .add(
            "https://openlineage.io/spec/facets/1-0-2/DataQualityAssertionsDatasetFacet.json",
            load("DataQualityAssertionsDatasetFacet-1-0-2.json"),
        )
        .unwrap()
        .add(
            "https://openlineage.io/spec/facets/1-0-3/DataQualityMetricsInputDatasetFacet.json",
            load("DataQualityMetricsInputDatasetFacet-1-0-3.json"),
        )
        .unwrap()
        .prepare()
        .unwrap();
    let validator = jsonschema::options()
        .with_registry(&registry)
        .should_validate_formats(true)
        .build(&json!({ "$ref": reference }))
        .unwrap();
    validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect()
}

fn assert_openlineage(event: &Value) {
    let errors = openlineage_errors(
        event,
        "https://openlineage.io/spec/2-0-2/OpenLineage.json#/$defs/RunEvent",
    );
    assert!(errors.is_empty(), "{errors:?}\n{event:#}");
    for input in event["inputs"].as_array().unwrap() {
        let Some(facets) = input.get("inputFacets") else {
            continue;
        };
        for (name, facet) in facets.as_object().unwrap() {
            // Each facet names the schema it is held to.
            let schema = facet["_schemaURL"].as_str().unwrap();
            let errors = openlineage_errors(facet, schema);
            assert!(errors.is_empty(), "{name}: {errors:?}\n{facet:#}");
        }
    }
}

fn assertion(event: &Value, rule: &str, column: Option<&str>) -> Value {
    event["inputs"][0]["inputFacets"]["dataQualityAssertions"]["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["assertion"] == rule && a.get("column").and_then(Value::as_str) == column)
        .unwrap_or_else(|| panic!("no {rule} assertion on {column:?}: {event:#}"))
        .clone()
}

#[test]
fn a_check_is_a_start_and_a_complete_with_each_rules_result() {
    let dir = setup(CONTRACT);
    let (url, received) = server(201);
    let out = run(
        dir.path(),
        &[
            "check",
            "orders.ndjson",
            "-c",
            "contract.yaml",
            "--openlineage",
            &url,
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let start = received.recv().unwrap();
    let complete = received.recv().unwrap();
    assert_eq!(
        start.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    let (start, complete) = (start.json(), complete.json());
    for event in [&start, &complete] {
        assert_openlineage(event);
    }
    assert_eq!(start["eventType"], "START");
    assert_eq!(complete["eventType"], "COMPLETE");
    // One run: one id, a UUIDv7.
    let run_id = start["run"]["runId"].as_str().unwrap();
    assert_eq!(complete["run"]["runId"], run_id);
    let v7 =
        regex::Regex::new("^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
            .unwrap();
    assert!(v7.is_match(run_id), "{run_id}");
    assert!(start["eventTime"].as_str() <= complete["eventTime"].as_str());
    assert_eq!(
        complete["job"],
        json!({ "namespace": "covenant", "name": "check.orders.orders" })
    );
    // The file as OpenLineage names it: `file` and its absolute path.
    let input = &complete["inputs"][0];
    assert_eq!(input["namespace"], "file");
    let name = input["name"].as_str().unwrap();
    assert!(Path::new(name).is_absolute() && name.ends_with("orders.ndjson"));
    // Every rule the model declares, by column, and whether it held.
    assert_eq!(
        assertion(&complete, "allowed", Some("currency")),
        json!({ "assertion": "allowed", "success": false, "severity": "error", "column": "currency" })
    );
    assert_eq!(
        assertion(&complete, "required_missing", Some("order_id"))["success"],
        true
    );
    assert_eq!(
        assertion(&complete, "unique", Some("order_id"))["success"],
        true
    );
    assert_eq!(
        assertion(&complete, "unexpected_field", None)["success"],
        true
    );
    assert_eq!(input["inputFacets"]["dataQualityMetrics"]["rowCount"], 2);
    // The schemas are not a formality: an event without its job, or a facet
    // without its assertions, is refused.
    let mut jobless = complete.clone();
    jobless.as_object_mut().unwrap().remove("job");
    assert!(!openlineage_errors(
        &jobless,
        "https://openlineage.io/spec/2-0-2/OpenLineage.json#/$defs/RunEvent"
    )
    .is_empty());
    let facet = &input["inputFacets"]["dataQualityAssertions"];
    let mut empty = facet.clone();
    empty.as_object_mut().unwrap().remove("assertions");
    assert!(!openlineage_errors(&empty, facet["_schemaURL"].as_str().unwrap()).is_empty());
    // No value leaves in an event.
    assert!(!complete.to_string().contains("BTC"));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(said.contains(&format!("openlineage: START and COMPLETE sent → {url}")));
    assert!(!said.contains(KEY));
}

#[test]
fn a_warn_contract_asserts_at_warn_and_a_rule_it_does_not_declare_still_counts() {
    let contract = format!("{CONTRACT}policy:\n  on_violation: warn\n");
    let dir = setup(&contract);
    // A column the strict model does not declare.
    std::fs::write(
        dir.path().join("orders.ndjson"),
        "{\"order_id\":\"ord_a\",\"extra\":1}\n",
    )
    .unwrap();
    let (url, received) = server(200);
    let out = run(
        dir.path(),
        &[
            "check",
            "orders.ndjson",
            "-c",
            "contract.yaml",
            "--openlineage",
            &url,
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    received.recv().unwrap();
    let complete = received.recv().unwrap().json();
    assert_openlineage(&complete);
    let closed = assertion(&complete, "unexpected_field", None);
    assert_eq!(closed["success"], false);
    assert_eq!(closed["severity"], "warn");
}

#[test]
fn events_that_cannot_be_sent_are_one_warning_and_the_exit_code_stands() {
    let dir = setup(CONTRACT);
    let (url, received) = server(500);
    let out = run(
        dir.path(),
        &[
            "check",
            "orders.ndjson",
            "-c",
            "contract.yaml",
            "--openlineage",
            &url,
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    received.recv().unwrap();
    assert!(
        received.try_recv().is_err(),
        "no COMPLETE after a refused START"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: OpenLineage events not sent to") && stderr.contains("HTTP 500"),
        "{stderr}"
    );
}

#[test]
fn a_stream_on_stdin_names_no_dataset() {
    let dir = setup(CONTRACT);
    let out = run(
        dir.path(),
        &[
            "gate",
            "-c",
            "contract.yaml",
            "--openlineage",
            "http://127.0.0.1:9/lineage",
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("gate a topic (--brokers)"));
}
