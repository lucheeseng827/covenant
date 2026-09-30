//! `covenant mcp`: the protocol's behaviour, and each tool against the
//! example contracts and data.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use covenant::mcp::handle_line;
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

fn example(p: &str) -> String {
    format!("{}/examples/{p}", env!("CARGO_MANIFEST_DIR"))
}

fn request(id: u64, method: &str, params: Value) -> Value {
    handle_line(
        &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string(),
    )
    .expect("a request gets a reply")
}

/// A `tools/call` result.
fn call(name: &str, args: Value) -> Value {
    let reply = request(7, "tools/call", json!({ "name": name, "arguments": args }));
    assert!(reply.get("error").is_none(), "{reply}");
    reply["result"].clone()
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

/// An ODCS contract with one rule the runtime cannot enforce.
const ODCS_PARTIAL: &str = "apiVersion: v3.2.0\nkind: DataContract\nid: orders\nversion: 1.0.0\n\
    schema:\n  - name: orders\n    properties:\n      - name: id\n        logicalType: string\n        required: true\n\
    \n    quality:\n      - type: sql\n        query: SELECT 1\n        mustBe: 0\n";

#[test]
fn initialize_negotiates_the_protocol_revision() {
    let r = request(1, "initialize", json!({ "protocolVersion": "2025-03-26" }));
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "covenant");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    // A revision it does not speak: it offers its latest; the client decides.
    let r = request(2, "initialize", json!({ "protocolVersion": "2099-01-01" }));
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    let r = request(3, "initialize", json!({}));
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn notifications_get_no_reply_and_mistakes_get_errors() {
    assert!(handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert!(handle_line(r#"{"jsonrpc":"2.0","method":"no/such/notification"}"#).is_none());
    assert_eq!(request(4, "ping", json!({}))["result"], json!({}));
    assert_eq!(
        request(5, "no/such/method", json!({}))["error"]["code"],
        -32601
    );
    let garbage = handle_line("{not json").unwrap();
    assert_eq!(
        (garbage["id"].clone(), garbage["error"]["code"].clone()),
        (Value::Null, json!(-32700))
    );
    let unknown_tool = request(6, "tools/call", json!({ "name": "delete_everything" }));
    assert_eq!(unknown_tool["error"]["code"], -32602);
}

#[test]
fn five_tools_are_listed_with_schemas() {
    let tools = request(1, "tools/list", json!({}))["result"]["tools"].clone();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["validate", "check", "diff", "explain", "drift"]);
    for t in tools.as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(t["description"].as_str().unwrap().len() > 40, "{t}");
    }
}

#[test]
fn validate_lints_files_and_inline_contracts() {
    let ok = call(
        "validate",
        json!({ "contract_path": example("contracts/orders.yaml") }),
    );
    assert_eq!(ok["isError"], false);
    assert_eq!(ok["structuredContent"]["enforceable"], true);

    let partial = call("validate", json!({ "contract": ODCS_PARTIAL }));
    assert_eq!(partial["structuredContent"]["enforceable"], false);
    assert!(
        text(&partial).contains("error: schema.orders.quality[0]: not enforced (sql)"),
        "{}",
        text(&partial)
    );
    let allowed = call(
        "validate",
        json!({ "contract": ODCS_PARTIAL, "allow_unenforced": true }),
    );
    assert_eq!(allowed["structuredContent"]["enforceable"], true);
}

#[test]
fn check_gives_a_verdict_for_files_and_for_records() {
    let contract = example("contracts/orders.yaml");
    let clean = call(
        "check",
        json!({ "contract_path": contract, "data_paths": [example("data/orders.ndjson")] }),
    );
    assert_eq!(
        (
            clean["isError"].clone(),
            clean["structuredContent"]["passed"].clone()
        ),
        (json!(false), json!(true))
    );

    // Failing data is a verdict, not an error.
    let dirty = call(
        "check",
        json!({ "contract_path": contract, "data_paths": [example("data/orders_bad.ndjson")] }),
    );
    assert_eq!(dirty["isError"], false);
    assert_eq!(dirty["structuredContent"]["passed"], false);
    assert!(dirty["structuredContent"]["violations"].as_u64().unwrap() > 0);
    assert!(text(&dirty).contains("verdict: FAIL"), "{}", text(&dirty));

    let records = call(
        "check",
        json!({
            "contract_path": contract,
            "records": [{ "order_id": "ord_abcdefabcdef", "amount_cents": -5, "currency": "JPY",
                          "created_at": "2026-09-01T00:00:00Z" }],
        }),
    );
    let s = &records["structuredContent"];
    assert_eq!(
        (s["passed"].clone(), s["violations"].clone()),
        (json!(false), json!(2))
    );
    assert_eq!(s["reports"][0]["source"], "<records>");
}

#[test]
fn check_honors_a_warn_only_contract_as_the_cli_does() {
    let dir = tempfile::tempdir().unwrap();
    let contract = dir.path().join("warn.yaml");
    std::fs::write(
        &contract,
        "covenant: 1\nid: w\nversion: 1.0.0\nmodels:\n  m:\n    fields:\n      n: { type: integer, min: 0 }\n\
         policy:\n  on_violation: warn\n",
    )
    .unwrap();
    let r = call(
        "check",
        json!({ "contract_path": contract, "records": [{ "n": -1 }, { "n": -2 }] }),
    );
    let s = &r["structuredContent"];
    assert_eq!(
        (
            s["passed"].clone(),
            s["violations"].clone(),
            s["warn_only"].clone()
        ),
        (json!(true), json!(2), json!(true))
    );
    assert!(
        text(&r).contains("verdict: PASS — 2 violations (budget 0; policy is on_violation: warn"),
        "{}",
        text(&r)
    );

    // The CLI reaches the same verdict on the same data.
    let data = dir.path().join("d.ndjson");
    std::fs::write(&data, "{\"n\": -1}\n{\"n\": -2}\n").unwrap();
    let cli = Command::new(BIN)
        .arg("check")
        .arg(&data)
        .arg("-c")
        .arg(&contract)
        .output()
        .unwrap();
    assert_eq!(cli.status.code(), Some(0));
}

#[test]
fn check_refuses_a_partial_contract_unless_allowed() {
    let args = json!({ "contract": ODCS_PARTIAL, "records": [{ "id": "a" }] });
    let refused = call("check", args.clone());
    assert_eq!(refused["isError"], true);
    assert!(
        text(&refused).contains("cannot be enforced yet"),
        "{}",
        text(&refused)
    );

    let mut allowed = args;
    allowed["allow_unenforced"] = json!(true);
    let partial = call("check", allowed);
    assert_eq!(partial["structuredContent"]["partial"], true);
    assert_eq!(partial["structuredContent"]["passed"], true);
    assert!(
        text(&partial).starts_with("PASS (partial)"),
        "{}",
        text(&partial)
    );
}

#[test]
fn diff_classifies_and_names_who_breaks() {
    let r = call(
        "diff",
        json!({
            "old_path": example("contracts/orders.yaml"),
            "new_path": example("contracts/orders_v2.yaml"),
            "consumers_dir": example("consumers"),
        }),
    );
    assert_eq!(r["isError"], false);
    assert_eq!(r["structuredContent"]["max_severity"], "breaking");
    assert!(
        !r["structuredContent"]["report"]["consumer_impact"]["impacted"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        text(&r).starts_with("contract diff: v1.2.0 -> v"),
        "{}",
        text(&r)
    );
}

#[test]
fn explain_says_what_the_data_must_satisfy() {
    let all = call(
        "explain",
        json!({ "contract_path": example("contracts/orders.yaml") }),
    );
    let t = text(&all);
    assert!(t.contains("Any violation fails a check."), "{t}");
    assert!(
        t.contains("Strict: a field not listed here is a violation."),
        "{t}"
    );
    assert!(t.contains("- currency (string): required (the key must be present); never null; one of \"USD\", \"EUR\", \"GBP\""), "{t}");
    assert!(
        t.contains("- created_at (timestamp): an RFC 3339 timestamp"),
        "{t}"
    );

    let one = call(
        "explain",
        json!({ "contract_path": example("contracts/orders.yaml"), "field": "amount_cents" }),
    );
    let fields = one["structuredContent"]["fields"].as_array().unwrap();
    assert_eq!(fields.len(), 1);
    assert!(fields[0]["rules"]
        .as_array()
        .unwrap()
        .contains(&json!("between 0 and 5000000 inclusive")));

    let missing = call(
        "explain",
        json!({ "contract_path": example("contracts/orders.yaml"), "field": "nope" }),
    );
    assert_eq!(missing["isError"], true);

    // ODCS rules it cannot enforce are named, not hidden.
    let odcs = call("explain", json!({ "contract": ODCS_PARTIAL }));
    assert!(
        text(&odcs).contains("Not enforced by Covenant (1 rule"),
        "{}",
        text(&odcs)
    );
}

#[test]
fn drift_compares_profiles_check_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let contract = example("contracts/orders.yaml");
    let (base, cur) = (dir.path().join("base.json"), dir.path().join("cur.json"));
    for (path, data) in [(&base, "data/orders.ndjson"), (&cur, "data/orders.ndjson")] {
        let r = call(
            "check",
            json!({ "contract_path": contract, "data_paths": [example(data)], "profile_path": path.display().to_string() }),
        );
        assert!(text(&r).contains("profile written to"), "{}", text(&r));
    }
    let r = call(
        "drift",
        json!({ "baseline_path": base.display().to_string(), "current_path": cur.display().to_string() }),
    );
    assert_eq!(r["isError"], false);
    assert_eq!(r["structuredContent"]["drifted"], false);
    assert!(text(&r).starts_with("STABLE orders/orders"), "{}", text(&r));

    // Only a profile is ever replaced.
    let precious = dir.path().join("notes.txt");
    std::fs::write(&precious, "keep me").unwrap();
    let refused = call(
        "check",
        json!({ "contract_path": contract, "data_paths": [example("data/orders.ndjson")], "profile_path": precious.display().to_string() }),
    );
    assert_eq!(refused["isError"], true);
    assert!(
        text(&refused).contains("exists and is not a profile"),
        "{}",
        text(&refused)
    );
    assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");

    let bad = call(
        "drift",
        json!({ "baseline_path": base.display().to_string(), "current_path": cur.display().to_string(), "psi": -1 }),
    );
    assert_eq!(bad["isError"], true);
}

#[test]
fn bad_arguments_are_tool_errors_the_agent_can_read() {
    for (tool, args, says) in [
        (
            "validate",
            json!({}),
            "`contract_path` or `contract` is required",
        ),
        (
            "validate",
            json!({ "contract_path": "a", "contract": "b" }),
            "not both",
        ),
        (
            "check",
            json!({ "contract_path": example("contracts/orders.yaml") }),
            "give `data_paths` or `records`",
        ),
        (
            "check",
            json!({ "contract_path": "/nonexistent.yaml", "records": [] }),
            "/nonexistent.yaml",
        ),
        (
            "drift",
            json!({ "baseline_path": "x" }),
            "`current_path` is required",
        ),
    ] {
        let r = call(tool, args);
        assert_eq!(r["isError"], true, "{tool}");
        assert!(text(&r).contains(says), "{tool}: {}", text(&r));
    }
}

#[test]
fn the_binary_speaks_mcp_over_stdio() {
    let mut child = Command::new(BIN)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let lines = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "validate", "arguments": { "contract_path": example("contracts/orders.yaml") } } }),
    ];
    for l in &lines {
        writeln!(stdin, "{l}").unwrap();
    }
    drop(stdin);
    let replies: Vec<Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    assert!(child.wait().unwrap().success());
    // Two requests, two replies: the notification got none.
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(
        replies[1]["result"]["structuredContent"]["enforceable"],
        true
    );
}
