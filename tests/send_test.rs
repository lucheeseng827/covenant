//! Sending a report document: `--report-to` on `check`, `gate` and `diff`,
//! and `covenant push`, against a server on this machine that records what
//! it is sent. A document that cannot be sent is one warning and never
//! changes a run's exit code; `push` exits 2 for what it could not deliver.
#![cfg(feature = "send")]

mod common;

use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};

use common::server;

const BIN: &str = env!("CARGO_BIN_EXE_covenant");

const CONTRACT: &str = r#"
covenant: 1
id: orders
version: 1.0.0
models:
  orders:
    fields:
      order_id: { type: string, required: true }
      currency: { type: string, allowed: [USD, EUR] }
"#;

const DIRTY: &str = r#"{"order_id":"a","currency":"USD"}
{"order_id":"b","currency":"BTC"}
"#;

const TOKEN: &str = "tok_7d1e44c09b2a";

/// Runs covenant in `dir` with the token set and no CI in its environment.
fn covenant(dir: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.current_dir(dir)
        .args(args)
        .env("COVENANT_TOKEN", TOKEN)
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("CI");
    match stdin {
        Some(name) => cmd.stdin(std::fs::File::open(dir.join(name)).unwrap()),
        None => cmd.stdin(std::process::Stdio::null()),
    };
    cmd.output().unwrap()
}

fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("contract.yaml"), CONTRACT).unwrap();
    std::fs::write(dir.path().join("orders.ndjson"), DIRTY).unwrap();
    dir
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_check_sends_its_document_with_the_token_and_keeps_its_exit_code() {
    let dir = setup();
    let (url, received) = server(202);
    let check = ["check", "orders.ndjson", "-c", "contract.yaml"];
    let out = covenant(
        dir.path(),
        &[
            &check[..],
            &["--report-json", "r.json", "--report-to", &url],
        ]
        .concat(),
        None,
    );
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let got = received.recv().unwrap();
    assert_eq!(got.request_line, "POST /v1/runs HTTP/1.1");
    assert_eq!(
        got.header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(got.header("content-type"), Some("application/json"));
    // What was sent is what was written, byte for byte.
    assert_eq!(got.body, std::fs::read(dir.path().join("r.json")).unwrap());
    assert_eq!(got.json()["kind"], "check");
    assert!(said(&out).contains(&format!("report: covenant-report/v1 sent → {url}")));
    assert!(!said(&out).contains(TOKEN), "the token was printed");
}

#[test]
fn a_document_that_cannot_be_sent_is_one_warning_and_the_exit_code_stands() {
    let dir = setup();
    // Refused by the server.
    let (url, received) = server(503);
    let out = covenant(
        dir.path(),
        &[
            "check",
            "orders.ndjson",
            "-c",
            "contract.yaml",
            "--report-to",
            &url,
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(1));
    received.recv().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("warning: report not sent to {url}: HTTP 503")),
        "{stderr}"
    );
    // Redirected: not followed, so the document goes nowhere the URL rule
    // did not allow.
    let (url, received) = server(307);
    let out = covenant(
        dir.path(),
        &[
            "check",
            "orders.ndjson",
            "-c",
            "contract.yaml",
            "--report-to",
            &url,
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(1));
    received.recv().unwrap();
    assert!(
        said(&out).contains(&format!(
            "warning: report not sent to {url}: HTTP 307: a redirect is not followed"
        )),
        "{}",
        said(&out)
    );
    // Nobody listening.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/v1/runs", listener.local_addr().unwrap())
    };
    let out = covenant(
        dir.path(),
        &[
            "check",
            "orders.ndjson",
            "-c",
            "contract.yaml",
            "--report-to",
            &closed,
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("warning: report not sent to"));
    assert!(!said(&out).contains(TOKEN));
}

#[test]
fn plain_http_leaves_this_machine_never_and_is_refused_before_the_run() {
    let dir = setup();
    for url in [
        "http://ingest.example.com/v1/runs",
        "https://user:secret@ingest.example.com/v1/runs",
    ] {
        let out = covenant(
            dir.path(),
            &[
                "check",
                "orders.ndjson",
                "-c",
                "contract.yaml",
                "--report-to",
                url,
            ],
            None,
        );
        assert_eq!(out.status.code(), Some(2), "{url}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains("FAIL"), "the run started: {stdout}");
    }
}

#[test]
fn the_gate_and_diff_send_theirs() {
    let dir = setup();
    let (url, received) = server(200);
    let out = covenant(
        dir.path(),
        &[
            "gate",
            "-c",
            "contract.yaml",
            "--dlq",
            "dlq.ndjson",
            "--report-to",
            &url,
        ],
        Some("orders.ndjson"),
    );
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let gate = received.recv().unwrap().json();
    assert_eq!(gate["kind"], "gate");
    assert_eq!(gate["gate"]["blocked"], 1);
    // The gate's stdout is its stream: the clean record, nothing else.
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 1);

    std::fs::write(
        dir.path().join("new.yaml"),
        CONTRACT.replace("version: 1.0.0", "version: 2.0.0"),
    )
    .unwrap();
    let out = covenant(
        dir.path(),
        &["diff", "contract.yaml", "new.yaml", "--report-to", &url],
        None,
    );
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert_eq!(received.recv().unwrap().json()["kind"], "diff");
}

#[test]
fn push_sends_documents_on_disk_and_refuses_what_is_not_one() {
    let dir = setup();
    let out = covenant(
        dir.path(),
        &[
            "check",
            "orders.ndjson",
            "-c",
            "contract.yaml",
            "--report-json",
            "r.json",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(1));
    let (url, received) = server(200);
    let out = covenant(dir.path(), &["push", "r.json", "--to", &url], None);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert_eq!(
        received.recv().unwrap().body,
        std::fs::read(dir.path().join("r.json")).unwrap()
    );
    assert!(said(&out).contains(&format!("pushed r.json → {url}")));

    // A contract is not a document: nothing is sent, and push says so.
    let out = covenant(
        dir.path(),
        &["push", "contract.yaml", "r.json", "--to", &url],
        None,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(said(&out).contains("contract.yaml: not a covenant-report/v1 document"));
    assert_eq!(
        received.recv().unwrap().json()["report"],
        1,
        "r.json still went"
    );
    assert!(received.try_recv().is_err(), "the contract was sent");

    // Refused by the server: push fails.
    let (refusing, _received) = server(401);
    let out = covenant(dir.path(), &["push", "r.json", "--to", &refusing], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(said(&out).contains("HTTP 401"), "{}", said(&out));
}
