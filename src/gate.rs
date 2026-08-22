//! The stream gate: NDJSON in on stdin, clean records out on stdout,
//! violations to a dead-letter sink. Transport-agnostic on purpose — pipe it
//! between `kcat -C` and `kcat -P` and it is a Kafka SMT; put it in a
//! CronJob reading a dump and it is a warehouse pre-load hook; a native
//! Kafka consumer/producer wrapper is a roadmap feature, not a prerequisite.
//!
//! Contract policy drives behavior: `on_violation: block` withholds dirty
//! records from stdout (they go to the DLQ with their violation list);
//! `on_violation: warn` passes everything through and still reports.

use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

use indexmap::IndexMap;
use serde::Serialize;

use crate::compile::{CompiledContract, CompiledModel};
use crate::engine::{row, UniqueTracker};
use crate::error::{CovenantError, Result};
use crate::report::Violation;
use crate::spec::OnViolation;

/// A rejected record and why — one JSON object per line in the DLQ, carrying
/// enough to replay the record after the producer is fixed.
#[derive(Debug, Serialize)]
pub struct DlqEnvelope<'a> {
    pub contract_id: &'a str,
    pub contract_version: &'a str,
    pub model: &'a str,
    /// 0-based index of the record in this gate run.
    pub row: u64,
    /// When the record was dead-lettered (RFC 3339). Additive since the
    /// initial format — readers must treat it as optional.
    pub ts: String,
    /// The record verbatim (the raw line when it wasn't even valid JSON).
    pub record: serde_json::Value,
    pub violations: &'a [Violation],
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Longest single NDJSON record the gate will buffer. Anything longer is
/// dead-lettered (truncated, with its size in the violation message) instead
/// of allocated — the gate protects the pipeline, so it must not be the
/// component that gets OOM-killed by one pathological producer line.
pub const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Default, Serialize)]
/// Counters for one gate run, printed to stderr at end of stream.
pub struct GateStats {
    pub records: u64,
    pub passed: u64,
    pub blocked: u64,
    /// Records that violated but were passed through (`on_violation: warn`).
    pub warned: u64,
}

/// A finished gate run: its counters and the enforcement verdict.
pub struct GateOutcome {
    pub stats: GateStats,
    /// True when enforcement (policy `block`, minus any `max_violations`
    /// budget) says the stream failed.
    pub failed: bool,
}

/// Width of one throughput bucket in the stats snapshot's `recent` series.
const RECENT_BUCKET_SECS: u64 = 10;
/// Buckets retained (matches the console's throughput panel width).
const RECENT_BUCKETS: usize = 36;
/// Minimum interval between snapshot writes.
const WRITE_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// The Phase-2 stats sidecar: `covenant gate --stats <path>` periodically
/// writes a machine-readable snapshot that `covenant serve --gate-stats
/// <path>` reads for `GET /v1/gate/stats` — keeping the gate a plain pipe
/// process (no control socket) while still lighting the console.
///
/// Snapshots are written atomically (tmp + rename) so a concurrent reader
/// never sees a torn file. Write failures print ONE stderr warning and
/// disable the sink — observability must never take down enforcement.
pub struct StatsSink {
    path: PathBuf,
    contract_id: String,
    contract_version: String,
    model: String,
    started_at: String,
    /// Exact per-(field, rule) violation counts; "" = record-level, the
    /// same convention as [`crate::report::RuleCount`].
    per_rule: IndexMap<(String, &'static str), u64>,
    /// log2(nanoseconds) histogram of per-record parse+validate time — the
    /// gate's "added latency". Bucketed, so the p99 is an upper bound.
    latency_ns: [u64; 40],
    /// Rolling throughput buckets, oldest first.
    recent: VecDeque<RecentBucket>,
    last_write: Instant,
    broken: bool,
}

#[derive(Clone, Copy)]
struct RecentBucket {
    slot: u64,
    records: u64,
    blocked: u64,
}

impl StatsSink {
    /// A sink writing snapshots for one gate run to `path`.
    pub fn new(path: PathBuf, contract: &CompiledContract, model: &CompiledModel) -> StatsSink {
        StatsSink {
            path,
            contract_id: contract.id.clone(),
            contract_version: contract.version.clone(),
            model: model.name.clone(),
            started_at: now_rfc3339(),
            per_rule: IndexMap::new(),
            latency_ns: [0; 40],
            recent: VecDeque::with_capacity(RECENT_BUCKETS + 1),
            // Backdated so the very first record triggers a write — a
            // just-started gate should become visible immediately.
            last_write: Instant::now()
                .checked_sub(WRITE_EVERY)
                .unwrap_or_else(Instant::now),
            broken: false,
        }
    }

    fn on_record(&mut self, blocked: bool, violations: &[Violation], validate_ns: Option<u64>) {
        for v in violations {
            let field = v.field.clone().unwrap_or_default();
            *self.per_rule.entry((field, v.rule.name())).or_insert(0) += 1;
        }
        if let Some(ns) = validate_ns {
            let idx = (64 - ns.max(1).leading_zeros() as usize).min(self.latency_ns.len() - 1);
            self.latency_ns[idx] += 1;
        }
        let slot = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() / RECENT_BUCKET_SECS)
            .unwrap_or(0);
        match self.recent.back_mut() {
            Some(b) if b.slot == slot => {
                b.records += 1;
                b.blocked += u64::from(blocked);
            }
            _ => {
                self.recent.push_back(RecentBucket {
                    slot,
                    records: 1,
                    blocked: u64::from(blocked),
                });
                while self.recent.len() > RECENT_BUCKETS {
                    self.recent.pop_front();
                }
            }
        }
    }

    fn maybe_write(&mut self, stats: &GateStats) {
        if !self.broken && self.last_write.elapsed() >= WRITE_EVERY {
            self.write(stats);
        }
    }

    /// Bucketed p99 of per-record validate time, in microseconds (upper
    /// bound of the bucket the 99th percentile falls in).
    fn p99_micros(&self) -> Option<f64> {
        let total: u64 = self.latency_ns.iter().sum();
        if total == 0 {
            return None;
        }
        let target = (total as f64 * 0.99).ceil() as u64;
        let mut cum = 0u64;
        for (idx, count) in self.latency_ns.iter().enumerate() {
            cum += count;
            if cum >= target {
                return Some((1u64 << idx) as f64 / 1000.0);
            }
        }
        None
    }

    fn write(&mut self, stats: &GateStats) {
        // Every caller obeys the one-warning promise, including the final
        // end-of-run write — a persistently failing path must not warn twice.
        if self.broken {
            return;
        }
        let mut rules: Vec<_> = self
            .per_rule
            .iter()
            .map(|((field, rule), count)| {
                serde_json::json!({ "field": field, "rule": rule, "count": count })
            })
            .collect();
        rules.sort_by_key(|r| std::cmp::Reverse(r["count"].as_u64().unwrap_or(0)));
        let recent: Vec<_> = self
            .recent
            .iter()
            .map(|b| serde_json::json!({ "records": b.records, "blocked": b.blocked }))
            .collect();
        let snapshot = serde_json::json!({
            "covenant_gate_stats": 1,
            "contract": self.contract_id,
            "version": self.contract_version,
            "model": self.model,
            "started_at": self.started_at,
            "updated_at": now_rfc3339(),
            "records": stats.records,
            "passed": stats.passed,
            "blocked": stats.blocked,
            "warned": stats.warned,
            "p99_validate_micros": self.p99_micros(),
            "per_rule": rules,
            "recent": recent,
        });
        let tmp = self.path.with_extension("tmp");
        let result = std::fs::write(&tmp, snapshot.to_string())
            .and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = result {
            eprintln!(
                "covenant gate: WARNING — stats snapshot {} failed ({e}); stats disabled for this run",
                self.path.display()
            );
            self.broken = true;
        }
        self.last_write = Instant::now();
    }
}

/// Run the gate: read NDJSON from `input`, write clean records to `output`,
/// dead-letter envelopes to `dlq`. Uniqueness tracking is exact and
/// in-memory; the CLI surfaces a flag to disable it for unbounded streams.
pub fn run<R: BufRead, W: Write, D: Write>(
    contract: &CompiledContract,
    model: &CompiledModel,
    input: R,
    output: &mut W,
    dlq: &mut D,
    track_unique: bool,
) -> Result<GateOutcome> {
    run_with_stats(contract, model, input, output, dlq, track_unique, None)
}

/// [`run`], optionally writing periodic [`StatsSink`] snapshots for
/// `covenant serve`'s `GET /v1/gate/stats`.
pub fn run_with_stats<R: BufRead, W: Write, D: Write>(
    contract: &CompiledContract,
    model: &CompiledModel,
    mut input: R,
    output: &mut W,
    dlq: &mut D,
    track_unique: bool,
    mut sink: Option<&mut StatsSink>,
) -> Result<GateOutcome> {
    let mut stats = GateStats::default();
    let mut unique = track_unique.then(|| UniqueTracker::new(model));
    let mut violations: Vec<Violation> = Vec::new();
    let mut line = String::new();
    let block = contract.policy.on_violation == OnViolation::Block;
    let mut total_violations: u64 = 0;

    let io_err = |e: std::io::Error| CovenantError::Io {
        path: "<gate stream>".to_string(),
        source: e,
    };

    let mut raw: Vec<u8> = Vec::new();
    loop {
        line.clear();
        raw.clear();
        // Bounded read, as BYTES: a record with no newline (or one enormous
        // line) must not grow the buffer without limit — and the byte cap can
        // land mid-UTF-8-character, where `read_line` would abort the whole
        // run with InvalidData. Raw bytes + lossy conversion dead-letter such
        // records instead; the gate must never be the component that dies.
        let read = {
            let mut limited = input.by_ref().take(MAX_RECORD_BYTES + 1);
            limited.read_until(b'\n', &mut raw).map_err(io_err)?
        };
        if read == 0 {
            break;
        }
        // A read that ends at a newline is a complete record even when it
        // filled the cap exactly; only a capped read WITHOUT its newline was
        // truncated mid-record. (Draining on a complete record would consume
        // and silently destroy the following record.)
        let complete = raw.last() == Some(&b'\n');
        if read as u64 > MAX_RECORD_BYTES && !complete {
            line.push_str(&String::from_utf8_lossy(&raw));
            // Drain the rest of the oversized record so the next iteration
            // starts on a real record boundary, then dead-letter a truncated
            // preview. Never forwarded downstream — even under `warn` — since
            // only a truncated prefix was kept.
            let mut discard = Vec::new();
            input.read_until(b'\n', &mut discard).map_err(io_err)?;
            let row = stats.records;
            stats.records += 1;
            stats.blocked += 1;
            total_violations += 1;
            let oversize_violation = [Violation {
                model: model.name.clone(),
                field: None,
                rule: crate::report::Rule::RecordNotObject,
                row: Some(row),
                value: Some(crate::report::truncate(&line, 64)),
                message: format!(
                    "row {row}: record exceeds {MAX_RECORD_BYTES} bytes and was dead-lettered unparsed (truncated preview kept)"
                ),
            }];
            let envelope = DlqEnvelope {
                contract_id: &contract.id,
                contract_version: &contract.version,
                model: &model.name,
                row,
                ts: now_rfc3339(),
                record: serde_json::Value::String(crate::report::truncate(&line, 256)),
                violations: &oversize_violation,
            };
            serde_json::to_writer(&mut *dlq, &envelope).map_err(|e| CovenantError::Io {
                path: "<dlq>".to_string(),
                source: std::io::Error::other(e),
            })?;
            dlq.write_all(b"\n").map_err(io_err)?;
            if let Some(s) = sink.as_deref_mut() {
                s.on_record(true, &oversize_violation, None);
                s.maybe_write(&stats);
            }
            continue;
        }
        // Strict UTF-8 for records that get parsed and forwarded: a lossy
        // conversion would ALTER the record (invalid bytes become U+FFFD,
        // which can still be valid JSON that passes the contract), and the
        // gate must never forward modified data. Invalid records dead-letter;
        // under `warn` the ORIGINAL bytes flow through untouched.
        match std::str::from_utf8(&raw) {
            Ok(text) => line.push_str(text),
            Err(e) => {
                let row = stats.records;
                stats.records += 1;
                total_violations += 1;
                violations.clear();
                let preview = String::from_utf8_lossy(&raw);
                violations.push(Violation {
                    model: model.name.clone(),
                    field: None,
                    rule: crate::report::Rule::RecordNotObject,
                    row: Some(row),
                    value: Some(crate::report::truncate(&preview, 64)),
                    message: format!("row {row}: record is not valid UTF-8 ({e})"),
                });
                let envelope = DlqEnvelope {
                    contract_id: &contract.id,
                    contract_version: &contract.version,
                    model: &model.name,
                    row,
                    ts: now_rfc3339(),
                    record: serde_json::Value::String(crate::report::truncate(&preview, 256)),
                    violations: &violations,
                };
                serde_json::to_writer(&mut *dlq, &envelope).map_err(|e| CovenantError::Io {
                    path: "<dlq>".to_string(),
                    source: std::io::Error::other(e),
                })?;
                dlq.write_all(b"\n").map_err(io_err)?;
                if block {
                    stats.blocked += 1;
                } else {
                    output.write_all(&raw).map_err(io_err)?;
                    if !complete {
                        output.write_all(b"\n").map_err(io_err)?;
                    }
                    stats.warned += 1;
                    stats.passed += 1;
                }
                if let Some(s) = sink.as_deref_mut() {
                    s.on_record(block, &violations, None);
                    s.maybe_write(&stats);
                }
                continue;
            }
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.trim().is_empty() {
            continue;
        }
        let row = stats.records;
        stats.records += 1;
        violations.clear();

        // The sink's "added latency" is the real per-record cost the gate
        // puts in the pipe: parse + validate, measured only when a sink is
        // attached so the bare gate pays nothing.
        let t0 = sink.is_some().then(Instant::now);
        let record = match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(record) => {
                row::validate_record(model, &record, row, unique.as_mut(), &mut violations);
                Some(record)
            }
            Err(e) => {
                violations.push(Violation {
                    model: model.name.clone(),
                    field: None,
                    rule: crate::report::Rule::RecordNotObject,
                    row: Some(row),
                    value: Some(crate::report::truncate(trimmed, 64)),
                    message: format!("row {row}: invalid JSON: {e}"),
                });
                None
            }
        };
        let validate_ns = t0.map(|t| t.elapsed().as_nanos() as u64);

        if violations.is_empty() {
            output.write_all(trimmed.as_bytes()).map_err(io_err)?;
            output.write_all(b"\n").map_err(io_err)?;
            stats.passed += 1;
            if let Some(s) = sink.as_deref_mut() {
                s.on_record(false, &[], validate_ns);
                s.maybe_write(&stats);
            }
            continue;
        }

        total_violations += violations.len() as u64;
        let envelope = DlqEnvelope {
            contract_id: &contract.id,
            contract_version: &contract.version,
            model: &model.name,
            row,
            ts: now_rfc3339(),
            record: record.unwrap_or(serde_json::Value::String(trimmed.to_string())),
            violations: &violations,
        };
        serde_json::to_writer(&mut *dlq, &envelope).map_err(|e| CovenantError::Io {
            path: "<dlq>".to_string(),
            source: std::io::Error::other(e),
        })?;
        dlq.write_all(b"\n").map_err(io_err)?;

        if block {
            stats.blocked += 1;
        } else {
            // Policy warn: the record still flows downstream.
            output.write_all(trimmed.as_bytes()).map_err(io_err)?;
            output.write_all(b"\n").map_err(io_err)?;
            stats.warned += 1;
            stats.passed += 1;
        }
        if let Some(s) = sink.as_deref_mut() {
            s.on_record(block, &violations, validate_ns);
            s.maybe_write(&stats);
        }
    }
    output.flush().map_err(io_err)?;
    dlq.flush().map_err(io_err)?;
    // Final snapshot so the file always reflects the finished run, even for
    // short streams that never crossed the write interval.
    if let Some(s) = sink {
        s.write(&stats);
    }

    let failed = block && total_violations > contract.policy.max_violations;
    Ok(GateOutcome { stats, failed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::CompiledContract;
    use crate::spec::Contract;
    use std::io::Cursor;

    fn compiled() -> CompiledContract {
        let doc = Contract::parse(
            "covenant: 1\nid: t\nversion: 1.0.0\nmodels: { m: { fields: { a: { type: string } } } }\n",
            "<test>",
        )
        .unwrap();
        CompiledContract::compile(&doc).unwrap()
    }

    fn run_gate(input: Vec<u8>) -> (GateOutcome, Vec<u8>, Vec<u8>) {
        let contract = compiled();
        let model = contract.resolve_model(None).unwrap();
        let mut out = Vec::new();
        let mut dlq = Vec::new();
        let outcome = run(
            &contract,
            model,
            Cursor::new(input),
            &mut out,
            &mut dlq,
            true,
        )
        .unwrap();
        (outcome, out, dlq)
    }

    /// A record whose bytes (incl. the newline) land exactly on the read cap
    /// is complete — it must pass, and the drain must NOT consume the record
    /// after it.
    #[test]
    fn exact_boundary_record_does_not_eat_the_next_record() {
        // `{"a":""}` is 8 bytes of wrapper; payload sized so content ==
        // MAX_RECORD_BYTES and the newline is byte MAX_RECORD_BYTES + 1.
        let filler = "x".repeat((MAX_RECORD_BYTES - 8) as usize);
        let mut input = format!("{{\"a\":\"{filler}\"}}\n").into_bytes();
        assert_eq!(input.len() as u64, MAX_RECORD_BYTES + 1);
        input.extend_from_slice(b"{\"a\":\"second\"}\n");

        let (outcome, out, dlq) = run_gate(input);
        assert_eq!(outcome.stats.records, 2);
        assert_eq!(
            outcome.stats.passed,
            2,
            "dlq: {}",
            String::from_utf8_lossy(&dlq[..dlq.len().min(200)])
        );
        assert!(String::from_utf8_lossy(&out).contains("second"));
    }

    /// A genuinely oversized record is dead-lettered (truncated) and the
    /// following record still flows.
    #[test]
    fn oversized_record_is_dead_lettered_and_next_survives() {
        let filler = "x".repeat((MAX_RECORD_BYTES + 100) as usize);
        let mut input = format!("{{\"a\":\"{filler}\"}}\n").into_bytes();
        input.extend_from_slice(b"{\"a\":\"second\"}\n");

        let (outcome, out, dlq) = run_gate(input);
        assert_eq!(outcome.stats.records, 2);
        assert_eq!(outcome.stats.blocked, 1);
        assert_eq!(outcome.stats.passed, 1);
        assert!(String::from_utf8_lossy(&out).contains("second"));
        assert!(String::from_utf8_lossy(&dlq).contains("exceeds"));
    }

    /// Invalid UTF-8 (including a cap that splits a multi-byte character) is
    /// a dead letter, never a fatal error that kills the gate.
    #[test]
    fn invalid_utf8_is_dead_lettered_not_fatal() {
        let mut input: Vec<u8> = Vec::new();
        input.extend_from_slice(b"{\"a\":\"ok\"}\n");
        input.extend_from_slice(&[0xff, 0xfe, b'\n']);
        input.extend_from_slice(b"{\"a\":\"ok2\"}\n");

        let (outcome, out, _dlq) = run_gate(input);
        assert_eq!(outcome.stats.records, 3);
        assert_eq!(outcome.stats.passed, 2);
        assert_eq!(outcome.stats.blocked, 1);
        assert!(String::from_utf8_lossy(&out).contains("ok2"));
    }

    /// The stats sidecar writes a parseable final snapshot whose counters
    /// match the run, with per-rule counts — and leaves no tmp file behind.
    #[test]
    fn stats_sink_writes_a_final_snapshot_matching_the_run() {
        let contract = compiled();
        let model = contract.resolve_model(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.json");
        let mut sink = StatsSink::new(path.clone(), &contract, model);
        let input = b"{\"a\":\"ok\"}\n{\"a\": 1}\nnot json\n".to_vec();
        let mut out = Vec::new();
        let mut dlq = Vec::new();
        let outcome = run_with_stats(
            &contract,
            model,
            Cursor::new(input),
            &mut out,
            &mut dlq,
            true,
            Some(&mut sink),
        )
        .unwrap();

        let snapshot: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(snapshot["covenant_gate_stats"], 1);
        assert_eq!(snapshot["contract"], "t");
        assert_eq!(snapshot["records"].as_u64(), Some(outcome.stats.records));
        assert_eq!(snapshot["blocked"].as_u64(), Some(outcome.stats.blocked));
        assert_eq!(snapshot["passed"].as_u64(), Some(outcome.stats.passed));
        assert_eq!(outcome.stats.blocked, 2, "type mismatch + invalid JSON");
        let rules: Vec<&str> = snapshot["per_rule"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["rule"].as_str().unwrap())
            .collect();
        assert!(rules.contains(&"type_mismatch"), "{rules:?}");
        assert!(rules.contains(&"record_not_object"), "{rules:?}");
        assert!(snapshot["p99_validate_micros"].as_f64().is_some());
        assert!(!snapshot["recent"].as_array().unwrap().is_empty());
        assert!(
            !path.with_extension("tmp").exists(),
            "tmp file must be renamed away"
        );
    }

    /// Invalid UTF-8 *inside a quoted JSON string* must not be lossy-repaired
    /// into a passing record — the gate never forwards altered data.
    #[test]
    fn invalid_utf8_in_json_string_is_not_lossy_accepted() {
        let mut input: Vec<u8> = Vec::new();
        input.extend_from_slice(b"{\"a\":\"");
        input.push(0xff);
        input.extend_from_slice(b"\"}\n");
        input.extend_from_slice(b"{\"a\":\"ok\"}\n");

        let (outcome, out, dlq) = run_gate(input);
        assert_eq!(outcome.stats.records, 2);
        assert_eq!(outcome.stats.blocked, 1);
        assert_eq!(outcome.stats.passed, 1);
        let out_s = String::from_utf8_lossy(&out);
        assert!(
            !out_s.contains('\u{FFFD}'),
            "an altered (replacement-character) record must never be forwarded: {out_s}"
        );
        assert!(String::from_utf8_lossy(&dlq).contains("not valid UTF-8"));
    }
}
