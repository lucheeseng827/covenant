//! `covenant serve` — the localhost console + JSON API (feature `serve`,
//! OFF by default so the default build stays a lean sync binary).
//!
//! This is the data-plane API only: everything here is answered by the
//! compiled contract loaded at startup, statelessly, per request. The
//! endpoints the console needs beyond that — registry listings, violation
//! history, DLQ browsing/replay, team rollups, notification routes — need
//! fleet state the runtime deliberately doesn't carry; the console marks
//! those screens "endpoint missing" rather than rendering stale state.
//!
//! | Route              | Answers                                             |
//! |--------------------|-----------------------------------------------------|
//! | `GET /`            | the console (embedded)                              |
//! | `GET /v1/health`   | liveness + version + served contract id             |
//! | `GET /v1/contract` | the loaded contract document + its lint findings    |
//! |                    | and the rules it runs without (`unenforced`)        |
//! | `POST /v1/validate`| body = contract YAML or ODCS → lint findings        |
//! | `POST /v1/check`   | body = NDJSON → a full CheckReport; partial, with   |
//! |                    | its `unenforced` list, under `--allow-unenforced`   |
//! | `POST /v1/diff`    | body = {old, new, consumers?} → a classified        |
//! |                    | DiffReport (+ blast radius when manifests are sent) |
//! | `POST /v1/consumers/verify` | body = {manifests} → each manifest checked |
//! |                    | against the served contract (consumer-side gate)    |
//! | `GET /v1/gate/stats` | the gate's `--stats` snapshot file, read per     |
//! |                    | request (`covenant serve --gate-stats <path>`)      |
//! | `GET /v1/dlq?limit=N` | newest dead letters from the gate's DLQ file    |
//! |                    | (`covenant serve --dlq <path>`)                     |

use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::compile::CompiledContract;
use crate::engine::UniqueTracker;
use crate::error::{CovenantError, Result};
use crate::report::{Collector, ReportHeader};
use crate::sources::check_ndjson_reader;
use crate::spec::{Contract, LintLevel};

/// The built console SPA (frontend/, Vite + React + TypeScript). `dist/` is
/// committed so building with `--features serve` needs no Node toolchain;
/// rebuild it with `npm run build` in `frontend/` after UI changes.
#[derive(rust_embed::RustEmbed)]
#[folder = "frontend/dist"]
struct ConsoleAssets;

/// Everything a request needs: the contract as authored, compiled, and the
/// resolved model name — plus the optional Phase-2 file taps (a gate's
/// stats snapshot and DLQ file) read per request, keeping the server
/// stateless.
pub struct AppState {
    pub source: String,
    pub doc: Contract,
    pub compiled: CompiledContract,
    pub model: String,
    /// Rules of the served contract that are not enforced (`covenant serve
    /// --allow-unenforced`). Every check report lists them, which marks it
    /// partial; empty for a fully enforced contract.
    pub unenforced: Vec<crate::spec::UnenforcedRule>,
    /// DLQ file to read for GET /v1/dlq (`covenant gate --dlq <path>`).
    pub dlq_path: Option<std::path::PathBuf>,
    /// Stats snapshot to read for GET /v1/gate/stats
    /// (`covenant gate --stats <path>`).
    pub gate_stats_path: Option<std::path::PathBuf>,
}

/// Build the API router (public so tests drive it without a socket).
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(console))
        .route("/console", get(console))
        .route("/assets/{*path}", get(asset))
        .route("/v1/health", get(health))
        .route("/v1/contract", get(contract))
        .route("/v1/validate", post(validate))
        .route("/v1/check", post(check))
        .route("/v1/diff", post(diff))
        .route("/v1/consumers/verify", post(consumers_verify))
        .route("/v1/gate/stats", get(gate_stats))
        .route("/v1/dlq", get(dlq))
        // /v1/check accepts whole NDJSON samples; axum's 2 MB default body
        // cap would 413 realistic batches, so raise it explicitly.
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

/// Bind and serve until the process is killed.
pub fn run(addr: &str, state: AppState) -> Result<()> {
    let addr: std::net::SocketAddr = addr.parse().map_err(|_| CovenantError::DataRead {
        path: addr.to_string(),
        message: "not a valid listen address (expected e.g. 127.0.0.1:8787)".into(),
    })?;
    // The API is unauthenticated and /v1/check burns CPU per request — the
    // loopback default is the safe posture; a wider bind is allowed but loud.
    if !addr.ip().is_loopback() {
        eprintln!(
            "covenant serve: WARNING — {addr} is not loopback; the console and /v1 API are unauthenticated"
        );
    }
    let rt = tokio::runtime::Runtime::new().map_err(|e| CovenantError::Io {
        path: "<tokio runtime>".into(),
        source: e,
    })?;
    rt.block_on(async move {
        let listener =
            tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| CovenantError::Io {
                    path: addr.to_string(),
                    source: e,
                })?;
        eprintln!(
            "covenant serve: console at http://{addr}/ — contract {} v{}, model {}",
            state.doc.id, state.doc.version, state.model
        );
        axum::serve(listener, router(Arc::new(state)))
            .await
            .map_err(|e| CovenantError::Io {
                path: addr.to_string(),
                source: e,
            })
    })
}

/// The console SPA entry point.
async fn console() -> Response {
    embedded("index.html")
}

/// Hashed build assets under /assets/ (JS bundle, stylesheet).
async fn asset(UrlPath(path): UrlPath<String>) -> Response {
    embedded(&format!("assets/{path}"))
}

/// Serve one embedded dist file with a content type from its extension.
fn embedded(path: &str) -> Response {
    match ConsoleAssets::get(path) {
        Some(file) => {
            let mime = match path.rsplit('.').next() {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                _ => "application/octet-stream",
            };
            ([(header::CONTENT_TYPE, mime)], file.data.into_owned()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Liveness probe: version + which contract/model this server enforces.
async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "contract": state.doc.id,
        "model": state.model,
    }))
}

/// The loaded contract document, verbatim, plus its lint findings.
async fn contract(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({
        "source": state.source,
        "model": state.model,
        "contract": state.doc,
        "findings": state.doc.lint(),
        "unenforced": state.unenforced,
    }))
}

/// Lint any contract YAML, `covenant: 1` or ODCS: 200 with findings, 400
/// when it won't parse. An ODCS rule the runtime cannot enforce is an error
/// finding, as `covenant validate` reports it without `--allow-unenforced`.
async fn validate(body: String) -> impl IntoResponse {
    match Contract::load(&body, "<request>") {
        Ok(loaded) => {
            let findings = loaded.lint(false);
            let enforceable = findings.iter().all(|f| f.level != LintLevel::Error);
            (
                StatusCode::OK,
                Json(json!({ "findings": findings, "enforceable": enforceable })),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

/// Run the row engine over an NDJSON body against the served model and
/// return the full CheckReport (exact counts, capped samples).
async fn check(State(state): State<Arc<AppState>>, body: String) -> impl IntoResponse {
    // The CLI resolves the model before serving, but AppState's fields are
    // public — never panic inside a request task on a broken invariant.
    let model = match state.compiled.resolve_model(Some(&state.model)) {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            );
        }
    };
    let mut collector = Collector::new(state.compiled.policy.sample_violations);
    let mut unique = UniqueTracker::new(model);
    match check_ndjson_reader(model, body.as_bytes(), &mut unique, &mut collector) {
        Ok(rows) => {
            let mut report = collector.into_report(
                ReportHeader {
                    contract_id: state.compiled.id.clone(),
                    contract_version: state.compiled.version.clone(),
                    owner: state.compiled.owner.clone(),
                    model: model.name.clone(),
                    source: "<http request>".into(),
                },
                rows,
            );
            report.unenforced = state.unenforced.clone();
            (
                StatusCode::OK,
                Json(serde_json::to_value(&report).expect("report serializes")),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
/// Request body for /v1/consumers/verify: manifest YAMLs to check against
/// the served contract.
struct VerifyBody {
    manifests: Vec<String>,
}

/// Verify consumer manifests against the served contract — the API form of
/// `covenant consumer-check`. 400 when a manifest won't parse, when the
/// list is empty, or when NOTHING in it consumes the served contract — a
/// CI job pointed at the wrong server must fail loudly, not pass on zero
/// findings forever (the same refusal the CLI makes). Verification
/// findings (stale fields, drifted pins) come back per manifest with 200.
async fn consumers_verify(
    State(state): State<Arc<AppState>>,
    Json(body): Json<VerifyBody>,
) -> impl IntoResponse {
    if body.manifests.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "no manifests supplied — nothing to verify" })),
        );
    }
    let mut manifests = Vec::with_capacity(body.manifests.len());
    for (i, src) in body.manifests.iter().enumerate() {
        match crate::consumers::ConsumerManifest::parse(src, &format!("<manifests[{i}]>")) {
            Ok(m) => manifests.push(m),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": e.to_string() })),
                );
            }
        }
    }
    if !manifests.iter().any(|m| m.consumes_contract(&state.doc.id)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "none of the {} manifest(s) consume contract {:?} — nothing to verify \
                     (wrong server, or a renamed contract id?)",
                    manifests.len(),
                    state.doc.id,
                ),
            })),
        );
    }
    // The same typed envelope `covenant consumer-check --format json`
    // prints — one wire shape, defined once in consumers.rs.
    let report = crate::consumers::ConsumerCheckReport {
        contract: state.doc.id.clone(),
        version: state.doc.version.clone(),
        results: manifests
            .iter()
            .map(|m| crate::consumers::VerifyResult {
                consumer: m.id.clone(),
                source: None,
                consumes_contract: m.consumes_contract(&state.doc.id),
                findings: m.verify_against(&state.doc),
            })
            .collect(),
    };
    (
        StatusCode::OK,
        Json(serde_json::to_value(&report).expect("verify report serializes")),
    )
}

/// The gate's `--stats` snapshot, read per request. Three honest shapes:
/// `{configured:false, hint}` when serve wasn't given `--gate-stats`;
/// `{configured:true, note}` when the file isn't there/parseable yet; and
/// `{configured:true, stats, age_seconds}` when it is — `age_seconds` lets
/// the console flag a snapshot whose gate has stopped writing.
async fn gate_stats(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let Some(path) = &state.gate_stats_path else {
        return Json(json!({
            "configured": false,
            "hint": "start serve with --gate-stats <path> and the gate with --stats <path> to light this screen",
        }));
    };
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let Some(stats) = parsed else {
        return Json(json!({
            "configured": true,
            "note": format!(
                "stats snapshot {} not found or not parseable yet — is `covenant gate --stats` running?",
                path.display()
            ),
        }));
    };
    // Age from the snapshot's own clock; absent/unparseable timestamps
    // simply omit the age rather than failing the read.
    let age_seconds = stats
        .get("updated_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds());
    Json(json!({ "configured": true, "stats": stats, "age_seconds": age_seconds }))
}

#[derive(Deserialize)]
/// Query for /v1/dlq: how many of the newest envelopes to return.
struct DlqQuery {
    limit: Option<usize>,
}

/// How much of the DLQ file's tail is scanned per request. The DLQ is
/// append-only NDJSON; reading a bounded tail keeps the endpoint O(1) in
/// file size while still serving the newest dead letters.
const DLQ_TAIL_BYTES: u64 = 1 << 20;

/// Newest dead letters from the gate's DLQ file, read per request
/// (newest first). Unparseable lines (e.g. a torn concurrent append at the
/// tail boundary) are counted in `skipped`, never silently dropped.
async fn dlq(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<DlqQuery>,
) -> Json<serde_json::Value> {
    let Some(path) = &state.dlq_path else {
        return Json(json!({
            "configured": false,
            "hint": "start serve with --dlq <path> (the same file the gate writes with --dlq)",
        }));
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    match read_dlq_tail(path, limit) {
        Ok((entries, skipped, total_bytes)) => Json(json!({
            "configured": true,
            "source": path.display().to_string(),
            "entries": entries,
            "skipped": skipped,
            "total_bytes": total_bytes,
        })),
        Err(e) => Json(json!({
            "configured": true,
            "note": format!(
                "DLQ file {} not readable ({e}) — has the gate written any dead letters?",
                path.display()
            ),
        })),
    }
}

/// Read the last `limit` well-formed envelopes from the DLQ file's bounded
/// tail, newest first. Returns (entries, skipped_lines, file_bytes).
///
/// The tail is read as BYTES and decoded lossily: the seek offset can land
/// mid-UTF-8-character, where a strict read would fail the whole request on
/// perfectly valid non-ASCII data — and the mangled partial first line is
/// dropped anyway. Lines that don't parse, or parse but aren't envelope
/// shaped (an object with a `violations` array and a `record`), are counted
/// in `skipped` rather than served — a stray JSON line in the file must
/// never crash the console.
fn read_dlq_tail(
    path: &std::path::Path,
    limit: usize,
) -> std::io::Result<(Vec<serde_json::Value>, u64, u64)> {
    use std::io::{Read as _, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let total_bytes = file.metadata()?.len();
    let start = total_bytes.saturating_sub(DLQ_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    let buf = String::from_utf8_lossy(&raw);
    let mut lines: Vec<&str> = buf.lines().collect();
    // A mid-file seek almost certainly landed inside a line — drop the
    // partial first line rather than counting it as corruption.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let is_envelope = |v: &serde_json::Value| {
        v.get("violations").is_some_and(serde_json::Value::is_array) && v.get("record").is_some()
    };
    let mut skipped = 0u64;
    let mut entries: Vec<serde_json::Value> = Vec::new();
    // Walk backwards so the newest `limit` envelopes win.
    for line in lines.iter().rev() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) if is_envelope(&v) => {
                entries.push(v);
                if entries.len() >= limit {
                    break;
                }
            }
            Ok(_) | Err(_) => skipped += 1,
        }
    }
    Ok((entries, skipped, total_bytes))
}

#[derive(Deserialize)]
/// Request body for /v1/diff: the two contract YAMLs to compare, plus
/// optional consumer-manifest YAMLs for the blast radius.
struct DiffBody {
    old: String,
    new: String,
    #[serde(default)]
    consumers: Option<Vec<String>>,
}

/// Classify the changes between two contracts (severity x impact) and
/// enforce the semver bump; when consumer manifests are sent, attach the
/// named blast radius. 400 when any document won't parse.
async fn diff(Json(body): Json<DiffBody>) -> impl IntoResponse {
    let old = match Contract::parse(&body.old, "<old>") {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            );
        }
    };
    let new = match Contract::parse(&body.new, "<new>") {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            );
        }
    };
    let mut report = crate::diff::diff(&old, &new);
    if let Some(sources) = &body.consumers {
        let mut manifests = Vec::with_capacity(sources.len());
        for (i, src) in sources.iter().enumerate() {
            match crate::consumers::ConsumerManifest::parse(src, &format!("<consumers[{i}]>")) {
                Ok(m) => manifests.push(m),
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": e.to_string() })),
                    );
                }
            }
        }
        crate::consumers::annotate(&mut report, &old, &new, &manifests);
    }
    (
        StatusCode::OK,
        Json(serde_json::to_value(&report).expect("diff serializes")),
    )
}
