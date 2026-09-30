//! `covenant mcp` — the checker as Model Context Protocol tools, so the
//! agents writing pipelines can validate a contract, check data, classify a
//! contract change, read what a contract requires and compare profiles,
//! without shelling out and parsing terminal text.
//!
//! JSON-RPC 2.0 over newline-delimited stdio: one message per line in, one
//! reply per request out. `initialize` negotiates the protocol revision,
//! `tools/list` and `tools/call` do the work, `ping` answers, and
//! notifications get no reply. stdout carries nothing but protocol; the
//! warnings the loaders print go to stderr.
//!
//! The tools read files with the permissions of whoever started the server,
//! exactly as the CLI does. A verdict is a successful call, even a failing
//! one; a contract that cannot be read, or a file that is missing, is a tool
//! error (`isError: true`) the agent can see and act on.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::compile::CompiledContract;
use crate::error::{CovenantError, Result};
use crate::spec::{Contract, Field, LintLevel, LoadedContract};

/// Protocol revisions this server speaks, oldest first. The tools surface
/// used here is the same in all of them.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Serve until `input` ends.
pub fn serve(input: impl BufRead, mut output: impl Write) -> Result<()> {
    let io = |e| CovenantError::Io {
        path: "<mcp stdio>".into(),
        source: e,
    };
    for line in input.lines() {
        let line = line.map_err(io)?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(line) {
            let mut buf = serde_json::to_vec(&reply).expect("replies serialize");
            buf.push(b'\n');
            output.write_all(&buf).map_err(io)?;
            output.flush().map_err(io)?;
        }
    }
    Ok(())
}

/// One incoming line; `None` when nothing may be sent back (a notification).
pub fn handle_line(line: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return id.map(|id| {
            error(
                id,
                INVALID_REQUEST,
                "a request needs a string `method`".into(),
            )
        });
    };
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    // A notification never gets a reply, known or not.
    let id = id?;
    Some(match method {
        "initialize" => success(id, initialize(&params)),
        "ping" => success(id, json!({})),
        "tools/list" => success(id, json!({ "tools": tool_defs() })),
        "tools/call" => call(id, &params),
        other => error(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
    })
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(PROTOCOL_VERSIONS[PROTOCOL_VERSIONS.len() - 1]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "covenant", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Covenant enforces data contracts (its own `covenant: 1` format, or ODCS v3). \
            Before writing code that produces or consumes a dataset with a contract, call `explain` \
            to read exactly what the data must satisfy. Use `check` to test data files (NDJSON, \
            CSV, Parquet) or a few inline records against it, `validate` after editing a contract, \
            `diff` to see whether a contract change breaks consumers, and `drift` to compare two \
            profiles written by `covenant check --profile`. Paths are read with this server's \
            permissions. An ODCS rule Covenant cannot enforce refuses the contract unless \
            `allow_unenforced` is true, and then the result says it is partial."
    })
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn call(id: Value, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error(
            id,
            INVALID_PARAMS,
            "tools/call needs a string `name`".into(),
        );
    };
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let run = match name {
        "validate" => validate,
        "check" => check,
        "diff" => diff,
        "explain" => explain,
        "drift" => drift,
        other => return error(id, INVALID_PARAMS, format!("unknown tool: {other}")),
    };
    let result = match run(&args) {
        Ok((text, structured)) => json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": structured,
            "isError": false,
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": e.to_string() }],
            "isError": true,
        }),
    };
    success(id, result)
}

/// What a tool returns: text for the model to read, and the same result as
/// data.
type ToolResult = Result<(String, Value)>;

fn tool_defs() -> Value {
    let contract = json!({
        "contract_path": { "type": "string", "description": "Path to the contract file (covenant: 1 or ODCS v3, YAML or JSON)" },
        "contract": { "type": "string", "description": "The contract document itself, instead of a path" },
        "allow_unenforced": { "type": "boolean", "description": "Run an ODCS contract even though some of its rules cannot be enforced yet; the result is then partial" }
    });
    let with = |extra: Value| {
        let mut props = contract.clone();
        if let (Some(p), Some(e)) = (props.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        props
    };
    json!([
        {
            "name": "validate",
            "title": "Lint a contract",
            "description": "Lint a data contract. Returns every finding (error or warning) with its path, and whether the contract is enforceable. For ODCS contracts, rules Covenant cannot enforce are errors unless allow_unenforced is true.",
            "inputSchema": { "type": "object", "properties": contract, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "check",
            "title": "Check data against a contract",
            "description": "Check data files (NDJSON, CSV, Parquet) or inline records against a contract. Returns pass or fail, exact violation counts per field and rule, and sample violations. A failing verdict is a successful call.",
            "inputSchema": {
                "type": "object",
                "properties": with(json!({
                    "data_paths": { "type": "array", "items": { "type": "string" }, "description": "Data files to check (.ndjson/.jsonl/.json, .csv, .parquet)" },
                    "records": { "type": "array", "items": { "type": "object" }, "description": "Records to check instead of files" },
                    "model": { "type": "string", "description": "Model to check (defaults to the contract's only model)" },
                    "profile_path": { "type": "string", "description": "Also write a profile of the checked data here, for drift. An existing file is replaced only if it is a profile" }
                })),
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "openWorldHint": false }
        },
        {
            "name": "diff",
            "title": "Classify a contract change",
            "description": "Classify the changes between two versions of a contract as breaking, risky or info, for producers or consumers, and check the semver bump. With consumers_dir, names every declared consumer each change breaks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "old_path": { "type": "string", "description": "The contract consumers rely on today" },
                    "old": { "type": "string", "description": "That contract's text, instead of a path" },
                    "new_path": { "type": "string", "description": "The proposed contract" },
                    "new": { "type": "string", "description": "That contract's text, instead of a path" },
                    "consumers_dir": { "type": "string", "description": "Directory of consumer manifests" },
                    "allow_unenforced": { "type": "boolean" }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "explain",
            "title": "What a contract requires",
            "description": "Describe, field by field and in plain words, everything a contract requires of the data, and any rule it states that is not enforced. Read this before writing code that produces or consumes the dataset.",
            "inputSchema": {
                "type": "object",
                "properties": with(json!({
                    "model": { "type": "string", "description": "Only this model" },
                    "field": { "type": "string", "description": "Only this field" }
                })),
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "drift",
            "title": "Compare two profiles",
            "description": "Compare two profiles of one contract model (written by `covenant check --profile`, or check's profile_path) and report what drifted: volume, null/missing/invalid rates, distinct values, and numeric or categorical distributions, each with a one-sentence explanation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "baseline_path": { "type": "string", "description": "The profile to compare against" },
                    "current_path": { "type": "string", "description": "The profile of the data now" },
                    "rate": { "type": "number", "minimum": 0, "description": "Tolerated change in a null/missing/invalid rate, in points (default 0.05)" },
                    "psi": { "type": "number", "minimum": 0, "description": "Tolerated population stability index (default 0.25)" },
                    "js": { "type": "number", "minimum": 0, "description": "Tolerated Jensen–Shannon distance (default 0.1)" },
                    "distinct": { "type": "number", "minimum": 0, "description": "Tolerated distinct-value change (default 0.1)" },
                    "volume": { "type": "number", "minimum": 0, "description": "Tolerated relative change in rows per run (default 0.5)" }
                },
                "required": ["baseline_path", "current_path"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        }
    ])
}

// --- argument helpers ----------------------------------------------------------------

fn usage(message: impl Into<String>) -> CovenantError {
    CovenantError::Usage {
        message: message.into(),
    }
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(usage(format!("`{key}` must be a string"))),
    }
}

fn flag(args: &Value, key: &str) -> Result<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(usage(format!("`{key}` must be true or false"))),
    }
}

/// A contract from `<prefix>_path` or inline `<prefix>`: exactly one.
fn load_contract(args: &Value, prefix: &str) -> Result<(LoadedContract, String)> {
    let path_key = format!("{prefix}_path");
    match (opt_str(args, &path_key)?, opt_str(args, prefix)?) {
        (Some(path), None) => Ok((Contract::load_path(Path::new(path))?, path.to_string())),
        (None, Some(text)) => Ok((
            Contract::load(text, &format!("<{prefix}>"))?,
            format!("<{prefix}>"),
        )),
        (Some(_), Some(_)) => Err(usage(format!("give `{path_key}` or `{prefix}`, not both"))),
        (None, None) => Err(usage(format!("`{path_key}` or `{prefix}` is required"))),
    }
}

/// [`load_contract`], refusing a partial contract unless allowed; returns the
/// rules it runs without.
fn enforceable(
    args: &Value,
    prefix: &str,
) -> Result<(Contract, Vec<crate::spec::UnenforcedRule>, String)> {
    let (loaded, origin) = load_contract(args, prefix)?;
    if flag(args, "allow_unenforced")? || loaded.unenforced.is_empty() {
        Ok((loaded.contract, loaded.unenforced, origin))
    } else {
        Err(loaded.into_enforceable(&origin).unwrap_err())
    }
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("results serialize")
}

// --- tools -------------------------------------------------------------------------

fn validate(args: &Value) -> ToolResult {
    let (loaded, origin) = load_contract(args, "contract")?;
    let findings = loaded.lint(flag(args, "allow_unenforced")?);
    let enforceable = findings.iter().all(|f| f.level != LintLevel::Error);
    let mut text = format!(
        "{} v{} ({origin}): {}",
        loaded.contract.id,
        loaded.contract.version,
        if enforceable {
            "enforceable"
        } else {
            "NOT enforceable — fix the errors below"
        }
    );
    for f in &findings {
        let level = match f.level {
            LintLevel::Error => "error",
            LintLevel::Warning => "warning",
        };
        text.push_str(&format!("\n{level}: {}: {}", f.path, f.message));
    }
    if findings.is_empty() {
        text.push_str("\nno findings");
    }
    Ok((
        text,
        json!({ "contract": loaded.contract.id, "version": loaded.contract.version, "enforceable": enforceable, "findings": findings }),
    ))
}

fn check(args: &Value) -> ToolResult {
    let (doc, unenforced, _) = enforceable(args, "contract")?;
    let compiled = CompiledContract::compile(&doc)?;
    let target = compiled.resolve_model(opt_str(args, "model")?)?;
    let paths: Vec<String> = match args.get("data_paths") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| usage("`data_paths` must be strings"))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(usage("`data_paths` must be an array of paths")),
    };
    let records = match args.get("records") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a),
        Some(_) => return Err(usage("`records` must be an array of objects")),
    };
    if paths.is_empty() == records.is_none() {
        return Err(usage("give `data_paths` or `records`, one of them"));
    }
    let profile_path = opt_str(args, "profile_path")?;
    if let Some(path) = profile_path {
        // The path comes from an agent: replace a profile, never anything else.
        if Path::new(path).exists() && crate::profile::Profile::from_path(Path::new(path)).is_err()
        {
            return Err(usage(format!(
                "refusing to write a profile to {path}: that file exists and is not a profile"
            )));
        }
    }
    let mut profiler = profile_path.map(|_| crate::profile::Profiler::new(&compiled, target));

    let mut reports = Vec::new();
    for path in &paths {
        reports.push(crate::sources::check_path_profiled(
            &compiled,
            target,
            Path::new(path),
            None,
            profiler.as_mut(),
        )?);
    }
    if let Some(records) = records {
        let ndjson: String = records.iter().map(|r| format!("{r}\n")).collect();
        let mut collector = crate::report::Collector::new(compiled.policy.sample_violations);
        let mut unique = crate::engine::UniqueTracker::new(target);
        let rows = crate::sources::check_ndjson_reader_profiled(
            target,
            BufReader::new(ndjson.as_bytes()),
            &mut unique,
            &mut collector,
            profiler.as_mut(),
        )?;
        reports.push(collector.into_report(
            crate::report::ReportHeader {
                contract_id: compiled.id.clone(),
                contract_version: compiled.version.clone(),
                owner: compiled.owner.clone(),
                model: target.name.clone(),
                source: "<records>".into(),
            },
            rows,
        ));
    }
    for r in &mut reports {
        r.unenforced = unenforced.clone();
    }
    let budget = compiled.policy.max_violations;
    let total: u64 = reports.iter().map(|r| r.violations).sum();
    // As `covenant check` decides: a warn-only contract reports violations
    // and never fails a run.
    let warn_only = compiled.policy.on_violation == crate::spec::OnViolation::Warn;
    let passed = warn_only || total <= budget;
    let mut text: String = reports.iter().map(|r| r.render_human(budget)).collect();
    text.push_str(&format!(
        "verdict: {} — {total} violation{} (budget {budget}{})",
        if passed { "PASS" } else { "FAIL" },
        if total == 1 { "" } else { "s" },
        if warn_only {
            "; policy is on_violation: warn — reporting only, not failing"
        } else {
            ""
        }
    ));
    if let (Some(path), Some(p)) = (profile_path, profiler) {
        let written = p.finish();
        written.write(Path::new(path))?;
        text.push_str(&format!("\nprofile written to {path}"));
    }
    Ok((
        text,
        json!({ "passed": passed, "violations": total, "budget": budget, "warn_only": warn_only, "partial": !unenforced.is_empty(), "reports": reports }),
    ))
}

fn diff(args: &Value) -> ToolResult {
    let (old, mut unenforced, _) = enforceable(args, "old")?;
    let (new, new_unenforced, _) = enforceable(args, "new")?;
    for r in new_unenforced {
        if !unenforced
            .iter()
            .any(|o| o.path == r.path && o.rule == r.rule)
        {
            unenforced.push(r);
        }
    }
    let mut report = crate::diff::diff(&old, &new);
    report.unenforced = unenforced;
    if let Some(dir) = opt_str(args, "consumers_dir")? {
        let manifests = crate::consumers::load_dir(Path::new(dir))?;
        crate::consumers::annotate(&mut report, &old, &new, &manifests);
    }
    let worst = report.max_severity().map(|s| s.name());
    Ok((
        report.render_human(),
        json!({ "max_severity": worst, "report": to_value(&report) }),
    ))
}

fn drift(args: &Value) -> ToolResult {
    // Arguments first, files second: a missing argument is the clearer error.
    let required = |key: &str| -> Result<&str> {
        opt_str(args, key)?.ok_or_else(|| usage(format!("`{key}` is required")))
    };
    let (baseline, current) = (required("baseline_path")?, required("current_path")?);
    let mut t = crate::drift::DriftThresholds::default();
    for (key, slot) in [
        ("rate", &mut t.rate),
        ("psi", &mut t.psi),
        ("js", &mut t.js),
        ("distinct", &mut t.distinct),
        ("volume", &mut t.volume),
    ] {
        match args.get(key) {
            None | Some(Value::Null) => {}
            Some(v) => match v.as_f64() {
                Some(x) if x.is_finite() && x >= 0.0 => *slot = x,
                _ => return Err(usage(format!("`{key}` must be a non-negative number"))),
            },
        }
    }
    let report = crate::drift::drift(
        &crate::profile::Profile::from_path(Path::new(baseline))?,
        &crate::profile::Profile::from_path(Path::new(current))?,
        &t,
    )?;
    Ok((
        report.render_human(),
        json!({ "drifted": report.drifted(), "report": to_value(&report) }),
    ))
}

fn explain(args: &Value) -> ToolResult {
    let (loaded, origin) = load_contract(args, "contract")?;
    let doc = &loaded.contract;
    let only_model = opt_str(args, "model")?;
    let only_field = opt_str(args, "field")?;
    if let Some(m) = only_model {
        if !doc.models.contains_key(m) {
            return Err(CovenantError::ModelNotFound {
                contract_id: doc.id.clone(),
                model: m.to_string(),
                available: doc.models.keys().cloned().collect::<Vec<_>>().join(", "),
            });
        }
    }
    let mut lines = vec![format!(
        "Contract {} v{} ({origin}){}{}",
        doc.id,
        doc.version,
        doc.owner
            .as_deref()
            .map(|o| format!(", owned by {o}"))
            .unwrap_or_default(),
        doc.description
            .as_deref()
            .map(|d| format!(": {d}"))
            .unwrap_or_default()
    )];
    let policy = &doc.policy;
    let warn_only = policy.on_violation == crate::spec::OnViolation::Warn;
    lines.push(match (policy.max_violations, warn_only) {
        (_, true) => {
            "This contract only warns: violations are reported, never fail a run.".to_string()
        }
        (0, false) => "Any violation fails a check.".to_string(),
        (n, false) => format!(
            "A check fails when more than {n} violation{} occur.",
            if n == 1 { "" } else { "s" }
        ),
    });
    let mut fields_out = Vec::new();
    let mut matched = false;
    for (name, model) in &doc.models {
        if only_model.is_some_and(|m| m != name) {
            continue;
        }
        lines.push(format!(
            "\nModel {name}{}{}",
            model
                .description
                .as_deref()
                .map(|d| format!(" — {d}"))
                .unwrap_or_default(),
            if model.strict {
                ". Strict: a field not listed here is a violation."
            } else {
                ". Open: fields not listed here pass unchecked."
            }
        ));
        for (fname, field) in &model.fields {
            if only_field.is_some_and(|f| f != fname) {
                continue;
            }
            matched = true;
            let rules = field_rules(field);
            lines.push(format!(
                "- {fname} ({}): {}",
                field.ty.name(),
                rules.join("; ")
            ));
            fields_out.push(
                json!({ "model": name, "field": fname, "type": field.ty.name(), "rules": rules }),
            );
        }
    }
    if let Some(f) = only_field {
        if !matched {
            return Err(usage(format!("no field {f:?} in contract {}", doc.id)));
        }
    }
    if !loaded.unenforced.is_empty() {
        lines.push(format!(
            "\nNot enforced by Covenant ({} rule{} stated in the contract; checking refuses unless allow_unenforced):",
            loaded.unenforced.len(),
            if loaded.unenforced.len() == 1 { "" } else { "s" }
        ));
        for r in &loaded.unenforced {
            lines.push(format!("- {} ({}): {}", r.path, r.rule, r.reason));
        }
    }
    Ok((
        lines.join("\n"),
        json!({ "contract": doc.id, "version": doc.version, "fields": fields_out, "unenforced": to_value(&loaded.unenforced) }),
    ))
}

/// A field's promise, in the words a producer needs.
fn field_rules(f: &Field) -> Vec<String> {
    use crate::spec::FieldType;
    // Every member named, none skipped with `..`: a rule added to `Field`
    // does not compile until this function describes it.
    let Field {
        ty: _,
        description: _,
        required: _,
        nullable: _,
        unique: _,
        pattern: _,
        min: _,
        max: _,
        min_length: _,
        max_length: _,
        allowed: _,
        format: _,
        deprecated: _,
    } = f;
    let mut r = Vec::new();
    match f.ty {
        FieldType::String => {}
        FieldType::Integer => r.push("a whole number".into()),
        FieldType::Float => r.push("a number".into()),
        FieldType::Boolean => r.push("true or false".into()),
        FieldType::Timestamp => r.push("an RFC 3339 timestamp such as 2026-09-01T12:00:00Z".into()),
        FieldType::Date => r.push("a YYYY-MM-DD date".into()),
        FieldType::Uuid => r.push("a canonical 8-4-4-4-12 hex UUID".into()),
    }
    r.push(if f.required {
        "required (the key must be present)".to_string()
    } else {
        "optional (the key may be absent)".to_string()
    });
    r.push(if f.nullable {
        "may be null".to_string()
    } else {
        "never null".to_string()
    });
    if f.unique {
        r.push("unique across the dataset".into());
    }
    match (f.min, f.max) {
        (Some(lo), Some(hi)) => r.push(format!("between {lo} and {hi} inclusive")),
        (Some(lo), None) => r.push(format!("at least {lo}")),
        (None, Some(hi)) => r.push(format!("at most {hi}")),
        (None, None) => {}
    }
    match (f.min_length, f.max_length) {
        (Some(lo), Some(hi)) if lo == hi => r.push(format!("exactly {lo} characters")),
        (Some(lo), Some(hi)) => r.push(format!("{lo} to {hi} characters")),
        (Some(lo), None) => r.push(format!("at least {lo} characters")),
        (None, Some(hi)) => r.push(format!("at most {hi} characters")),
        (None, None) => {}
    }
    if let Some(p) = &f.pattern {
        r.push(if p.starts_with('^') && p.ends_with('$') {
            format!("matches the regex {p}")
        } else {
            format!("contains a match for the regex {p} (it is not anchored)")
        });
    }
    if let Some(fmt) = f.format {
        r.push(format!("formatted as {}", fmt.name()));
    }
    if let Some(a) = &f.allowed {
        let vals: Vec<String> = a.iter().map(|v| v.to_string()).collect();
        r.push(format!("one of {}", vals.join(", ")));
    }
    if let Some(note) = &f.deprecated {
        r.push(if note.is_empty() {
            "deprecated".to_string()
        } else {
            format!("deprecated: {note}")
        });
    }
    r
}
