//! The stable surface, pinned. Every item `covenant::prelude` exports is
//! named here with its full signature, so a change that would break an
//! embedder does not compile: it has to be made on purpose, with the
//! version bump `docs/API.md` asks for, and this file edited in the same
//! change.

// Spelling each signature out in full is the point of this file.
#![allow(clippy::type_complexity)]

use std::path::Path;

#[cfg(feature = "arrow")]
use arrow_array::RecordBatch;
use covenant::prelude::*;
use serde_json::Value;

#[test]
fn signatures() {
    // Contracts.
    let _: fn(&str, &str) -> Result<LoadedContract> = Contract::load;
    let _: fn(&Path) -> Result<LoadedContract> = Contract::load_path;
    let _: fn(&str, &str) -> Result<Contract> = Contract::parse;
    let _: fn(&Path) -> Result<Contract> = Contract::from_path;
    let _: fn(LoadedContract, &str) -> Result<Contract> = LoadedContract::into_enforceable;
    let _: fn(&LoadedContract, bool) -> Vec<LintFinding> = LoadedContract::lint;
    let _: fn(&Contract) -> Vec<LintFinding> = Contract::lint;

    // Compiling.
    let _: fn(&Contract) -> Result<CompiledContract> = CompiledContract::compile;
    let _: for<'a> fn(&'a CompiledContract, Option<&str>) -> Result<&'a CompiledModel> =
        CompiledContract::resolve_model;

    // Checking.
    let _: fn(
        &CompiledModel,
        &Value,
        u64,
        Option<&mut UniqueTracker>,
        &mut Vec<Violation>,
    ) -> bool = validate_record;
    let _: fn(&CompiledModel) -> UniqueTracker = UniqueTracker::new;
    let _: fn(usize) -> Collector = Collector::new;
    let _: fn(Collector, ReportHeader, u64) -> CheckReport = Collector::into_report;
    let _: fn(&CompiledContract, &CompiledModel, &Path, Option<DataFormat>) -> Result<CheckReport> =
        check_path;
    let _: fn(
        &CompiledContract,
        &CompiledModel,
        &Path,
        Option<DataFormat>,
        Option<&mut Profiler>,
    ) -> Result<CheckReport> = check_path_profiled;
    let _: fn(&Path) -> Result<DataFormat> = DataFormat::infer;
    let _: fn(&CheckReport, u64) -> bool = CheckReport::passed;
    let _: fn(&CheckReport, u64) -> String = CheckReport::render_human;

    // Streams.
    let _: fn(
        &CompiledContract,
        &CompiledModel,
        std::io::Cursor<Vec<u8>>,
        &mut Vec<u8>,
        &mut Vec<u8>,
        bool,
    ) -> Result<GateOutcome> = gate::<std::io::Cursor<Vec<u8>>, Vec<u8>, Vec<u8>>;

    // Contract changes.
    let _: fn(&Contract, &Contract) -> DiffReport = diff;
    let _: fn(&DiffReport) -> Option<Severity> = DiffReport::max_severity;
    let _: fn(&DiffReport) -> String = DiffReport::render_human;

    // Profiles and drift.
    let _: fn(&CompiledContract, &CompiledModel) -> Profiler = Profiler::new;
    let _: fn(&mut Profiler, &CompiledModel, &Value) = Profiler::observe_record;
    let _: fn(Profiler) -> Profile = Profiler::finish;
    let _: fn(&Path) -> Result<Profile> = Profile::from_path;
    let _: fn(&str, &str) -> Result<Profile> = Profile::parse;
    let _: fn(&mut Profile, &Profile) -> Result<()> = Profile::merge;
    let _: fn(&Profile, &Path) -> Result<()> = Profile::write;
    let _: fn(&Profile) -> String = Profile::to_json;
    let _: fn(&Profile) -> ProfileSummary = Profile::summary;
    let _: fn(&Profile, &Profile, &DriftThresholds) -> Result<DriftReport> = drift;
    let _: fn(&DriftReport) -> bool = DriftReport::drifted;
    let _: fn(&DriftReport) -> String = DriftReport::render_human;
}

/// The Arrow half of the surface, present with the `arrow` feature (on by
/// default): batches checked as a source, and profiled.
#[cfg(feature = "arrow")]
#[test]
fn arrow_signatures() {
    let _: fn(
        &CompiledModel,
        &RecordBatch,
        u64,
        Option<&mut UniqueTracker>,
        &mut Collector,
    ) -> u64 = validate_batch;
    let _: fn(
        &CompiledModel,
        &RecordBatch,
        u64,
        Option<&mut UniqueTracker>,
        &mut SchemaFindings,
        &mut Collector,
    ) -> u64 = validate_source_batch;
    let _: fn() -> SchemaFindings = SchemaFindings::new;
    let _: fn() -> SchemaFindings = SchemaFindings::default;
    let _: fn(&mut Profiler, &CompiledModel, &RecordBatch) = Profiler::observe_batch;
}

/// The fields embedders read. Fields may be added (the policy allows it);
/// removing, renaming or retyping one of these fails here.
#[allow(dead_code)]
fn fields(r: &CheckReport, v: &Violation, d: &DiffReport, c: &Change, p: &Profile, f: &Finding) {
    let _: (
        &String,
        &String,
        &Option<String>,
        &String,
        &String,
        u64,
        u64,
    ) = (
        &r.contract_id,
        &r.contract_version,
        &r.owner,
        &r.model,
        &r.source,
        r.rows,
        r.violations,
    );
    let _: (&Vec<RuleCount>, &Vec<Violation>, &Vec<UnenforcedRule>) =
        (&r.per_rule, &r.samples, &r.unenforced);
    let _: (
        &String,
        &Option<String>,
        Rule,
        Option<u64>,
        &Option<String>,
        &String,
    ) = (&v.model, &v.field, v.rule, v.row, &v.value, &v.message);
    let _: (&String, &String, &Vec<Change>) = (&d.old_version, &d.new_version, &d.changes);
    let _: (Severity, Impact, &String, &String) = (c.severity, c.impact, &c.path, &c.message);
    let _: (&String, &String, &String, u64, u64) =
        (&p.contract, &p.contract_version, &p.model, p.runs, p.rows);
    let _: (&Option<String>, Metric, f64, f64, f64, f64, &String) = (
        &f.field,
        f.metric,
        f.baseline,
        f.current,
        f.score,
        f.threshold,
        &f.message,
    );
}

/// A gate judged one record at a time, as a stream processor embeds it, and
/// the counts and dead letters it hands back.
#[allow(dead_code)]
fn record_gate<'c>(_: &RecordGate<'c>, o: &GateOutcome, s: &GateStats, d: &DlqEnvelope<'_>) {
    let _: fn(&'c CompiledContract, &'c CompiledModel, bool) -> RecordGate<'c> = RecordGate::new;
    let _: fn(&mut RecordGate<'c>, &[u8]) -> Verdict = RecordGate::judge;
    let _: for<'a> fn(&'a RecordGate<'c>) -> &'a [Violation] = RecordGate::violations;
    let _: for<'a> fn(&'a RecordGate<'c>) -> DlqEnvelope<'a> = RecordGate::dead_letter;
    let _: for<'a> fn(&'a RecordGate<'c>) -> &'a GateStats = RecordGate::stats;
    let _: fn(&RecordGate<'c>) -> bool = RecordGate::failed;
    let _: fn(RecordGate<'c>) -> GateStats = RecordGate::into_stats;
    let _: [Verdict; 3] = [Verdict::Pass, Verdict::Block, Verdict::Warn];

    let _: (&GateStats, bool) = (&o.stats, o.failed);
    let _: (u64, u64, u64, u64) = (s.records, s.passed, s.blocked, s.warned);
    let _: (&str, &str, &str, u64, &String, &Value, &[Violation]) = (
        d.contract_id,
        d.contract_version,
        d.model,
        d.row,
        &d.ts,
        &d.record,
        d.violations,
    );
}

/// The contract side of the same promise, and the types an embedder names
/// only in a signature or a `match`: removing one from the prelude, or
/// renaming or retyping one of these fields, fails here.
#[allow(dead_code)]
fn contract_fields(l: &LoadedContract, lint: &LintFinding, n: &NotCompared, e: CovenantError) {
    let _: (&Contract, &SourceFormat, &Vec<UnenforcedRule>) =
        (&l.contract, &l.format, &l.unenforced);
    let _: &Vec<_> = &l.notes;
    let _: (LintLevel, &String, &String) = (lint.level, &lint.path, &lint.message);
    let _: (&String, &String) = (&n.field, &n.reason);
    let _: &dyn std::error::Error = &e;
}

#[test]
fn the_prelude_is_enough_to_check_data() {
    let yaml = "covenant: 1\nid: t\nversion: 1.0.0\nmodels:\n  t:\n    fields:\n      a: { type: integer, min: 0 }\n";
    let contract = CompiledContract::compile(&Contract::parse(yaml, "t.yaml").unwrap()).unwrap();
    let model = contract.resolve_model(None).unwrap();
    let mut out = Vec::new();
    assert!(validate_record(
        model,
        &serde_json::json!({ "a": 1 }),
        0,
        None,
        &mut out
    ));
    assert!(!validate_record(
        model,
        &serde_json::json!({ "a": -1 }),
        1,
        None,
        &mut out
    ));
    assert_eq!(out[0].rule, Rule::Min);
}
