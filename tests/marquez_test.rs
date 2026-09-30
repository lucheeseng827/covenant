//! `check --openlineage` against a running catalog: Marquez, the OpenLineage
//! reference implementation. Its API is read back to see what a catalog keeps
//! of a run and shows on the dataset. The test is ignored by default and runs
//! as
//!
//! ```text
//! COVENANT_TEST_MARQUEZ=http://localhost:5000 cargo test --test marquez_test -- --ignored
//! ```
//!
//! against a Marquez on this machine. CI's `marquez` job does exactly that.
//! Every run is a job of its own, so runs can share one Marquez.
#![cfg(feature = "send")]

use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_covenant");
const NEEDS_MARQUEZ: &str = "needs Marquez: COVENANT_TEST_MARQUEZ=http://host:port";

fn marquez() -> String {
    let url = std::env::var("COVENANT_TEST_MARQUEZ").expect(NEEDS_MARQUEZ);
    url.trim_end_matches('/').to_string()
}

/// A Marquez API path read as JSON. Marquez is on this machine, so no proxy
/// the environment names is in the way.
fn get(path: &str) -> Value {
    let config = ureq::Agent::config_builder().proxy(None).build();
    let agent = ureq::Agent::new_with_config(config);
    let url = format!("{}{path}", marquez());
    let mut response = agent
        .get(&url)
        .call()
        .unwrap_or_else(|e| panic!("GET {url}: {e}"));
    serde_json::from_str(&response.body_mut().read_to_string().unwrap()).unwrap()
}

/// A dataset name as one path segment of a Marquez URL.
fn segment(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn check(dir: &Path) -> Output {
    Command::new(BIN)
        .current_dir(dir)
        .args([
            "check",
            "orders.ndjson",
            "-c",
            "contract.yaml",
            "--openlineage",
        ])
        .arg(format!("{}/api/v1/lineage", marquez()))
        .env_remove("OPENLINEAGE_API_KEY")
        .output()
        .unwrap()
}

/// The dataset's assertions as `(column, assertion, held)`, in their order.
fn results(facets: &Value) -> Vec<(String, String, bool)> {
    facets["dataQualityAssertions"]["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let column = a["column"].as_str().unwrap_or_default().to_string();
            let rule = a["assertion"].as_str().unwrap().to_string();
            (column, rule, a["success"].as_bool().unwrap())
        })
        .collect()
}

/// Every rule the contract declares, and whether it held when `currency`'s
/// `allowed` did or did not.
fn declared(allowed_held: bool) -> Vec<(String, String, bool)> {
    [
        ("order_id", "required_missing", true),
        ("order_id", "null_not_allowed", true),
        ("order_id", "type_mismatch", true),
        ("currency", "null_not_allowed", true),
        ("currency", "type_mismatch", true),
        ("currency", "allowed", allowed_held),
    ]
    .map(|(c, r, held)| (c.to_string(), r.to_string(), held))
    .to_vec()
}

#[test]
#[ignore = "needs Marquez: COVENANT_TEST_MARQUEZ=http://host:port"]
fn marquez_shows_each_rules_result_on_the_dataset_and_the_latest_verdict() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // A job no other run has: the job is named after the contract.
    let id = format!("orders-{}-{nanos}", std::process::id());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("contract.yaml"),
        format!(
            "covenant: 1\nid: {id}\nversion: 1.0.0\nmodels:\n  orders:\n    fields:\n      \
             order_id: {{ type: string, required: true }}\n      \
             currency: {{ type: string, allowed: [USD, EUR] }}\n"
        ),
    )
    .unwrap();
    let data = dir.path().join("orders.ndjson");
    std::fs::write(
        &data,
        "{\"order_id\":\"a\",\"currency\":\"USD\"}\n{\"order_id\":\"b\",\"currency\":\"BTC\"}\n",
    )
    .unwrap();

    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("openlineage: START and COMPLETE sent"),
        "{out:?}"
    );

    // The job, its completed run, and the file it read.
    let job = get(&format!(
        "/api/v1/namespaces/covenant/jobs/check.{id}.orders"
    ));
    assert_eq!(job["latestRun"]["state"], "COMPLETED", "{job:#}");
    let name = std::path::absolute(&data).unwrap().display().to_string();
    assert_eq!(job["inputs"][0]["namespace"], "file", "{job:#}");
    assert_eq!(job["inputs"][0]["name"], name.as_str(), "{job:#}");

    // The dataset shows the verdict: each rule's result, and the rows read.
    let dataset = format!("/api/v1/namespaces/file/datasets/{}", segment(&name));
    let facets = get(&dataset)["facets"].clone();
    assert_eq!(results(&facets), declared(false), "{facets:#}");
    assert_eq!(facets["dataQualityMetrics"]["rowCount"], 2, "{facets:#}");
    // The run keeps them on the version of the dataset it read.
    let run = get(&format!(
        "/api/v1/jobs/runs/{}",
        job["latestRun"]["id"].as_str().unwrap()
    ));
    let input = &run["inputDatasetVersions"][0]["facets"];
    assert_eq!(
        input["dataQualityAssertions"], facets["dataQualityAssertions"],
        "{run:#}"
    );

    // Fixed and checked again: the dataset shows the new verdict, and the job
    // both runs.
    std::fs::write(
        &data,
        "{\"order_id\":\"a\",\"currency\":\"USD\"}\n{\"order_id\":\"b\",\"currency\":\"EUR\"}\n\
         {\"order_id\":\"c\",\"currency\":\"EUR\"}\n",
    )
    .unwrap();
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let facets = get(&dataset)["facets"].clone();
    assert_eq!(results(&facets), declared(true), "{facets:#}");
    assert_eq!(facets["dataQualityMetrics"]["rowCount"], 3, "{facets:#}");
    let runs = get(&format!(
        "/api/v1/namespaces/covenant/jobs/check.{id}.orders/runs"
    ));
    let states: Vec<&Value> = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["state"])
        .collect();
    assert_eq!(states, ["COMPLETED", "COMPLETED"], "{runs:#}");
}
