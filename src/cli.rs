//! The `covenant` CLI — thin argument parsing and exit-code discipline over
//! the library. Exit codes are part of the product contract (CI scripts key
//! off them): 0 = clean, 1 = the data or the diff violates the contract,
//! 2 = the run itself failed (bad flags, unreadable file, invalid contract).

use std::io::{BufWriter, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::compile::CompiledContract;
use crate::diff::{self, Severity};
use crate::error::CovenantError;
use crate::sources::{self, DataFormat};
use crate::spec::{Contract, LintLevel};

/// Exit code: data conforms / diff acceptable / contract lints clean.
pub const EXIT_CLEAN: i32 = 0;
/// Exit code: the checked subject violates its contract.
pub const EXIT_VIOLATED: i32 = 1;
/// Exit code: the run itself failed (usage, IO, unusable contract).
pub const EXIT_ERROR: i32 = 2;

#[derive(Parser)]
#[command(
    name = "covenant",
    version,
    about = "Data-contract enforcement runtime: validate, gate, and diff contracts at the producer boundary",
    long_about = "Covenant runs data-contract assertions where the data actually flows.\n\
                  `check` gates files in CI, `gate` filters NDJSON streams inline \n\
                  (pipe it through kcat for Kafka), `diff` blocks breaking contract \n\
                  changes pre-merge (naming who breaks, given --consumers), \n\
                  `consumer-check` verifies consumer manifests against the contract, \n\
                  and `validate` lints the contract itself."
)]
/// Top-level CLI: `covenant <subcommand>`.
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
/// The enforcement subcommands (doc comments double as `--help` text).
pub enum Command {
    /// Lint a contract document (exit 1 on error-level findings)
    Validate {
        /// Contract file (.yaml/.yml/.json)
        contract: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Check data files against a contract (the CI gate)
    Check {
        /// Data files (.ndjson/.jsonl/.json, .csv, .parquet)
        #[arg(required = true)]
        data: Vec<PathBuf>,
        /// Contract file
        #[arg(short, long)]
        contract: PathBuf,
        /// Model to enforce (defaults to the contract's only model)
        #[arg(short, long)]
        model: Option<String>,
        /// Override the data format instead of inferring from the extension
        #[arg(long, value_enum)]
        data_format: Option<DataFormatArg>,
        /// Override the contract's violation budget for this run (applies to
        /// all files combined, not per file)
        #[arg(long)]
        max_violations: Option<u64>,
        /// Check as this consumer manifest: only its declared fields are
        /// validated, strict mode is off (consumer-side CI gate)
        #[arg(long, value_name = "MANIFEST")]
        as_consumer: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Verify consumer manifests against a contract (the consumer-side gate:
    /// declared fields still exist, version pins aren't left behind)
    ConsumerCheck {
        /// Manifest files or directories of manifests (recursive)
        #[arg(required = true)]
        manifests: Vec<PathBuf>,
        /// Contract file the manifests are verified against
        #[arg(short, long)]
        contract: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Diff two contract versions and classify the changes (the merge gate)
    Diff {
        /// The contract consumers rely on today
        old: PathBuf,
        /// The proposed contract
        new: PathBuf,
        /// Directory of consumer manifests (recursive) — names exactly who
        /// each change breaks, in the report and the exit code
        #[arg(long, value_name = "DIR")]
        consumers: Option<PathBuf>,
        /// Severity that fails the diff (exit 1)
        #[arg(long, value_enum, default_value_t = FailOn::Breaking)]
        fail_on: FailOn,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Enforce a contract on an NDJSON stream: stdin -> clean stdout, violations -> DLQ
    Gate {
        /// Contract file
        #[arg(short, long)]
        contract: PathBuf,
        /// Model to enforce (defaults to the contract's only model)
        #[arg(short, long)]
        model: Option<String>,
        /// Write dead-letter envelopes to this file instead of stderr
        /// (appended, so reruns never destroy earlier dead letters)
        #[arg(long)]
        dlq: Option<PathBuf>,
        /// Periodically write a machine-readable stats snapshot here
        /// (atomic tmp+rename) — `covenant serve --gate-stats <path>` reads
        /// it for GET /v1/gate/stats
        #[arg(long, value_name = "PATH")]
        stats: Option<PathBuf>,
        /// Skip `unique` constraints (their memory grows with stream cardinality)
        #[arg(long)]
        no_unique: bool,
        /// Suppress the end-of-stream stats summary on stderr
        #[arg(short, long)]
        quiet: bool,
    },
    /// Draft a contract from a sample of real data (the on-ramp)
    Infer {
        /// Data files (.ndjson/.jsonl/.json, .csv, .parquet)
        #[arg(required = true)]
        data: Vec<PathBuf>,
        /// Contract id (defaults to the first file's name)
        #[arg(long)]
        id: Option<String>,
        /// Model name (defaults to the contract id)
        #[arg(long)]
        model: Option<String>,
        /// Records to sample; 0 reads everything
        #[arg(long, default_value_t = crate::infer::DEFAULT_SAMPLE)]
        sample: u64,
        /// Distinct values at or below which a string column becomes
        /// `allowed:`; 0 never drafts enums
        #[arg(long, default_value_t = crate::infer::DEFAULT_MAX_ENUM)]
        max_enum: usize,
        /// Do not draft observed numeric ranges / lengths as rules
        #[arg(long)]
        no_ranges: bool,
        /// Write the draft here instead of stdout (will not overwrite)
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// yaml = the editable draft; json = contract + notes as data
        #[arg(long, value_enum, default_value_t = DraftFormat::Yaml)]
        format: DraftFormat,
    },
    /// Write a starter contract to get going
    Init {
        /// Destination (will not overwrite)
        #[arg(default_value = "covenant.yaml")]
        path: PathBuf,
    },
    /// Serve the console + JSON API for one contract (build with --features serve)
    #[cfg(feature = "serve")]
    Serve {
        /// Contract file
        #[arg(short, long)]
        contract: PathBuf,
        /// Model to serve (defaults to the contract's only model)
        #[arg(short, long)]
        model: Option<String>,
        /// Listen address
        #[arg(long, default_value = "127.0.0.1:8787")]
        addr: String,
        /// DLQ file to read for GET /v1/dlq (the file `covenant gate --dlq`
        /// appends to) — lights the console's Dead-letters screen
        #[arg(long, value_name = "PATH")]
        dlq: Option<PathBuf>,
        /// Gate stats snapshot to read for GET /v1/gate/stats (the file
        /// `covenant gate --stats` writes) — lights the Stream-gate screen
        #[arg(long, value_name = "PATH")]
        gate_stats: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
/// Report rendering: human text or machine JSON.
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
/// How `covenant infer` renders its draft.
pub enum DraftFormat {
    /// The commented, editable contract document.
    Yaml,
    /// Contract plus the uncertainty notes, as data.
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
/// CLI-facing data-format override (maps onto [`DataFormat`]).
pub enum DataFormatArg {
    Ndjson,
    Csv,
    Parquet,
}

impl From<DataFormatArg> for DataFormat {
    fn from(v: DataFormatArg) -> DataFormat {
        match v {
            DataFormatArg::Ndjson => DataFormat::Ndjson,
            DataFormatArg::Csv => DataFormat::Csv,
            DataFormatArg::Parquet => DataFormat::Parquet,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
/// Diff severity threshold that flips the exit code to 1.
pub enum FailOn {
    Breaking,
    /// Fail only when a breaking change hits a *declared* consumer
    /// (requires --consumers) — the adoption-friendly merge gate: block
    /// when someone actually breaks, report otherwise.
    BreakingWithConsumers,
    Risky,
    Any,
    Never,
}

/// Run the parsed CLI; returns the process exit code.
pub fn run(cli: Cli) -> i32 {
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("covenant: error: {e}");
            EXIT_ERROR
        }
    }
}

fn dispatch(cli: Cli) -> crate::error::Result<i32> {
    match cli.command {
        Command::Validate { contract, format } => {
            let doc = Contract::from_path(&contract)?;
            let findings = doc.lint();
            let has_errors = findings.iter().any(|f| f.level == LintLevel::Error);
            match format {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&findings).expect("lint findings serialize")
                    );
                }
                OutputFormat::Human => {
                    if findings.is_empty() {
                        println!("OK  {} v{} — no findings", doc.id, doc.version);
                    } else {
                        for f in &findings {
                            let level = match f.level {
                                LintLevel::Error => "error",
                                LintLevel::Warning => "warning",
                            };
                            println!("{level:<8} {}: {}", f.path, f.message);
                        }
                        println!(
                            "{} v{}: {} finding{}",
                            doc.id,
                            doc.version,
                            findings.len(),
                            if findings.len() == 1 { "" } else { "s" }
                        );
                    }
                }
            }
            Ok(if has_errors { EXIT_VIOLATED } else { EXIT_CLEAN })
        }

        Command::Check {
            data,
            contract,
            model,
            data_format,
            max_violations,
            as_consumer,
            format,
        } => {
            let doc = Contract::from_path(&contract)?;
            // --as-consumer narrows the contract to what one manifest
            // declares before compiling; the model is resolved by name
            // against the FULL contract (no compile needed just for that —
            // the single post-scope compile below is the lint gate).
            let mut scoped_consumer: Option<String> = None;
            let doc = match &as_consumer {
                Some(path) => {
                    let manifest = crate::consumers::ConsumerManifest::from_path(path)?;
                    let model_name = doc.resolve_model_name(model.as_deref())?.to_string();
                    let scoped = crate::consumers::scope_contract_to_consumer(
                        &doc,
                        &model_name,
                        &manifest,
                        &path.display().to_string(),
                    )?;
                    if format == OutputFormat::Human {
                        let kept = scoped.models[&model_name].fields.len();
                        let total = doc.models[&model_name].fields.len();
                        println!(
                            "checking as consumer {} — {kept} of {total} fields of model {model_name}, strict off",
                            manifest.id,
                        );
                    }
                    scoped_consumer = Some(manifest.id);
                    scoped
                }
                None => doc,
            };
            let compiled = CompiledContract::compile(&doc)?;
            let target = compiled.resolve_model(model.as_deref())?;
            let budget = max_violations.unwrap_or(compiled.policy.max_violations);
            let warn_only = compiled.policy.on_violation == crate::spec::OnViolation::Warn;

            let mut reports = Vec::with_capacity(data.len());
            for path in &data {
                let mut report =
                    sources::check_path(&compiled, target, path, data_format.map(Into::into))?;
                // Mark scoped runs in the report itself: a JSON artifact
                // saying "conforms to orders v1.2.0" must not be mistaken
                // for a full-contract verdict when only one consumer's
                // fields were validated with strict off.
                report.as_consumer = scoped_consumer.clone();
                reports.push(report);
            }
            // The budget is a per-run allowance (spec: violations tolerated
            // "before a check run fails"), so the verdict compares the sum —
            // three files with 5 violations each is 15 against the budget,
            // not three independent 5s.
            let total_violations: u64 = reports.iter().map(|r| r.violations).sum();
            let any_failed = total_violations > budget;
            match format {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&reports).expect("reports serialize")
                    );
                }
                OutputFormat::Human => {
                    for r in &reports {
                        print!("{}", r.render_human(budget));
                    }
                    if reports.len() > 1 {
                        println!(
                            "run total: {total_violations} violation{} across {} files{}",
                            if total_violations == 1 { "" } else { "s" },
                            reports.len(),
                            if budget > 0 { format!(" (budget {budget})") } else { String::new() },
                        );
                    }
                    if warn_only && any_failed {
                        println!("policy is on_violation: warn — reporting only, not failing");
                    }
                }
            }
            Ok(if any_failed && !warn_only {
                EXIT_VIOLATED
            } else {
                EXIT_CLEAN
            })
        }

        Command::ConsumerCheck {
            manifests,
            contract,
            format,
        } => {
            let doc = Contract::from_path(&contract)?;
            // Same refusal as every other enforcement path: a contract that
            // fails its own lint must exit 2 here, not produce FAIL verdicts
            // that blame the consumers (e.g. a non-semver contract version
            // would fail every pinned manifest).
            CompiledContract::compile(&doc)?;
            // Expand files and directories into (path, manifest) pairs.
            let mut entries: Vec<(PathBuf, crate::consumers::ConsumerManifest)> = Vec::new();
            for path in &manifests {
                if path.is_dir() {
                    entries.extend(crate::consumers::load_dir_entries(path)?);
                } else {
                    entries.push((path.clone(), crate::consumers::ConsumerManifest::from_path(path)?));
                }
            }
            let consuming = entries.iter().filter(|(_, m)| m.consumes_contract(&doc.id)).count();
            if consuming == 0 {
                return Err(CovenantError::ManifestInvalid {
                    path: manifests
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    message: format!(
                        "none of the {} manifest{} consume contract {:?} — nothing to verify",
                        entries.len(),
                        if entries.len() == 1 { "" } else { "s" },
                        doc.id,
                    ),
                });
            }

            let results: Vec<crate::consumers::VerifyResult> = entries
                .iter()
                .map(|(path, m)| crate::consumers::VerifyResult {
                    consumer: m.id.clone(),
                    source: Some(path.display().to_string()),
                    consumes_contract: m.consumes_contract(&doc.id),
                    findings: m.verify_against(&doc),
                })
                .collect();
            let failed = results.iter().any(|r| {
                r.findings.iter().any(|f| f.level == LintLevel::Error)
            });
            // The one shared envelope (also what /v1/consumers/verify
            // serves), so one script works against either surface.
            let report = crate::consumers::ConsumerCheckReport {
                contract: doc.id.clone(),
                version: doc.version.clone(),
                results,
            };

            match format {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).expect("verify report serializes")
                    );
                }
                OutputFormat::Human => {
                    println!("consumer check against {} v{}", report.contract, report.version);
                    let (mut ok, mut fail, mut warned, mut skipped) = (0u32, 0u32, 0u32, 0u32);
                    for r in &report.results {
                        let source = r.source.as_deref().unwrap_or("<inline>");
                        if !r.consumes_contract {
                            skipped += 1;
                            println!("  skip  {} ({}) — does not consume {}", r.consumer, source, report.contract);
                            continue;
                        }
                        let errors = r.findings.iter().filter(|f| f.level == LintLevel::Error).count();
                        if errors > 0 {
                            fail += 1;
                        } else if r.findings.is_empty() {
                            ok += 1;
                            println!("  OK    {} ({source})", r.consumer);
                        } else {
                            warned += 1;
                        }
                        for f in &r.findings {
                            let level = match f.level {
                                LintLevel::Error => "FAIL",
                                LintLevel::Warning => "warn",
                            };
                            println!("  {level}  {} — {}: {}", r.consumer, f.path, f.message);
                        }
                    }
                    println!("{ok} ok · {fail} failed · {warned} warned · {skipped} skipped");
                }
            }
            Ok(if failed { EXIT_VIOLATED } else { EXIT_CLEAN })
        }

        Command::Diff {
            old,
            new,
            consumers,
            fail_on,
            format,
        } => {
            // Checked before any file work: this mode without manifests
            // would never fail — the exact silent-pass hole it exists to close.
            if fail_on == FailOn::BreakingWithConsumers && consumers.is_none() {
                return Err(CovenantError::Usage {
                    message: "--fail-on breaking-with-consumers needs --consumers <dir> \
                              (without manifests there is nothing to match)"
                        .into(),
                });
            }
            let old_doc = Contract::from_path(&old)?;
            let new_doc = Contract::from_path(&new)?;
            let mut report = diff::diff(&old_doc, &new_doc);
            if let Some(dir) = &consumers {
                let manifests = crate::consumers::load_dir(dir)?;
                crate::consumers::annotate(&mut report, &old_doc, &new_doc, &manifests);
            }
            match format {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).expect("diff serializes")
                    );
                }
                OutputFormat::Human => print!("{}", report.render_human()),
            }
            let max = report.max_severity();
            let failed = match fail_on {
                FailOn::Never => false,
                FailOn::Any => max.is_some(),
                FailOn::Risky => max >= Some(Severity::Risky),
                FailOn::Breaking => max >= Some(Severity::Breaking),
                FailOn::BreakingWithConsumers => report
                    .consumer_impact
                    .as_ref()
                    .is_some_and(|ci| {
                        ci.impacted.iter().any(|c| c.severity >= Severity::Breaking)
                    }),
            };
            Ok(if failed { EXIT_VIOLATED } else { EXIT_CLEAN })
        }

        Command::Gate {
            contract,
            model,
            dlq,
            stats,
            no_unique,
            quiet,
        } => {
            let doc = Contract::from_path(&contract)?;
            let compiled = CompiledContract::compile(&doc)?;
            let target = compiled.resolve_model(model.as_deref())?;

            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            let mut out = BufWriter::new(stdout.lock());
            let mut sink = stats.map(|path| crate::gate::StatsSink::new(path, &compiled, target));

            let outcome = match &dlq {
                Some(path) => {
                    // Append, never truncate: the DLQ is the ONLY copy of
                    // blocked records under policy `block`, and a rerun that
                    // wiped the previous run's dead letters would destroy
                    // exactly the data the DLQ exists to preserve for replay.
                    let file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(|e| CovenantError::Io {
                            path: path.display().to_string(),
                            source: e,
                        })?;
                    let mut dlq_out = BufWriter::new(file);
                    crate::gate::run_with_stats(
                        &compiled,
                        target,
                        stdin.lock(),
                        &mut out,
                        &mut dlq_out,
                        !no_unique,
                        sink.as_mut(),
                    )?
                }
                None => {
                    let stderr = std::io::stderr();
                    let mut dlq_out = BufWriter::new(stderr.lock());
                    crate::gate::run_with_stats(
                        &compiled,
                        target,
                        stdin.lock(),
                        &mut out,
                        &mut dlq_out,
                        !no_unique,
                        sink.as_mut(),
                    )?
                }
            };
            drop(out);

            if !quiet {
                let mut stderr = std::io::stderr().lock();
                let _ = writeln!(
                    stderr,
                    "covenant gate [{} v{}, model {}]: {} records, {} passed, {} blocked, {} warned",
                    compiled.id,
                    compiled.version,
                    target.name,
                    outcome.stats.records,
                    outcome.stats.passed,
                    outcome.stats.blocked,
                    outcome.stats.warned,
                );
            }
            Ok(if outcome.failed { EXIT_VIOLATED } else { EXIT_CLEAN })
        }

        #[cfg(feature = "serve")]
        Command::Serve {
            contract,
            model,
            addr,
            dlq,
            gate_stats,
        } => {
            let doc = Contract::from_path(&contract)?;
            let compiled = CompiledContract::compile(&doc)?;
            let model_name = compiled.resolve_model(model.as_deref())?.name.clone();
            crate::serve::run(
                &addr,
                crate::serve::AppState {
                    source: contract.display().to_string(),
                    doc,
                    compiled,
                    model: model_name,
                    dlq_path: dlq,
                    gate_stats_path: gate_stats,
                },
            )?;
            Ok(EXIT_CLEAN)
        }

        Command::Infer {
            data,
            id,
            model,
            sample,
            max_enum,
            no_ranges,
            out,
            format,
        } => {
            let opts = crate::infer::InferOptions {
                id,
                model,
                sample,
                max_enum,
                ranges: !no_ranges,
            };
            let draft = crate::infer::infer_paths(&data, &opts)?;
            let rendered = match format {
                DraftFormat::Yaml => draft.to_yaml(),
                DraftFormat::Json => {
                    serde_json::to_string_pretty(&draft.to_json()).expect("draft serializes")
                }
            };
            match &out {
                Some(path) => {
                    write_new(path, &rendered)?;
                    eprintln!(
                        "covenant: drafted {} from {} sampled record(s) -> {}",
                        draft.contract.id,
                        draft.sampled,
                        path.display(),
                    );
                    eprintln!("next: review the `confirm:` notes, then `covenant validate {}`", path.display());
                }
                None => print!("{rendered}"),
            }
            // A draft is never a verdict: nothing was checked, so nothing
            // can have failed.
            Ok(EXIT_CLEAN)
        }

        Command::Init { path } => {
            write_new(&path, STARTER_CONTRACT)?;
            println!("wrote starter contract to {}", path.display());
            println!("next: edit it, then `covenant validate {}`", path.display());
            Ok(EXIT_CLEAN)
        }
    }
}

/// Write a file that must not already exist. `create_new` is atomic:
/// exists-then-write would truncate a file another process created in the
/// gap between the two syscalls.
fn write_new(path: &std::path::Path, content: &str) -> crate::error::Result<()> {
    let io_err = |e: std::io::Error| CovenantError::Io {
        path: path.display().to_string(),
        source: if e.kind() == std::io::ErrorKind::AlreadyExists {
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "refusing to overwrite an existing file",
            )
        } else {
            e
        },
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_err)?;
    file.write_all(content.as_bytes()).map_err(io_err)?;
    file.flush().map_err(io_err)
}

const STARTER_CONTRACT: &str = r#"# Covenant data contract — see `covenant validate` for linting.
covenant: 1
id: orders
name: Orders stream
version: 1.0.0
owner: data-platform@example.com
description: One record per confirmed order.

models:
  orders:
    strict: true          # undeclared fields are violations
    fields:
      order_id:
        type: string
        required: true
        unique: true
        pattern: "^ord_[a-z0-9]{12}$"
      amount_cents:
        type: integer
        required: true
        min: 0
      currency:
        type: string
        required: true
        allowed: [USD, EUR, GBP]
      customer_email:
        type: string
        format: email
        nullable: true
      created_at:
        type: timestamp
        required: true

policy:
  on_violation: block     # block | warn
  max_violations: 0       # violations tolerated before a check fails
  sample_violations: 10   # examples kept per (field, rule) in reports
"#;
