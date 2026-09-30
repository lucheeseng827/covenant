//! The `/v1` API contract the console depends on, driven through the router
//! without a socket. Compiled only with `--features serve`.
#![cfg(feature = "serve")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use covenant::compile::CompiledContract;
use covenant::serve::{router, AppState};
use covenant::spec::Contract;
use tower::ServiceExt;

const CONTRACT: &str = r#"
covenant: 1
id: orders
version: 1.2.0
owner: data-platform@acme.io
models:
  orders:
    strict: true
    fields:
      order_id: { type: string, required: true, unique: true, pattern: "^ord_[a-z0-9]{4}$" }
      amount:   { type: integer, required: true, min: 0, max: 100 }
      currency: { type: string, allowed: [USD, EUR] }
"#;

fn app() -> axum::Router {
    app_with(None, None)
}

fn app_with(
    dlq_path: Option<std::path::PathBuf>,
    gate_stats_path: Option<std::path::PathBuf>,
) -> axum::Router {
    let doc = Contract::parse(CONTRACT, "<test>").unwrap();
    let compiled = CompiledContract::compile(&doc).unwrap();
    router(Arc::new(AppState {
        source: "<test>".into(),
        model: compiled.resolve_model(None).unwrap().name.clone(),
        doc,
        compiled,
        unenforced: Vec::new(),
        dlq_path,
        gate_stats_path,
    }))
}

/// An ODCS contract with one rule the runtime cannot enforce (the `sql` check).
const ODCS_PARTIAL: &str = r#"
apiVersion: v3.2.0
kind: DataContract
id: orders
version: 1.2.0
schema:
  - name: orders
    properties:
      - name: order_id
        logicalType: string
        required: true
      - name: amount
        logicalType: integer
        logicalTypeOptions: { minimum: 0 }
        quality:
          - type: sql
            query: SELECT COUNT(*) FROM orders WHERE amount > 1000000
            mustBe: 0
"#;

/// What `covenant serve --allow-unenforced` builds from `ODCS_PARTIAL`.
fn partial_app() -> axum::Router {
    let loaded = Contract::load(ODCS_PARTIAL, "<test>").unwrap();
    assert_eq!(loaded.unenforced.len(), 1);
    let compiled = CompiledContract::compile(&loaded.contract).unwrap();
    router(Arc::new(AppState {
        source: "<test>".into(),
        model: compiled.resolve_model(None).unwrap().name.clone(),
        doc: loaded.contract,
        compiled,
        unenforced: loaded.unenforced,
        dlq_path: None,
        gate_stats_path: None,
    }))
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_reports_contract_and_version() {
    let resp = app()
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["status"], "ok");
    assert_eq!(j["contract"], "orders");
    assert!(j["version"].is_string());
}

#[tokio::test]
async fn contract_endpoint_serves_document_and_findings() {
    let resp = app()
        .oneshot(Request::get("/v1/contract").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["contract"]["id"], "orders");
    assert_eq!(
        j["contract"]["models"]["orders"]["fields"]["amount"]["type"],
        "integer"
    );
    assert!(j["findings"].is_array());
}

#[tokio::test]
async fn validate_lints_and_rejects_garbage() {
    let resp = app()
        .oneshot(
            Request::post("/v1/validate")
                .body(Body::from(CONTRACT))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["enforceable"], true);

    let resp = app()
        .oneshot(
            Request::post("/v1/validate")
                .body(Body::from("covenant: [nope"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = body_json(resp).await;
    assert!(j["error"].is_string());
}

#[tokio::test]
async fn check_returns_a_full_report() {
    let ndjson = "{\"order_id\":\"ord_ab12\",\"amount\":10}\n{\"order_id\":\"ord_ab12\",\"amount\":-5,\"currency\":\"JPY\"}\n";
    let resp = app()
        .oneshot(Request::post("/v1/check").body(Body::from(ndjson)).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["rows"], 2);
    // duplicate id + min + allowed
    assert_eq!(j["violations"], 3);
    let rules: Vec<&str> = j["per_rule"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rule"].as_str().unwrap())
        .collect();
    assert!(rules.contains(&"unique") && rules.contains(&"min") && rules.contains(&"allowed"));
    assert!(j["samples"].as_array().unwrap().len() >= 3);
}

#[tokio::test]
async fn a_fully_enforced_check_carries_no_unenforced_list() {
    let resp = app()
        .oneshot(
            Request::post("/v1/check")
                .body(Body::from("{\"order_id\":\"ord_ab12\",\"amount\":10}\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    let j = body_json(resp).await;
    assert!(j.get("unenforced").is_none(), "{j}");
}

#[tokio::test]
async fn a_server_running_without_some_rules_marks_every_check_partial() {
    let resp = partial_app()
        .oneshot(
            Request::post("/v1/check")
                .body(Body::from("{\"order_id\":\"a\",\"amount\":-1}\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["violations"], 1, "the enforced rules still run: {j}");
    let unenforced = j["unenforced"]
        .as_array()
        .expect("a partial report lists them");
    assert_eq!(unenforced.len(), 1);
    assert_eq!(
        unenforced[0]["path"],
        "schema.orders.properties.amount.quality[0]"
    );

    let resp = partial_app()
        .oneshot(Request::get("/v1/contract").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let j = body_json(resp).await;
    assert_eq!(j["unenforced"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn validate_reads_odcs_and_reports_what_it_cannot_enforce() {
    let resp = app()
        .oneshot(
            Request::post("/v1/validate")
                .body(Body::from(ODCS_PARTIAL))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["enforceable"], false);
    let errors: Vec<&str> = j["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["level"] == "error")
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(errors, vec!["schema.orders.properties.amount.quality[0]"]);

    let enforceable = ODCS_PARTIAL.replace(
        "        quality:\n          - type: sql\n            query: SELECT COUNT(*) FROM orders WHERE amount > 1000000\n            mustBe: 0\n",
        "",
    );
    assert_ne!(enforceable, ODCS_PARTIAL);
    let resp = app()
        .oneshot(
            Request::post("/v1/validate")
                .body(Body::from(enforceable))
                .unwrap(),
        )
        .await
        .unwrap();
    let j = body_json(resp).await;
    assert_eq!(j["enforceable"], true, "{j}");
}

#[tokio::test]
async fn diff_classifies_and_rejects_garbage() {
    let new_contract = CONTRACT.replace(
        "amount:   { type: integer, required: true, min: 0, max: 100 }\n      ",
        "",
    );
    let body = serde_json::json!({ "old": CONTRACT, "new": new_contract }).to_string();
    let resp = app()
        .oneshot(
            Request::post("/v1/diff")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    let severities: Vec<&str> = j["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["severity"].as_str().unwrap())
        .collect();
    assert!(severities.contains(&"breaking"), "{j}");

    let resp = app()
        .oneshot(
            Request::post("/v1/diff")
                .header("content-type", "application/json")
                .body(Body::from("{\"old\":\"covenant: [\",\"new\":\"x\"}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn diff_attaches_the_blast_radius_when_manifests_are_sent() {
    // amount removed -> breaking (consumers); one manifest declares it,
    // one declares an untouched field.
    let new_contract = CONTRACT
        .replace(
            "amount:   { type: integer, required: true, min: 0, max: 100 }\n      ",
            "",
        )
        .replace("version: 1.2.0", "version: 2.0.0");
    let hit = "consumer: 1\nid: rollup\nowner: fin@acme.io\nconsumes: [{contract: orders, fields: [amount]}]\n";
    let safe = "consumer: 1\nid: alerting\nconsumes: [{contract: orders, fields: [order_id]}]\n";
    let body =
        serde_json::json!({ "old": CONTRACT, "new": new_contract, "consumers": [hit, safe] })
            .to_string();
    let resp = app()
        .oneshot(
            Request::post("/v1/diff")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    let ci = &j["consumer_impact"];
    assert_eq!(ci["manifests"], 2, "{j}");
    assert_eq!(ci["consumers_of_contract"], 2);
    assert_eq!(ci["impacted"][0]["consumer"], "rollup");
    assert_eq!(ci["impacted"][0]["severity"], "breaking");
    assert_eq!(ci["impacted"][0]["fields"][0], "amount");
    assert_eq!(ci["unaffected"][0], "alerting");

    // An invalid manifest is a 400, not a silently smaller blast radius —
    // and the error must say which document is wrong and why.
    let body = serde_json::json!({ "old": CONTRACT, "new": new_contract, "consumers": ["consumer: 1\nid: broken\n"] })
        .to_string();
    let resp = app()
        .oneshot(
            Request::post("/v1/diff")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let j = body_json(resp).await;
    let msg = j["error"].as_str().unwrap();
    assert!(msg.contains("consumer manifest error"), "{msg}");
    assert!(msg.contains("consumes"), "{msg}");
}

#[tokio::test]
async fn consumers_verify_grades_each_manifest_against_the_served_contract() {
    let good =
        "consumer: 1\nid: good\nconsumes: [{contract: orders, fields: [amount], verified: 1.2.0}]";
    let stale = "consumer: 1\nid: stale\nconsumes: [{contract: orders, fields: [ghost]}]";
    let other = "consumer: 1\nid: other\nconsumes: [{contract: payments, fields: [x]}]";
    let body = serde_json::json!({ "manifests": [good, stale, other] }).to_string();
    let resp = app()
        .oneshot(
            Request::post("/v1/consumers/verify")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["contract"], "orders");
    let results = j["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0]["consumer"], "good");
    assert!(results[0]["findings"].as_array().unwrap().is_empty(), "{j}");
    assert_eq!(results[1]["findings"][0]["level"], "error");
    assert_eq!(results[2]["consumes_contract"], false);

    // Unparseable manifests are a 400, same contract as /v1/diff consumers.
    let body = serde_json::json!({ "manifests": ["consumer: 1\nid: broken\n"] }).to_string();
    let resp = app()
        .oneshot(
            Request::post("/v1/consumers/verify")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // The silent-pass holes are closed like the CLI closes them: an empty
    // list and a set where NOTHING consumes the served contract are 400s,
    // not a clean 200 a misconfigured CI job would pass on forever.
    for manifests in [
        serde_json::json!([]),
        serde_json::json!([
            "consumer: 1\nid: other\nconsumes: [{contract: payments, fields: [x]}]"
        ]),
    ] {
        let body = serde_json::json!({ "manifests": manifests }).to_string();
        let resp = app()
            .oneshot(
                Request::post("/v1/consumers/verify")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let j = body_json(resp).await;
        assert!(
            j["error"].as_str().unwrap().contains("nothing to verify"),
            "{j}"
        );
    }
}

#[tokio::test]
async fn gate_stats_and_dlq_report_their_wiring_honestly() {
    // Unconfigured: configured:false with a hint, never a fake-empty answer.
    for route in ["/v1/gate/stats", "/v1/dlq"] {
        let resp = app()
            .oneshot(Request::get(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["configured"], false, "{route}: {j}");
        assert!(j["hint"].as_str().is_some(), "{route}: {j}");
    }

    // Configured but the files aren't there yet: configured:true + note.
    let resp = app_with(
        Some("/nonexistent/dlq".into()),
        Some("/nonexistent/stats".into()),
    )
    .oneshot(Request::get("/v1/gate/stats").body(Body::empty()).unwrap())
    .await
    .unwrap();
    let j = body_json(resp).await;
    assert_eq!(j["configured"], true);
    assert!(j["note"].as_str().unwrap().contains("not found"), "{j}");
}

#[tokio::test]
async fn gate_stats_and_dlq_serve_the_phase2_files() {
    let dir = tempfile::tempdir().unwrap();
    let stats_path = dir.path().join("stats.json");
    std::fs::write(
        &stats_path,
        r#"{"covenant_gate_stats":1,"contract":"orders","records":10,"passed":8,"blocked":2,"warned":0,"updated_at":"2026-08-18T00:00:00Z","per_rule":[],"recent":[{"records":10,"blocked":2}]}"#,
    )
    .unwrap();
    let dlq_path = dir.path().join("d.ndjson");
    let mut lines = String::new();
    for row in 0..3 {
        lines.push_str(&format!(
            "{{\"contract_id\":\"orders\",\"row\":{row},\"ts\":\"2026-08-18T00:00:0{row}Z\",\"record\":{{}},\"violations\":[]}}\n"
        ));
    }
    lines.push_str("{\"row\":99}\n"); // parses, but is not envelope-shaped
    lines.push_str("{\"torn\": "); // a concurrent append caught mid-write
    std::fs::write(&dlq_path, lines).unwrap();

    let resp = app_with(Some(dlq_path.clone()), Some(stats_path.clone()))
        .oneshot(Request::get("/v1/gate/stats").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let j = body_json(resp).await;
    assert_eq!(j["configured"], true);
    assert_eq!(j["stats"]["records"], 10, "{j}");
    assert!(
        j["age_seconds"].as_i64().unwrap() > 0,
        "snapshot is old: {j}"
    );

    let resp = app_with(Some(dlq_path), Some(stats_path))
        .oneshot(Request::get("/v1/dlq?limit=2").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let j = body_json(resp).await;
    assert_eq!(j["configured"], true);
    let entries = j["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{j}");
    // Newest first; the torn trailing line AND the non-envelope JSON line
    // are counted in `skipped`, never served — a stray line in the file
    // must not reach (and crash) the console.
    assert_eq!(entries[0]["row"], 2);
    assert_eq!(entries[1]["row"], 1);
    assert_eq!(j["skipped"], 2, "{j}");
}

#[tokio::test]
async fn dlq_tail_survives_a_seek_that_splits_a_multibyte_character() {
    // The bounded tail read seeks to len - 1 MiB. Engineer the file so that
    // offset lands INSIDE a two-byte UTF-8 character in the padding line: a
    // strict read_to_string would fail the whole request on valid data.
    const TAIL_BYTES: u64 = 1 << 20;
    let mut tail = String::new();
    for row in 0..3 {
        tail.push_str(&format!(
            "{{\"row\":{row},\"ts\":\"2026-08-19T00:00:0{row}Z\",\"record\":{{\"a\":\"é\"}},\"violations\":[]}}\n"
        ));
    }
    // Padding line: {"p":"ééé…é"}\n — the é run starts at byte 6; target the
    // seek at byte 7 (mid-character). Solve 9 + 2N + t = TAIL_BYTES + 7.
    let mut t = tail.len() as u64;
    if !(TAIL_BYTES + 7 - 9 - t).is_multiple_of(2) {
        tail.insert(0, '\n'); // blank line; the reader skips it
        t += 1;
    }
    let n = (TAIL_BYTES + 7 - 9 - t) / 2;
    let mut content = String::with_capacity((TAIL_BYTES + 7) as usize);
    content.push_str("{\"p\":\"");
    content.extend(std::iter::repeat_n('é', n as usize));
    content.push_str("\"}\n");
    content.push_str(&tail);
    assert_eq!(content.len() as u64, TAIL_BYTES + 7, "boundary math");

    let dir = tempfile::tempdir().unwrap();
    let dlq_path = dir.path().join("big.ndjson");
    std::fs::write(&dlq_path, content).unwrap();

    let resp = app_with(Some(dlq_path), None)
        .oneshot(
            Request::get("/v1/dlq?limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["configured"], true, "{j}");
    let entries = j["entries"].as_array().expect("must serve, not error");
    assert_eq!(entries.len(), 3, "{j}");
    assert_eq!(entries[0]["row"], 2);
    assert_eq!(
        j["skipped"], 0,
        "the split padding line is dropped, not counted: {j}"
    );
}

#[tokio::test]
async fn console_spa_and_assets_are_embedded() {
    let resp = app()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes).to_string();
    assert!(html.contains("Covenant Console"));

    // The SPA entry references a hashed bundle under ./assets/ — it must be
    // resolvable through the asset route.
    let asset_path = html
        .split("./assets/")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("index.html references a build asset");
    let resp = app()
        .oneshot(
            Request::get(format!("/assets/{asset_path}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "asset {asset_path} must be embedded"
    );
}
