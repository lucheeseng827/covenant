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
use crate::protocol::{Outcome, Producer, ReportDocument, RunClock, RunContext, SampleMode};
use crate::send::Destination;
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
                  and `validate` lints the contract itself. `check --profile` also \n\
                  records what the data looked like, `drift` compares two such profiles, \n\
                  `export postgres` compiles the contract into a table's own constraints, \n\
                  and `mcp` serves the tools to AI agents."
)]
/// Top-level CLI: `covenant <subcommand>`.
pub struct Cli {
    /// Run an ODCS contract even though some of its rules cannot be enforced
    /// yet: they are skipped and named on stderr, and `check`, `diff` and
    /// `serve` reports list them and read as partial. Without it such a
    /// contract is refused (exit 2).
    #[arg(long, global = true)]
    pub allow_unenforced: bool,
    #[command(subcommand)]
    pub command: Command,
}

/// `covenant gate`'s Kafka flags, as given.
// Read only by the Kafka gate; a build without it refuses them unread.
#[cfg_attr(not(feature = "kafka"), allow(dead_code))]
struct KafkaFlags {
    brokers: String,
    from: String,
    to: String,
    dlq_topic: Option<String>,
    group: Option<String>,
    exit_at_end: bool,
    commit_interval_ms: u64,
    properties: Vec<String>,
    config: Option<PathBuf>,
}

/// `covenant gate --brokers …`: the gate over Kafka topics, until SIGINT or
/// SIGTERM (a second one stops at once, without settling) or, with
/// `--exit-at-end`, until every assigned partition is read to its end.
/// Returns the run's outcome and the tombstones it passed on unjudged.
#[cfg(feature = "kafka")]
#[allow(clippy::too_many_arguments)]
fn gate_kafka(
    compiled: &CompiledContract,
    target: &crate::compile::CompiledModel,
    flags: KafkaFlags,
    dlq: Option<PathBuf>,
    stats: Option<PathBuf>,
    track_unique: bool,
    quiet: bool,
    report: Option<&mut crate::report::Collector>,
) -> crate::error::Result<(crate::gate::GateOutcome, u64)> {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use crate::kafka::{parse_properties, Topics};

    let mut properties = Vec::new();
    if let Some(path) = &flags.config {
        let text = std::fs::read_to_string(path).map_err(|e| CovenantError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        properties.extend(parse_properties(&text, &path.display().to_string())?);
    }
    for property in &flags.properties {
        properties.extend(parse_properties(property, "-X")?);
    }
    let group = flags
        .group
        .unwrap_or_else(|| Topics::default_group(&compiled.id, &flags.from));
    let topics = Topics {
        brokers: flags.brokers,
        from: flags.from,
        to: flags.to,
        dlq: flags.dlq_topic,
        group,
        exit_at_end: flags.exit_at_end,
        commit_interval: std::time::Duration::from_millis(flags.commit_interval_ms),
        properties,
    };
    let dead_letters: Box<dyn Write + Send> = match &dlq {
        Some(path) => Box::new(BufWriter::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| CovenantError::Io {
                    path: path.display().to_string(),
                    source: e,
                })?,
        )),
        None => Box::new(std::io::stderr()),
    };

    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        // The first signal settles and stops; a second one exits at once.
        signal_hook::flag::register_conditional_shutdown(signal, EXIT_ERROR, Arc::clone(&stop))
            .and_then(|_| signal_hook::flag::register(signal, Arc::clone(&stop)))
            .map_err(|e| CovenantError::Io {
                path: "<signal handler>".to_string(),
                source: e,
            })?;
    }
    if !quiet {
        eprintln!(
            "covenant gate [{} v{}, model {}]: {} -> {}, dead letters to {}, group {}",
            compiled.id,
            compiled.version,
            target.name,
            topics.from,
            topics.to,
            topics.dlq.clone().unwrap_or_else(|| match &dlq {
                Some(path) => path.display().to_string(),
                None => "stderr".to_string(),
            }),
            topics.group,
        );
    }
    let mut sink = stats.map(|path| crate::gate::StatsSink::new(path, compiled, target));
    let outcome = crate::kafka::run(
        compiled,
        target,
        &topics,
        dead_letters,
        track_unique,
        sink.as_mut(),
        report,
        &stop,
    )?;
    if !quiet {
        let tombstones = match outcome.tombstones {
            0 => String::new(),
            1 => ", 1 tombstone passed on unjudged".to_string(),
            n => format!(", {n} tombstones passed on unjudged"),
        };
        eprintln!(
            "covenant gate [{} v{}, model {}]: {} records, {} passed, {} blocked, {} warned{tombstones}",
            compiled.id,
            compiled.version,
            target.name,
            outcome.stats.records,
            outcome.stats.passed,
            outcome.stats.blocked,
            outcome.stats.warned,
        );
    }
    let gate = crate::gate::GateOutcome {
        stats: outcome.stats,
        failed: outcome.failed,
    };
    Ok((gate, outcome.tombstones))
}

/// Without the `kafka` feature there is no Kafka client to run.
#[cfg(not(feature = "kafka"))]
#[allow(clippy::too_many_arguments)]
fn gate_kafka(
    _: &CompiledContract,
    _: &crate::compile::CompiledModel,
    _: KafkaFlags,
    _: Option<PathBuf>,
    _: Option<PathBuf>,
    _: bool,
    _: bool,
    _: Option<&mut crate::report::Collector>,
) -> crate::error::Result<(crate::gate::GateOutcome, u64)> {
    Err(CovenantError::Usage {
        message: "--brokers needs covenant built with the kafka feature \
                  (cargo install covenant-data --features kafka)"
            .to_string(),
    })
}

/// Where a run says what it did with its document: stdout for a command's
/// human output, stderr for the gate (whose stdout is the stream), or
/// nowhere (JSON output, `--quiet`, `--subprocess`).
#[derive(Clone, Copy)]
enum Say {
    Stdout,
    Stderr,
    Quiet,
}

impl Say {
    fn human(format: OutputFormat) -> Say {
        match format {
            OutputFormat::Human => Say::Stdout,
            OutputFormat::Json => Say::Quiet,
        }
    }

    fn line(self, text: &str) {
        match self {
            Say::Stdout => println!("{text}"),
            Say::Stderr => {
                let _ = writeln!(std::io::stderr().lock(), "{text}");
            }
            Say::Quiet => {}
        }
    }
}

/// Deliver a run's document as it was asked for: written to `path`, sent to
/// `to`. A document that cannot be written fails the run; one that cannot be
/// sent is one warning on stderr, and the run's exit code stands.
fn deliver(
    doc: &ReportDocument,
    path: Option<&std::path::Path>,
    to: Option<&Destination>,
    say: Say,
) -> crate::error::Result<()> {
    if let Some(path) = path {
        doc.write(path)?;
        say.line(&format!("report: covenant-report/v1 → {}", path.display()));
    }
    if let Some(to) = to {
        // The same bytes `write` wrote: the document serializes one way.
        match crate::send::post(to, &doc.to_json()) {
            Ok(()) => say.line(&format!("report: covenant-report/v1 sent → {}", to.url())),
            Err(why) => eprintln!("covenant: warning: report not sent to {}: {why}", to.url()),
        }
    }
    Ok(())
}

/// Send a run's OpenLineage events, START then COMPLETE, with the key from
/// OPENLINEAGE_API_KEY. A failure is one warning, and the exit code stands.
fn send_lineage(
    doc: &ReportDocument,
    to: &Destination,
    model: &crate::compile::CompiledModel,
    warn_only: bool,
    say: Say,
) {
    let declared = crate::lineage::declared_rules(model);
    let events = match crate::lineage::run_events(doc, &declared, warn_only) {
        Ok(events) => events,
        Err(e) => {
            eprintln!("covenant: warning: OpenLineage events not sent: {e}");
            return;
        }
    };
    for event in &events {
        let body = serde_json::to_vec(event).expect("events serialize");
        if let Err(why) = crate::send::post_as(to, &body, crate::lineage::API_KEY_ENV) {
            eprintln!(
                "covenant: warning: OpenLineage events not sent to {}: {why}",
                to.url()
            );
            return;
        }
    }
    say.line(&format!(
        "openlineage: START and COMPLETE sent → {}",
        to.url()
    ));
}

/// A gated topic as the report names it, the way OpenLineage names a Kafka
/// dataset: `kafka://<first bootstrap server>/<topic>`.
fn kafka_source(brokers: &str, topic: &str) -> String {
    let first = brokers.split(',').next().unwrap_or_default().trim();
    // A listener name (`SASL_SSL://host:port`) is not part of the address.
    let address = first.rsplit("://").next().unwrap_or(first);
    format!("kafka://{address}/{topic}")
}

/// Load a contract for enforcement. Rules the runtime cannot enforce refuse
/// the run unless it allows them — and then the run says so on stderr,
/// every time, and returns the list for the report.
fn load_for_enforcement(
    path: &std::path::Path,
    allow_unenforced: bool,
) -> crate::error::Result<(Contract, Vec<crate::spec::UnenforcedRule>)> {
    let loaded = Contract::load_path(path)?;
    if !allow_unenforced || loaded.unenforced.is_empty() {
        let contract = loaded.into_enforceable(&path.display().to_string())?;
        return Ok((contract, Vec::new()));
    }
    eprintln!(
        "covenant: warning: {}: {} contract rule(s) not enforced (--allow-unenforced); \
         the verdict covers the rest",
        path.display(),
        loaded.unenforced.len()
    );
    Ok((loaded.contract, loaded.unenforced))
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
        /// Also write a profile of the checked data here: per-field sketches
        /// collected in the same read, which `profile merge` combines and
        /// `drift` compares
        #[arg(long, value_name = "PATH")]
        profile: Option<PathBuf>,
        /// Also write the run's verdict here as a `covenant-report/v1` document:
        /// the counts, the run's context and samples that never carry a value,
        /// for CI artifacts, catalogs and services
        #[arg(long, value_name = "PATH")]
        report_json: Option<PathBuf>,
        /// What the report's samples say: `masked` gives each one's field, rule
        /// and row and the value's type and length, never the value; `hashed`
        /// adds a keyed hash of the value (the key from COVENANT_SAMPLE_KEY);
        /// `none` keeps the counts only
        #[arg(long, value_enum, default_value_t = ReportSamples::Masked)]
        report_samples: ReportSamples,
        /// Also send the document here: an https URL (plain http only to
        /// this machine), POSTed as JSON with the token from COVENANT_TOKEN.
        /// A document that cannot be sent is one warning; the exit code stands
        #[arg(long, value_name = "URL")]
        report_to: Option<String>,
        /// Also send the run to an OpenLineage endpoint (e.g. Marquez's
        /// /api/v1/lineage): START and COMPLETE events whose input datasets
        /// carry each rule's result (dataQualityAssertions) and the rows read.
        /// The key from OPENLINEAGE_API_KEY; a failure is one warning
        #[arg(long, value_name = "URL")]
        openlineage: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Merge or summarize profiles written by `check --profile`
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Compare two profiles of one contract model and report what drifted in
    /// the data (exit 1 on drift)
    Drift {
        /// The profile to compare against, e.g. `profile merge` of recent runs
        baseline: PathBuf,
        /// The profile of the data now
        current: PathBuf,
        /// Largest tolerated change in a null, missing or invalid rate, in
        /// points (0.05 = 5 points)
        #[arg(long, default_value_t = 0.05)]
        rate: f64,
        /// Largest tolerated population stability index of a numeric field
        #[arg(long, default_value_t = 0.25)]
        psi: f64,
        /// Largest tolerated Jensen–Shannon distance (0–1) between the value
        /// shares of a boolean or `allowed:` field
        #[arg(long, default_value_t = 0.1)]
        js: f64,
        /// Largest tolerated change in the distinct share (points), or in the
        /// distinct count for fields with under 1,000 values (relative)
        #[arg(long, default_value_t = 0.1)]
        distinct: f64,
        /// Largest tolerated relative change in rows per run
        #[arg(long, default_value_t = 0.5)]
        volume: f64,
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
        /// Also write the verdict here as a `covenant-report/v1` document: the
        /// classified changes and who they break, and the run's context
        #[arg(long, value_name = "PATH")]
        report_json: Option<PathBuf>,
        /// Also send the document here: an https URL (plain http only to
        /// this machine), POSTed as JSON with the token from COVENANT_TOKEN.
        /// A document that cannot be sent is one warning; the exit code stands
        #[arg(long, value_name = "URL")]
        report_to: Option<String>,
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
        /// Answer every line with one line, flushed at once: the record on
        /// stdout if it goes on, its dead-letter envelope on stderr if it is
        /// withheld, a blank line for a blank one. For stream processors that
        /// run the gate as a subprocess (Redpanda Connect's `subprocess`:
        /// stdout replaces the message, stderr marks it failed). Dead letters
        /// go to --dlq as well when it is given; no summary is printed
        #[arg(long, conflicts_with = "stats")]
        subprocess: bool,
        /// When the stream ends, write its verdict here as a
        /// `covenant-report/v1` document: the counts per field and rule, what
        /// became of the records, and samples that never carry a value. The
        /// directory must exist before the stream is read
        #[arg(long, value_name = "PATH")]
        report_json: Option<PathBuf>,
        /// What the report's samples say: `masked` gives each one's field, rule
        /// and row (the dead letter's row) and the value's type and length,
        /// never the value; `hashed` adds a keyed hash of the value (the key
        /// from COVENANT_SAMPLE_KEY); `none` keeps the counts only
        #[arg(long, value_enum, default_value_t = ReportSamples::Masked)]
        report_samples: ReportSamples,
        /// When the stream ends, also send its document here: an https URL
        /// (plain http only to this machine), POSTed as JSON with the token
        /// from COVENANT_TOKEN. A document that cannot be sent is one warning;
        /// the exit code stands. Not with --subprocess, whose stderr is taken
        #[arg(long, value_name = "URL", conflicts_with = "subprocess")]
        report_to: Option<String>,
        /// When the run ends, also send it to an OpenLineage endpoint: the
        /// topic as an input dataset with each rule's result and the rows
        /// read. The Kafka gate only: a stream on stdin names no dataset
        #[arg(long, value_name = "URL", conflicts_with = "subprocess")]
        openlineage: Option<String>,
        /// Gate Kafka topics instead of stdin: the brokers to bootstrap
        /// from (host:port, comma-separated). Needs covenant built with the
        /// `kafka` feature
        #[arg(
            long,
            value_name = "HOST:PORT",
            requires_all = ["from", "to"],
            conflicts_with = "subprocess",
            help_heading = "Kafka",
            hide = cfg!(not(feature = "kafka"))
        )]
        brokers: Option<String>,
        /// The topic to gate
        #[arg(long, value_name = "TOPIC", requires = "brokers", help_heading = "Kafka", hide = cfg!(not(feature = "kafka")))]
        from: Option<String>,
        /// The topic the records that go on are produced to, with their key,
        /// headers and timestamp
        #[arg(long, value_name = "TOPIC", requires = "brokers", help_heading = "Kafka", hide = cfg!(not(feature = "kafka")))]
        to: Option<String>,
        /// Produce dead letters to this topic (the record's key and headers,
        /// plus covenant.source.topic, .partition and .offset headers)
        /// instead of to --dlq or stderr
        #[arg(
            long,
            value_name = "TOPIC",
            requires = "brokers",
            conflicts_with = "dlq",
            help_heading = "Kafka",
            hide = cfg!(not(feature = "kafka"))
        )]
        dlq_topic: Option<String>,
        /// The consumer group the gate commits under
        /// [default: covenant-gate.<contract id>.<from>]
        #[arg(long, value_name = "ID", requires = "brokers", help_heading = "Kafka", hide = cfg!(not(feature = "kafka")))]
        group: Option<String>,
        /// Stop once every assigned partition is read to its end (a backfill,
        /// a test), instead of running until SIGINT or SIGTERM
        #[arg(long, requires = "brokers", help_heading = "Kafka", hide = cfg!(not(feature = "kafka")))]
        exit_at_end: bool,
        /// How often to wait for the brokers' acknowledgements and commit
        /// (a rebalance and the end of a run always do)
        #[arg(
            long,
            value_name = "MS",
            default_value_t = 1000,
            value_parser = clap::value_parser!(u64).range(1..),
            requires = "brokers",
            help_heading = "Kafka",
            hide = cfg!(not(feature = "kafka"))
        )]
        commit_interval_ms: u64,
        /// A librdkafka property for both clients, e.g. -X
        /// security.protocol=SASL_SSL (repeatable; applied after
        /// --kafka-config)
        #[arg(
            short = 'X',
            value_name = "KEY=VALUE",
            requires = "brokers",
            help_heading = "Kafka",
            hide = cfg!(not(feature = "kafka"))
        )]
        kafka_property: Vec<String>,
        /// A file of librdkafka properties, one key=value per line (keeps
        /// credentials off the command line)
        #[arg(long, value_name = "FILE", requires = "brokers", help_heading = "Kafka", hide = cfg!(not(feature = "kafka")))]
        kafka_config: Option<PathBuf>,
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
    /// Serve validate, check, diff, explain and drift as Model Context
    /// Protocol tools over stdio, for AI agents
    Mcp,
    /// Run ODCS conformance vectors through this engine (exit 1 if one it
    /// supports comes out differently from the standard)
    Conformance {
        /// Vector files, or directories of them (searched recursively)
        #[arg(required = true)]
        vectors: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Send covenant-report/v1 documents written with --report-json: each is
    /// POSTed as JSON, with the token from COVENANT_TOKEN (exit 2 if one is
    /// not a document or is not delivered)
    Push {
        /// Report documents
        #[arg(required = true)]
        documents: Vec<PathBuf>,
        /// Where to send them: an https URL (plain http only to this machine)
        #[arg(long, value_name = "URL")]
        to: String,
    },
    /// Write a starter contract to get going
    Init {
        /// Destination (will not overwrite)
        #[arg(default_value = "covenant.yaml")]
        path: PathBuf,
    },
    /// Compile the contract into an engine's own enforcement. `postgres`
    /// prints a CREATE TABLE whose constraints reject the rows `check`
    /// reports; a rule with no exact equivalent refuses the export (exit 2)
    Export {
        /// The engine
        #[arg(value_enum)]
        dialect: Dialect,
        /// Contract file
        #[arg(short, long)]
        contract: PathBuf,
        /// Model to export (defaults to the contract's only model)
        #[arg(short, long)]
        model: Option<String>,
        /// Table name, optionally schema-qualified (defaults to the model's name)
        #[arg(long)]
        table: Option<String>,
        /// Write the SQL here instead of stdout
        #[arg(short, long)]
        out: Option<PathBuf>,
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

#[derive(Subcommand)]
/// `covenant profile …`
pub enum ProfileAction {
    /// Merge profiles of one contract model — runs, partitions, days — into
    /// one, e.g. a baseline for `drift`
    Merge {
        /// Profiles to merge (at least one)
        #[arg(required = true)]
        profiles: Vec<PathBuf>,
        /// Write the merged profile here instead of stdout
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Summarize a profile: presence, nulls, distinct counts, ranges and
    /// quantiles per field
    Show {
        profile: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
/// The engines `covenant export` compiles a contract for.
pub enum Dialect {
    /// A PostgreSQL table: column types, NOT NULL, CHECK and UNIQUE.
    Postgres,
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
/// What `--report-json` samples say (maps onto [`SampleMode`]).
pub enum ReportSamples {
    /// Each sample's field, rule and row, and the value's type and length;
    /// never the value.
    Masked,
    /// Masked, and a keyed hash of each value: the key from
    /// COVENANT_SAMPLE_KEY, never from the command line.
    Hashed,
    /// No samples: the counts only.
    None,
}

impl ReportSamples {
    /// The mode, with the key when it hashes: read before the run starts,
    /// so a missing key fails the run before any work.
    fn mode(self) -> crate::error::Result<SampleMode> {
        Ok(match self {
            ReportSamples::Masked => SampleMode::Masked,
            ReportSamples::Hashed => SampleMode::Hashed(crate::protocol::SampleKey::from_env()?),
            ReportSamples::None => SampleMode::None,
        })
    }
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

impl FailOn {
    /// The value as `--fail-on` spells it.
    fn name(self) -> &'static str {
        match self {
            FailOn::Breaking => "breaking",
            FailOn::BreakingWithConsumers => "breaking-with-consumers",
            FailOn::Risky => "risky",
            FailOn::Any => "any",
            FailOn::Never => "never",
        }
    }
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
    let allow_unenforced = cli.allow_unenforced;
    match cli.command {
        Command::Validate { contract, format } => {
            let loaded = Contract::load_path(&contract)?;
            let doc = &loaded.contract;
            let findings = loaded.lint(allow_unenforced);
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
            Ok(if has_errors {
                EXIT_VIOLATED
            } else {
                EXIT_CLEAN
            })
        }

        Command::Check {
            data,
            contract,
            model,
            data_format,
            max_violations,
            as_consumer,
            profile,
            report_json,
            report_samples,
            report_to,
            openlineage,
            format,
        } => {
            let clock = RunClock::start();
            let report_to = report_to.as_deref().map(Destination::parse).transpose()?;
            let lineage_to = openlineage.as_deref().map(Destination::parse).transpose()?;
            let reporting = report_json.is_some() || report_to.is_some() || lineage_to.is_some();
            let sample_mode = match reporting {
                true => report_samples.mode()?,
                false => SampleMode::None,
            };
            let (doc, unenforced) = load_for_enforcement(&contract, allow_unenforced)?;
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

            let mut profiler = profile
                .as_ref()
                .map(|_| crate::profile::Profiler::new(&compiled, target));
            let mut reports = Vec::with_capacity(data.len());
            for path in &data {
                let mut report = sources::check_path_profiled(
                    &compiled,
                    target,
                    path,
                    data_format.map(Into::into),
                    profiler.as_mut(),
                )?;
                // Mark scoped runs in the report itself: a JSON artifact
                // saying "conforms to orders v1.2.0" must not be mistaken
                // for a full-contract verdict when only one consumer's
                // fields were validated with strict off.
                report.as_consumer = scoped_consumer.clone();
                // Same reason: a verdict over part of the contract carries
                // the list of what it skipped.
                report.unenforced = unenforced.clone();
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
                            if budget > 0 {
                                format!(" (budget {budget})")
                            } else {
                                String::new()
                            },
                        );
                    }
                    if warn_only && any_failed {
                        println!("policy is on_violation: warn — reporting only, not failing");
                    }
                }
            }
            if let (Some(path), Some(profiler)) = (&profile, profiler) {
                let written = profiler.finish();
                written.write(path)?;
                if format == OutputFormat::Human {
                    println!(
                        "profile: {} field{} over {} row{} → {}",
                        written.fields.len(),
                        if written.fields.len() == 1 { "" } else { "s" },
                        written.rows,
                        if written.rows == 1 { "" } else { "s" },
                        path.display()
                    );
                }
            }
            let outcome = Outcome::judge(total_violations, budget, warn_only);
            if reporting {
                let run = RunContext::capture(Producer::Command, clock.stop());
                let doc = ReportDocument::for_check(&reports, budget, outcome, &sample_mode, run);
                deliver(
                    &doc,
                    report_json.as_deref(),
                    report_to.as_ref(),
                    Say::human(format),
                )?;
                if let Some(to) = &lineage_to {
                    send_lineage(&doc, to, target, warn_only, Say::human(format));
                }
            }
            Ok(if outcome == Outcome::Fail {
                EXIT_VIOLATED
            } else {
                EXIT_CLEAN
            })
        }

        Command::Profile { action } => match action {
            ProfileAction::Merge { profiles, out } => {
                let mut merged: Option<crate::profile::Profile> = None;
                for path in &profiles {
                    let p = crate::profile::Profile::from_path(path)?;
                    match &mut merged {
                        Some(m) => m.merge(&p)?,
                        None => merged = Some(p),
                    }
                }
                let merged = merged.expect("clap requires at least one profile");
                match out {
                    Some(path) => merged.write(&path)?,
                    None => println!("{}", merged.to_json()),
                }
                Ok(EXIT_CLEAN)
            }
            ProfileAction::Show { profile, format } => {
                let summary = crate::profile::Profile::from_path(&profile)?.summary();
                match format {
                    OutputFormat::Json => println!(
                        "{}",
                        serde_json::to_string_pretty(&summary).expect("summaries serialize")
                    ),
                    OutputFormat::Human => print!("{}", summary.render_human()),
                }
                Ok(EXIT_CLEAN)
            }
        },

        Command::Conformance { vectors, format } => {
            let report =
                crate::conformance::SuiteReport::run(&crate::conformance::load_all(&vectors)?);
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("suite reports serialize")
                ),
                OutputFormat::Human => print!("{}", report.render_human()),
            }
            Ok(if report.conforms() {
                EXIT_CLEAN
            } else {
                EXIT_VIOLATED
            })
        }

        Command::Export {
            dialect,
            contract,
            model,
            table,
            out,
        } => {
            let (doc, upstream) = load_for_enforcement(&contract, allow_unenforced)?;
            let export = match dialect {
                Dialect::Postgres => crate::postgres::export(
                    &doc,
                    &crate::postgres::Options {
                        model: model.as_deref(),
                        table: table.as_deref(),
                        allow_unenforced,
                        upstream: &upstream,
                    },
                )?,
            };
            if export.unenforced.len() > upstream.len() {
                eprintln!(
                    "covenant: warning: {} contract rule(s) have no exact equivalent and are not \
                     in the table (--allow-unenforced); the SQL lists them",
                    export.unenforced.len() - upstream.len()
                );
            }
            match out {
                Some(path) => std::fs::write(&path, &export.sql).map_err(|source| {
                    crate::error::CovenantError::Io {
                        path: path.display().to_string(),
                        source,
                    }
                })?,
                None => print!("{}", export.sql),
            }
            Ok(EXIT_CLEAN)
        }

        Command::Mcp => {
            crate::mcp::serve(std::io::stdin().lock(), std::io::stdout().lock())?;
            Ok(EXIT_CLEAN)
        }

        Command::Drift {
            baseline,
            current,
            rate,
            psi,
            js,
            distinct,
            volume,
            format,
        } => {
            for (flag, v) in [
                ("rate", rate),
                ("psi", psi),
                ("js", js),
                ("distinct", distinct),
                ("volume", volume),
            ] {
                if !(v.is_finite() && v >= 0.0) {
                    return Err(CovenantError::Usage {
                        message: format!("--{flag} must be a non-negative number, got {v}"),
                    });
                }
            }
            let report = crate::drift::drift(
                &crate::profile::Profile::from_path(&baseline)?,
                &crate::profile::Profile::from_path(&current)?,
                &crate::drift::DriftThresholds {
                    rate,
                    psi,
                    js,
                    distinct,
                    volume,
                },
            )?;
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("drift reports serialize")
                ),
                OutputFormat::Human => print!("{}", report.render_human()),
            }
            Ok(if report.drifted() {
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
            let (doc, _) = load_for_enforcement(&contract, allow_unenforced)?;
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
                    entries.push((
                        path.clone(),
                        crate::consumers::ConsumerManifest::from_path(path)?,
                    ));
                }
            }
            let consuming = entries
                .iter()
                .filter(|(_, m)| m.consumes_contract(&doc.id))
                .count();
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
            let failed = results
                .iter()
                .any(|r| r.findings.iter().any(|f| f.level == LintLevel::Error));
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
                    println!(
                        "consumer check against {} v{}",
                        report.contract, report.version
                    );
                    let (mut ok, mut fail, mut warned, mut skipped) = (0u32, 0u32, 0u32, 0u32);
                    for r in &report.results {
                        let source = r.source.as_deref().unwrap_or("<inline>");
                        if !r.consumes_contract {
                            skipped += 1;
                            println!(
                                "  skip  {} ({}) — does not consume {}",
                                r.consumer, source, report.contract
                            );
                            continue;
                        }
                        let errors = r
                            .findings
                            .iter()
                            .filter(|f| f.level == LintLevel::Error)
                            .count();
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
            report_json,
            report_to,
            format,
        } => {
            let clock = RunClock::start();
            let report_to = report_to.as_deref().map(Destination::parse).transpose()?;
            // Checked before any file work: this mode without manifests
            // would never fail — the exact silent-pass hole it exists to close.
            if fail_on == FailOn::BreakingWithConsumers && consumers.is_none() {
                return Err(CovenantError::Usage {
                    message: "--fail-on breaking-with-consumers needs --consumers <dir> \
                              (without manifests there is nothing to match)"
                        .into(),
                });
            }
            // Under --allow-unenforced the diff classifies the enforceable
            // part of each contract, and the report names every rule on
            // either side that it did not compare.
            let (old_doc, mut unenforced) = load_for_enforcement(&old, allow_unenforced)?;
            let (new_doc, new_unenforced) = load_for_enforcement(&new, allow_unenforced)?;
            for r in new_unenforced {
                if !unenforced
                    .iter()
                    .any(|o| o.path == r.path && o.rule == r.rule)
                {
                    unenforced.push(r);
                }
            }
            let mut report = diff::diff(&old_doc, &new_doc);
            report.unenforced = unenforced;
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
                    .is_some_and(|ci| ci.impacted.iter().any(|c| c.severity >= Severity::Breaking)),
            };
            if report_json.is_some() || report_to.is_some() {
                let verdict = if failed { Outcome::Fail } else { Outcome::Pass };
                let run = RunContext::capture(Producer::Command, clock.stop());
                let doc =
                    ReportDocument::for_diff(&new_doc.id, &report, fail_on.name(), verdict, run);
                deliver(
                    &doc,
                    report_json.as_deref(),
                    report_to.as_ref(),
                    Say::human(format),
                )?;
            }
            Ok(if failed { EXIT_VIOLATED } else { EXIT_CLEAN })
        }

        Command::Gate {
            contract,
            model,
            dlq,
            stats,
            no_unique,
            quiet,
            subprocess,
            report_json,
            report_samples,
            report_to,
            openlineage,
            brokers,
            from,
            to,
            dlq_topic,
            group,
            exit_at_end,
            commit_interval_ms,
            kafka_property,
            kafka_config,
        } => {
            let clock = RunClock::start();
            let (doc, unenforced) = load_for_enforcement(&contract, allow_unenforced)?;
            let compiled = CompiledContract::compile(&doc)?;
            let target = compiled.resolve_model(model.as_deref())?;
            // A stream runs as long as it runs: a report with nowhere to go is
            // refused before the first record, not after the last.
            if let Some(path) = &report_json {
                crate::protocol::ensure_writable_dir(path)?;
            }
            let report_to = report_to.as_deref().map(Destination::parse).transpose()?;
            let lineage_to = openlineage.as_deref().map(Destination::parse).transpose()?;
            if lineage_to.is_some() && brokers.is_none() {
                return Err(CovenantError::Usage {
                    message: "--openlineage names the dataset a run was about, and a stream on \
                              stdin has no name: gate a topic (--brokers)"
                        .to_string(),
                });
            }
            let reporting = report_json.is_some() || report_to.is_some() || lineage_to.is_some();
            let sample_mode = match reporting {
                true => report_samples.mode()?,
                false => SampleMode::None,
            };
            let mut collector =
                reporting.then(|| crate::report::Collector::new(compiled.policy.sample_violations));
            // What the report says once the stream has ended: `source` names
            // the stream, `tombstones` counts what the Kafka gate passed on
            // unjudged, and `announce` says where the report went on stderr
            // (never in subprocess mode, where stderr is the reply channel).
            let write_report = |collector: Option<crate::report::Collector>,
                                outcome: &crate::gate::GateOutcome,
                                source: String,
                                tombstones: Option<u64>,
                                announce: bool|
             -> crate::error::Result<()> {
                let Some(collector) = collector else {
                    return Ok(());
                };
                let header = crate::report::ReportHeader {
                    contract_id: compiled.id.clone(),
                    contract_version: compiled.version.clone(),
                    owner: compiled.owner.clone(),
                    model: target.name.clone(),
                    source,
                };
                let mut report = collector.into_report(header, outcome.stats.records);
                report.unenforced = unenforced.clone();
                let budget = compiled.policy.max_violations;
                let warn_only = compiled.policy.on_violation == crate::spec::OnViolation::Warn;
                let verdict = Outcome::judge(report.violations, budget, warn_only);
                // The document says what the exit code says; the gate decides
                // both from the same count against the same budget.
                debug_assert_eq!(verdict == Outcome::Fail, outcome.failed);
                let run = RunContext::capture(Producer::Gate, clock.stop());
                let doc = ReportDocument::for_gate(
                    &report,
                    &outcome.stats,
                    tombstones,
                    budget,
                    verdict,
                    &sample_mode,
                    run,
                );
                let say = if announce { Say::Stderr } else { Say::Quiet };
                deliver(&doc, report_json.as_deref(), report_to.as_ref(), say)?;
                if let Some(to) = &lineage_to {
                    send_lineage(&doc, to, target, warn_only, say);
                }
                Ok(())
            };

            if let Some(brokers) = brokers {
                let source = kafka_source(&brokers, from.as_deref().unwrap_or_default());
                let flags = KafkaFlags {
                    brokers,
                    from: from.unwrap_or_default(),
                    to: to.unwrap_or_default(),
                    dlq_topic,
                    group,
                    exit_at_end,
                    commit_interval_ms,
                    properties: kafka_property,
                    config: kafka_config,
                };
                let (outcome, tombstones) = gate_kafka(
                    &compiled,
                    target,
                    flags,
                    dlq,
                    stats,
                    !no_unique,
                    quiet,
                    collector.as_mut(),
                )?;
                write_report(collector, &outcome, source, Some(tombstones), !quiet)?;
                return Ok(if outcome.failed {
                    EXIT_VIOLATED
                } else {
                    EXIT_CLEAN
                });
            }

            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            let mut out = BufWriter::new(stdout.lock());
            if subprocess {
                // stderr is the reply channel here, so dead letters go only to
                // --dlq, and no summary follows the stream.
                let mut replies = std::io::stderr().lock();
                let outcome = match &dlq {
                    Some(path) => {
                        let file = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                            .map_err(|e| CovenantError::Io {
                                path: path.display().to_string(),
                                source: e,
                            })?;
                        crate::gate::run_as_subprocess(
                            &compiled,
                            target,
                            stdin.lock(),
                            &mut out,
                            &mut replies,
                            &mut BufWriter::new(file),
                            !no_unique,
                            collector.as_mut(),
                        )?
                    }
                    None => crate::gate::run_as_subprocess(
                        &compiled,
                        target,
                        stdin.lock(),
                        &mut out,
                        &mut replies,
                        &mut std::io::sink(),
                        !no_unique,
                        collector.as_mut(),
                    )?,
                };
                drop(out);
                write_report(collector, &outcome, "stdin".to_string(), None, false)?;
                return Ok(if outcome.failed {
                    EXIT_VIOLATED
                } else {
                    EXIT_CLEAN
                });
            }
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
                        collector.as_mut(),
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
                        collector.as_mut(),
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
            write_report(collector, &outcome, "stdin".to_string(), None, !quiet)?;
            Ok(if outcome.failed {
                EXIT_VIOLATED
            } else {
                EXIT_CLEAN
            })
        }

        #[cfg(feature = "serve")]
        Command::Serve {
            contract,
            model,
            addr,
            dlq,
            gate_stats,
        } => {
            let (doc, unenforced) = load_for_enforcement(&contract, allow_unenforced)?;
            let compiled = CompiledContract::compile(&doc)?;
            let model_name = compiled.resolve_model(model.as_deref())?.name.clone();
            crate::serve::run(
                &addr,
                crate::serve::AppState {
                    source: contract.display().to_string(),
                    doc,
                    compiled,
                    model: model_name,
                    unenforced,
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
                    eprintln!(
                        "next: review the `confirm:` notes, then `covenant validate {}`",
                        path.display()
                    );
                }
                None => print!("{rendered}"),
            }
            // A draft is never a verdict: nothing was checked, so nothing
            // can have failed.
            Ok(EXIT_CLEAN)
        }

        Command::Push { documents, to } => {
            let to = Destination::parse(&to)?;
            let mut undelivered = 0usize;
            for path in &documents {
                let body = match std::fs::read(path) {
                    Ok(body) => body,
                    Err(e) => {
                        eprintln!("covenant: {}: {e}; not sent", path.display());
                        undelivered += 1;
                        continue;
                    }
                };
                // A document, by its revision key; anything else stays here.
                let revision = serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|doc| doc.get("report").and_then(serde_json::Value::as_u64));
                if revision != Some(u64::from(crate::protocol::REPORT_REVISION)) {
                    eprintln!(
                        "covenant: {}: not a covenant-report/v1 document; not sent",
                        path.display()
                    );
                    undelivered += 1;
                    continue;
                }
                match crate::send::post(&to, &body) {
                    Ok(()) => println!("pushed {} → {}", path.display(), to.url()),
                    Err(why) => {
                        eprintln!(
                            "covenant: {}: not sent to {}: {why}",
                            path.display(),
                            to.url()
                        );
                        undelivered += 1;
                    }
                }
            }
            Ok(if undelivered == 0 {
                EXIT_CLEAN
            } else {
                EXIT_ERROR
            })
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

#[cfg(test)]
mod tests {
    use super::kafka_source;

    #[test]
    fn a_topic_is_named_by_its_first_bootstrap_server_without_the_listener() {
        assert_eq!(
            kafka_source("b1:9092,b2:9092", "orders"),
            "kafka://b1:9092/orders"
        );
        assert_eq!(
            kafka_source(" SASL_SSL://b1:9093 ,b2:9093", "orders"),
            "kafka://b1:9093/orders"
        );
    }
}
