//! The stream gate: NDJSON in on stdin, clean records out on stdout,
//! violations to a dead-letter sink. Transport-agnostic on purpose — pipe it
//! between `kcat -C` and `kcat -P` and it is a Kafka SMT; put it in a
//! CronJob reading a dump and it is a warehouse pre-load hook; build the
//! `kafka` feature and `crate::kafka` runs it over topics directly. Every
//! transport judges records through [`RecordGate`].
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
use crate::report::{Collector, Violation};
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

    pub(crate) fn on_record(
        &mut self,
        blocked: bool,
        violations: &[Violation],
        validate_ns: Option<u64>,
    ) {
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

    pub(crate) fn maybe_write(&mut self, stats: &GateStats) {
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

    pub(crate) fn write(&mut self, stats: &GateStats) {
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

/// What a gate decides about one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The record keeps the contract, and goes on.
    Pass,
    /// It breaks the contract under `on_violation: block`: it is withheld.
    Block,
    /// It breaks the contract under `on_violation: warn`: it goes on, and its
    /// dead letter reports it.
    Warn,
}

/// The gate's judgement, one record at a time. Every way a record reaches a
/// gate — a line of a stream, a subprocess message, a Kafka message, a record
/// in a broker's transform — gets its verdict and its dead letter from here,
/// so they cannot drift apart. It keeps what spans the stream: uniqueness,
/// row numbers, the counts and the violation budget.
pub struct RecordGate<'c> {
    contract: &'c CompiledContract,
    model: &'c CompiledModel,
    block: bool,
    unique: Option<UniqueTracker>,
    stats: GateStats,
    total_violations: u64,
    row: u64,
    violations: Vec<Violation>,
    record: serde_json::Value,
}

impl<'c> RecordGate<'c> {
    /// A gate over one stream. `track_unique` keeps every `unique` key the
    /// stream has shown, exactly, in memory.
    pub fn new(
        contract: &'c CompiledContract,
        model: &'c CompiledModel,
        track_unique: bool,
    ) -> Self {
        RecordGate {
            contract,
            model,
            block: contract.policy.on_violation == OnViolation::Block,
            unique: track_unique.then(|| UniqueTracker::new(model)),
            stats: GateStats::default(),
            total_violations: 0,
            row: 0,
            violations: Vec::new(),
            record: serde_json::Value::Null,
        }
    }

    /// Judge one record: the bytes of one JSON object, a trailing line end
    /// allowed. A record that is not UTF-8, not JSON or not an object breaks
    /// the contract too.
    pub fn judge(&mut self, record: &[u8]) -> Verdict {
        let row = self.next_row();
        match std::str::from_utf8(record) {
            Ok(text) => {
                let text = text.trim_end_matches(['\n', '\r']);
                match serde_json::from_str::<serde_json::Value>(text) {
                    Ok(value) => {
                        row::validate_record(
                            self.model,
                            &value,
                            row,
                            self.unique.as_mut(),
                            &mut self.violations,
                        );
                        self.record = value;
                    }
                    Err(e) => {
                        self.unreadable(text, format!("row {row}: invalid JSON: {e}"));
                        self.record = serde_json::Value::String(text.to_string());
                    }
                }
            }
            Err(e) => {
                let preview = String::from_utf8_lossy(record);
                self.unreadable(
                    &preview,
                    format!("row {row}: record is not valid UTF-8 ({e})"),
                );
                self.record = serde_json::Value::String(crate::report::truncate(&preview, 256));
            }
        }
        if self.violations.is_empty() {
            self.stats.passed += 1;
            return Verdict::Pass;
        }
        self.total_violations += self.violations.len() as u64;
        if self.block {
            self.stats.blocked += 1;
            Verdict::Block
        } else {
            self.stats.warned += 1;
            self.stats.passed += 1;
            Verdict::Warn
        }
    }

    /// A record too large to read, withheld whatever the policy, from a
    /// preview of it.
    pub(crate) fn oversized(&mut self, preview: &str) -> Verdict {
        let row = self.next_row();
        self.unreadable(
            preview,
            format!(
                "row {row}: record exceeds {MAX_RECORD_BYTES} bytes and was dead-lettered unparsed (truncated preview kept)"
            ),
        );
        self.record = serde_json::Value::String(crate::report::truncate(preview, 256));
        self.total_violations += 1;
        self.stats.blocked += 1;
        Verdict::Block
    }

    fn next_row(&mut self) -> u64 {
        self.row = self.stats.records;
        self.stats.records += 1;
        self.violations.clear();
        self.row
    }

    fn unreadable(&mut self, text: &str, message: String) {
        self.violations.push(Violation {
            model: self.model.name.clone(),
            field: None,
            rule: crate::report::Rule::RecordNotObject,
            row: Some(self.row),
            value: Some(crate::report::truncate(text, 64)),
            message,
            // Not a value of any type: a line that is not JSON at all.
            observed: None,
        });
    }

    /// The violations of the record just judged.
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }

    /// The dead letter for the record just judged, stamped now.
    pub fn dead_letter(&self) -> DlqEnvelope<'_> {
        DlqEnvelope {
            contract_id: &self.contract.id,
            contract_version: &self.contract.version,
            model: &self.model.name,
            row: self.row,
            ts: now_rfc3339(),
            record: self.record.clone(),
            violations: &self.violations,
        }
    }

    /// The counts so far.
    pub fn stats(&self) -> &GateStats {
        &self.stats
    }

    /// Whether enforcement fails the stream so far: under `block`, more
    /// violations than the contract's budget.
    pub fn failed(&self) -> bool {
        self.block && self.total_violations > self.contract.policy.max_violations
    }

    /// The counts, at the end of the stream.
    pub fn into_stats(self) -> GateStats {
        self.stats
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
    run_with_stats(
        contract,
        model,
        input,
        output,
        dlq,
        track_unique,
        None,
        None,
    )
}

/// [`run`], optionally writing periodic [`StatsSink`] snapshots for
/// `covenant serve`'s `GET /v1/gate/stats`, and counting every violation
/// into `report` for the run's `covenant-report/v1` document.
#[allow(clippy::too_many_arguments)]
pub fn run_with_stats<R: BufRead, W: Write, D: Write>(
    contract: &CompiledContract,
    model: &CompiledModel,
    input: R,
    output: &mut W,
    dlq: &mut D,
    track_unique: bool,
    sink: Option<&mut StatsSink>,
    report: Option<&mut Collector>,
) -> Result<GateOutcome> {
    let mode = Mode {
        track_unique,
        sink,
        replies: None,
        report,
    };
    run_inner(contract, model, input, output, dlq, mode)
}

/// [`run`] as a subprocess filter (`covenant gate --subprocess`): every line
/// in gets exactly one line back, flushed at once. A record that goes on is
/// answered on `output`; a record the gate withholds is answered on
/// `replies` with its dead-letter envelope; a blank line gets a blank line.
/// That is the protocol of Redpanda Connect's `subprocess` processor, where
/// a reply on stdout replaces the message and a reply on stderr marks it
/// failed. Dead letters still go to `dlq` too, and a record passed under
/// `on_violation: warn` has its envelope there alone. Every violation is
/// counted into `report`, when given, for the run's document.
#[allow(clippy::too_many_arguments)]
pub fn run_as_subprocess<R: BufRead, W: Write, D: Write>(
    contract: &CompiledContract,
    model: &CompiledModel,
    input: R,
    output: &mut W,
    replies: &mut dyn Write,
    dlq: &mut D,
    track_unique: bool,
    report: Option<&mut Collector>,
) -> Result<GateOutcome> {
    let mode = Mode {
        track_unique,
        sink: None,
        replies: Some(replies),
        report,
    };
    run_inner(contract, model, input, output, dlq, mode)
}

/// What a gate run tracks, and how it answers.
struct Mode<'a> {
    track_unique: bool,
    sink: Option<&'a mut StatsSink>,
    /// Subprocess mode: where a withheld record is answered.
    replies: Option<&'a mut dyn Write>,
    /// Counts every violation, and keeps the contract's samples of them, for
    /// the run's report document.
    report: Option<&'a mut Collector>,
}

/// Dead-letter a record and, in subprocess mode, flush it; a withheld
/// record is also answered with the same line.
fn dead_letter<D: Write>(
    dlq: &mut D,
    replies: &mut Option<&mut dyn Write>,
    envelope: &DlqEnvelope<'_>,
    withheld: bool,
) -> Result<()> {
    let io = |path: &'static str| {
        move |e: std::io::Error| CovenantError::Io {
            path: path.to_string(),
            source: e,
        }
    };
    let mut line = serde_json::to_vec(envelope).map_err(|e| CovenantError::Io {
        path: "<dlq>".to_string(),
        source: std::io::Error::other(e),
    })?;
    line.push(b'\n');
    dlq.write_all(&line).map_err(io("<dlq>"))?;
    if let Some(replies) = replies.as_deref_mut() {
        dlq.flush().map_err(io("<dlq>"))?;
        if withheld {
            replies.write_all(&line).map_err(io("<gate replies>"))?;
            replies.flush().map_err(io("<gate replies>"))?;
        }
    }
    Ok(())
}

/// Count a record's violations into the run's report, when there is one.
pub(crate) fn count_into(report: Option<&mut Collector>, violations: &[Violation]) {
    if let Some(report) = report {
        for v in violations {
            report.record(v.field.as_deref(), v.rule, || v.clone());
        }
    }
}

fn run_inner<R: BufRead, W: Write, D: Write>(
    contract: &CompiledContract,
    model: &CompiledModel,
    mut input: R,
    output: &mut W,
    dlq: &mut D,
    mode: Mode<'_>,
) -> Result<GateOutcome> {
    let Mode {
        track_unique,
        mut sink,
        mut replies,
        mut report,
    } = mode;
    let mut gate = RecordGate::new(contract, model, track_unique);

    let io_err = |e: std::io::Error| CovenantError::Io {
        path: "<gate stream>".to_string(),
        source: e,
    };

    let mut raw: Vec<u8> = Vec::new();
    loop {
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
            let preview = String::from_utf8_lossy(&raw).into_owned();
            // Drain the rest of the oversized record so the next iteration
            // starts on a real record boundary, then dead-letter a truncated
            // preview. Never forwarded downstream — even under `warn` — since
            // only a truncated prefix was kept.
            let mut discard = Vec::new();
            input.read_until(b'\n', &mut discard).map_err(io_err)?;
            gate.oversized(&preview);
            count_into(report.as_deref_mut(), gate.violations());
            dead_letter(dlq, &mut replies, &gate.dead_letter(), true)?;
            if let Some(s) = sink.as_deref_mut() {
                s.on_record(true, gate.violations(), None);
                s.maybe_write(gate.stats());
            }
            continue;
        }
        // Strict UTF-8 for records that get parsed and forwarded: a lossy
        // conversion would ALTER the record (invalid bytes become U+FFFD,
        // which can still be valid JSON that passes the contract), and the
        // gate must never forward modified data. Invalid records dead-letter;
        // under `warn` the ORIGINAL bytes flow through untouched.
        let trimmed = std::str::from_utf8(&raw)
            .ok()
            .map(|text| text.trim_end_matches(['\n', '\r']));
        if trimmed.is_some_and(|t| t.trim().is_empty()) {
            // Not a record, but in subprocess mode every line is answered.
            if replies.is_some() {
                output.write_all(b"\n").map_err(io_err)?;
                output.flush().map_err(io_err)?;
            }
            continue;
        }

        // The sink's "added latency" is the real per-record cost the gate
        // puts in the pipe: parse + validate, measured only when a sink is
        // attached so the bare gate pays nothing.
        let t0 = (sink.is_some() && trimmed.is_some()).then(Instant::now);
        let verdict = gate.judge(&raw);
        let validate_ns = t0.map(|t| t.elapsed().as_nanos() as u64);
        count_into(report.as_deref_mut(), gate.violations());

        if verdict != Verdict::Pass {
            dead_letter(
                dlq,
                &mut replies,
                &gate.dead_letter(),
                verdict == Verdict::Block,
            )?;
        }
        if verdict != Verdict::Block {
            // The record goes on: without its line end, or, when it is not
            // UTF-8 (only `warn` lets one through), exactly as it arrived.
            match trimmed {
                Some(text) => {
                    output.write_all(text.as_bytes()).map_err(io_err)?;
                    output.write_all(b"\n").map_err(io_err)?;
                }
                None => {
                    output.write_all(&raw).map_err(io_err)?;
                    if !complete {
                        output.write_all(b"\n").map_err(io_err)?;
                    }
                }
            }
            if replies.is_some() {
                output.flush().map_err(io_err)?;
            }
        }
        if let Some(s) = sink.as_deref_mut() {
            s.on_record(verdict == Verdict::Block, gate.violations(), validate_ns);
            s.maybe_write(gate.stats());
        }
    }
    output.flush().map_err(io_err)?;
    dlq.flush().map_err(io_err)?;
    // Final snapshot so the file always reflects the finished run, even for
    // short streams that never crossed the write interval.
    if let Some(s) = sink {
        s.write(gate.stats());
    }

    let failed = gate.failed();
    Ok(GateOutcome {
        stats: gate.into_stats(),
        failed,
    })
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
            None,
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

    #[test]
    fn a_report_collector_counts_every_violation_the_gate_judged() {
        let contract = compiled();
        let model = contract.resolve_model(None).unwrap();
        // Two type mismatches, one line that is not JSON, a blank line (not a
        // record) and a clean record.
        let input = b"{\"a\": 1}\nnot json\n\n{\"a\":\"ok\"}\n{\"a\": 2}\n".to_vec();
        let mut collector = Collector::new(1);
        let outcome = run_with_stats(
            &contract,
            model,
            Cursor::new(input),
            &mut Vec::new(),
            &mut Vec::new(),
            true,
            None,
            Some(&mut collector),
        )
        .unwrap();
        assert_eq!(collector.total(), 3);
        let header = crate::report::ReportHeader {
            contract_id: "t".into(),
            contract_version: "1.0.0".into(),
            owner: None,
            model: "m".into(),
            source: "stdin".into(),
        };
        let report = collector.into_report(header, outcome.stats.records);
        assert_eq!(report.rows, 4, "the blank line is not a record");
        let counts: Vec<(&str, &str, u64)> = report
            .per_rule
            .iter()
            .map(|c| (c.field.as_str(), c.rule, c.count))
            .collect();
        assert_eq!(
            counts,
            [("a", "type_mismatch", 2), ("", "record_not_object", 1)]
        );
        // The contract's cap (1 here) bounds the samples per field and rule;
        // the rows are the gate's own, as its dead letters give them.
        let rows: Vec<Option<u64>> = report.samples.iter().map(|v| v.row).collect();
        assert_eq!(rows, [Some(0), Some(1)]);
    }

    fn compiled_with(policy: &str) -> CompiledContract {
        let doc = Contract::parse(
            &format!(
                "covenant: 1\nid: t\nversion: 1.0.0\npolicy: {{ {policy} }}\n\
                 models: {{ m: {{ fields: {{ id: {{ type: string, unique: true }}, n: {{ type: integer, min: 0 }} }} }} }}\n"
            ),
            "<test>",
        )
        .unwrap();
        CompiledContract::compile(&doc).unwrap()
    }

    /// One record at a time: its verdict, its violations and its dead letter,
    /// with row numbers counting every record judged.
    #[test]
    fn record_gate_judges_one_record_at_a_time() {
        let contract = compiled_with("on_violation: block");
        let model = contract.resolve_model(None).unwrap();
        let mut gate = RecordGate::new(&contract, model, true);

        assert_eq!(gate.judge(br#"{"id":"a","n":1}"#), Verdict::Pass);
        assert!(gate.violations().is_empty());

        assert_eq!(gate.judge(b"{\"id\":\"b\",\"n\":-1}\r\n"), Verdict::Block);
        assert_eq!(gate.violations().len(), 1);
        assert_eq!(gate.violations()[0].rule, crate::report::Rule::Min);
        let letter = serde_json::to_value(gate.dead_letter()).unwrap();
        assert_eq!(letter["row"], 1);
        assert_eq!(letter["contract_id"], "t");
        assert_eq!(letter["model"], "m");
        assert_eq!(letter["record"], serde_json::json!({ "id": "b", "n": -1 }));
        assert_eq!(letter["violations"][0]["rule"], "min");

        assert_eq!(gate.judge(b"not json"), Verdict::Block);
        let letter = serde_json::to_value(gate.dead_letter()).unwrap();
        assert_eq!(letter["row"], 2);
        assert_eq!(letter["record"], "not json");
        assert_eq!(letter["violations"][0]["rule"], "record_not_object");

        let stats = gate.stats();
        assert_eq!(
            (stats.records, stats.passed, stats.blocked, stats.warned),
            (3, 1, 2, 0)
        );
        assert!(gate.failed());
    }

    /// `unique` spans the records a gate has judged, when it tracks them.
    #[test]
    fn record_gate_tracks_unique_keys_across_records() {
        let contract = compiled_with("on_violation: block");
        let model = contract.resolve_model(None).unwrap();

        let mut tracking = RecordGate::new(&contract, model, true);
        assert_eq!(tracking.judge(br#"{"id":"a"}"#), Verdict::Pass);
        assert_eq!(tracking.judge(br#"{"id":"a"}"#), Verdict::Block);
        assert_eq!(tracking.violations()[0].rule, crate::report::Rule::Unique);

        let mut not_tracking = RecordGate::new(&contract, model, false);
        assert_eq!(not_tracking.judge(br#"{"id":"a"}"#), Verdict::Pass);
        assert_eq!(not_tracking.judge(br#"{"id":"a"}"#), Verdict::Pass);
    }

    /// Under `warn` a violating record goes on and is still reported; the
    /// stream never fails.
    #[test]
    fn record_gate_under_warn_passes_and_reports() {
        let contract = compiled_with("on_violation: warn");
        let model = contract.resolve_model(None).unwrap();
        let mut gate = RecordGate::new(&contract, model, true);

        assert_eq!(gate.judge(br#"{"n":-1}"#), Verdict::Warn);
        assert_eq!(gate.violations().len(), 1);
        assert_eq!(gate.judge(&[0xff, 0xfe]), Verdict::Warn);
        let letter = serde_json::to_value(gate.dead_letter()).unwrap();
        assert_eq!(letter["violations"][0]["rule"], "record_not_object");
        let stats = gate.stats();
        assert_eq!(
            (stats.records, stats.passed, stats.blocked, stats.warned),
            (2, 2, 0, 2)
        );
        assert!(!gate.failed());
    }

    /// The contract's violation budget: the stream fails only past it.
    #[test]
    fn record_gate_fails_past_the_budget() {
        let contract = compiled_with("on_violation: block, max_violations: 1");
        let model = contract.resolve_model(None).unwrap();
        let mut gate = RecordGate::new(&contract, model, false);

        assert_eq!(gate.judge(br#"{"n":-1}"#), Verdict::Block);
        assert!(!gate.failed(), "one violation is within the budget");
        assert_eq!(gate.judge(br#"{"n":-2}"#), Verdict::Block);
        assert!(gate.failed());
        assert_eq!(gate.into_stats().blocked, 2);
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
