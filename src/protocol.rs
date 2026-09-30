//! The report protocol: one run's verdict as a versioned document,
//! `covenant-report/v1`.
//!
//! `check`, `gate` and `diff` write it with `--report-json <path>`, and the
//! Python face's `Report` writes it too. It carries what a CI artifact, a
//! catalog or a service needs to record a run — the verdict, the exact counts
//! per field and rule, and where and when the run happened — and, because the
//! document is made to leave the machine, samples that say where each
//! violation was but never what the value was. The JSON Schema is
//! `schema/covenant-report.v1.json`; golden files in `tests/golden/report/`
//! pin the document and hold it to the schema.
//!
//! Like every Covenant format it has a revision key (`report: 1`), and within
//! a revision it only grows: fields, kinds and planes are added, never renamed
//! or removed, so a reader ignores fields it does not know and skips a kind it
//! does not know.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::consumers::ConsumerImpactReport;
use crate::diff::{Change, DiffReport, Severity};
use crate::error::{CovenantError, Result};
use crate::gate::GateStats;
use crate::report::{CheckReport, Observed, Rule, RuleCount, ValueKind};
use crate::spec::UnenforcedRule;

/// The revision of the document this build writes.
pub const REPORT_REVISION: u32 = 1;

/// What a run did, which decides what its document holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Data checked against a contract: `covenant check`, or the Python face.
    Check,
    /// A stream held to a contract record by record: `covenant gate`.
    Gate,
    /// Two versions of a contract compared: `covenant diff`.
    Diff,
}

impl Kind {
    /// The name a document gives the kind.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Check => "check",
            Kind::Gate => "gate",
            Kind::Diff => "diff",
        }
    }
}

/// What a document's samples may carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleMode {
    /// Where each sampled violation was — field, rule, row — and what the
    /// value was without the value: its type and length. Never the value,
    /// nor the message, which quotes it.
    Masked,
    /// [`SampleMode::Masked`], and a keyed hash of each value, so two runs
    /// can say "the same bad value" without either saying what it was.
    Hashed(SampleKey),
    /// No samples: the counts per field and rule only.
    None,
}

impl SampleMode {
    /// The name a document gives the mode.
    pub fn name(&self) -> &'static str {
        match self {
            SampleMode::Masked => "masked",
            SampleMode::Hashed(_) => "hashed",
            SampleMode::None => "none",
        }
    }
}

/// The key a document's sample hashes are made with. It comes from
/// `COVENANT_SAMPLE_KEY`, never from the command line, and is never written
/// anywhere; its id, which is, says which key made a document's hashes, so a
/// reader compares hashes only under one key.
#[derive(Clone, PartialEq, Eq)]
pub struct SampleKey {
    key: Vec<u8>,
}

impl std::fmt::Debug for SampleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SampleKey")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}

impl SampleKey {
    /// Where the key is read from.
    pub const ENV: &'static str = "COVENANT_SAMPLE_KEY";
    /// The shortest key accepted: a short key would let anyone who tries
    /// candidate keys and candidate values together undo the hashes.
    pub const MIN_BYTES: usize = 16;

    pub fn new(key: &[u8]) -> Result<Self> {
        if key.len() < Self::MIN_BYTES {
            return Err(CovenantError::Usage {
                message: format!(
                    "hashed samples need a key of at least {} bytes in {}; it has {}",
                    Self::MIN_BYTES,
                    Self::ENV,
                    key.len()
                ),
            });
        }
        Ok(SampleKey { key: key.to_vec() })
    }

    /// The key in `COVENANT_SAMPLE_KEY`.
    pub fn from_env() -> Result<Self> {
        match std::env::var(Self::ENV) {
            Ok(key) => Self::new(key.as_bytes()),
            Err(std::env::VarError::NotPresent) => Err(CovenantError::Usage {
                message: format!(
                    "hashed samples need a key: set {} (at least {} bytes, e.g. \
                     `openssl rand -hex 32`); it is never read from the command line",
                    Self::ENV,
                    Self::MIN_BYTES
                ),
            }),
            Err(std::env::VarError::NotUnicode(_)) => Err(CovenantError::Usage {
                message: format!("{} is not UTF-8", Self::ENV),
            }),
        }
    }

    /// The key's id: the first 8 bytes, in hex, of the SHA-256 of a label and
    /// the key. It names the key without giving it away.
    pub fn id(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"covenant-report/v1 sample key\0");
        hasher.update(&self.key);
        hex(&hasher.finalize()[..8])
    }

    /// A value's hash: the first 16 bytes, in hex, of the HMAC-SHA256 of its
    /// digest under this key. The same value has the same hash under one
    /// key, and nothing about the value can be tried without the key.
    pub fn hash(&self, observed: &Observed) -> String {
        use hmac::{Hmac, Mac};
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&self.key)
            .expect("HMAC takes a key of any length");
        mac.update(observed.digest());
        hex(&mac.finalize().into_bytes()[..16])
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How the run ended, as its exit code says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Within the violation budget, or a contract change that `--fail-on`
    /// lets through (exit 0).
    Pass,
    /// Over the budget, or a change at or above `--fail-on` (exit 1).
    Fail,
    /// Over the budget under `on_violation: warn`: reported, not failed (exit 0).
    Warn,
}

impl Outcome {
    /// The verdict on data, as `check` and the Python face decide it: more
    /// violations than the budget fail the run, unless the contract only
    /// warns.
    pub fn judge(violations: u64, budget: u64, warn_only: bool) -> Self {
        match (violations > budget, warn_only) {
            (false, _) => Outcome::Pass,
            (true, false) => Outcome::Fail,
            (true, true) => Outcome::Warn,
        }
    }
}

/// `covenant-report/v1`: one run's verdict.
#[derive(Debug, Serialize)]
pub struct ReportDocument {
    /// The revision key, as every Covenant format has one.
    pub report: u32,
    pub kind: Kind,
    pub verdict: Outcome,
    pub engine: Engine,
    pub run: RunContext,
    #[serde(flatten)]
    pub body: Body,
}

/// What a document says, by its kind.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Body {
    /// `check` and `gate`: data held to a contract.
    Data {
        /// The run's violation budget; the verdict compares the total with it.
        budget: u64,
        /// Violations across every source of the run.
        violations: u64,
        /// `masked`, `hashed` or `none`.
        sample_mode: &'static str,
        /// `hashed` only: the id of the key the hashes were made with.
        #[serde(skip_serializing_if = "Option::is_none")]
        sample_key: Option<String>,
        /// One entry per source, in the order given; a gate has one, its
        /// stream.
        checks: Vec<CheckEntry>,
        /// `gate` only: what became of the stream's records.
        #[serde(skip_serializing_if = "Option::is_none")]
        gate: Option<GateCounts>,
    },
    /// `diff`: a contract change, classified.
    Diff { diff: DiffEntry },
}

/// The engine that produced a document.
#[derive(Debug, Serialize)]
pub struct Engine {
    pub name: &'static str,
    pub version: &'static str,
}

/// One source's verdict. It is built field by field from the check's report,
/// so a field added to [`CheckReport`] reaches this document only when it is
/// added here, on purpose: nothing new leaves the machine by accident.
#[derive(Debug, Serialize)]
pub struct CheckEntry {
    pub contract_id: String,
    pub contract_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub model: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_consumer: Option<String>,
    pub rows: u64,
    pub violations: u64,
    pub per_rule: Vec<RuleCount>,
    pub samples: Vec<MaskedSample>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unenforced: Vec<UnenforcedRule>,
}

impl CheckEntry {
    fn from_report(r: &CheckReport, sample_mode: &SampleMode) -> Self {
        CheckEntry {
            contract_id: r.contract_id.clone(),
            contract_version: r.contract_version.clone(),
            owner: r.owner.clone(),
            model: r.model.clone(),
            source: r.source.clone(),
            as_consumer: r.as_consumer.clone(),
            rows: r.rows,
            violations: r.violations,
            per_rule: r.per_rule.clone(),
            samples: match sample_mode {
                SampleMode::None => Vec::new(),
                SampleMode::Masked | SampleMode::Hashed(_) => r
                    .samples
                    .iter()
                    .map(|v| {
                        let observed = v.observed.as_ref();
                        MaskedSample {
                            field: v.field.clone(),
                            rule: v.rule,
                            row: v.row,
                            value_type: observed.map(|o| o.kind),
                            length: observed.and_then(|o| o.length),
                            hash: match (sample_mode, observed) {
                                (SampleMode::Hashed(key), Some(o)) => Some(key.hash(o)),
                                _ => None,
                            },
                        }
                    })
                    .collect(),
            },
            unenforced: r.unenforced.clone(),
        }
    }
}

/// Where one sampled violation was, and what its value was without the
/// value.
#[derive(Debug, Serialize)]
pub struct MaskedSample {
    /// Absent for record-level findings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub rule: Rule,
    /// 0-based record index; absent for schema-level findings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<u64>,
    /// The offending value's type; absent when the finding has no value (a
    /// missing field, a missing column, a line that is not JSON).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub value_type: Option<ValueKind>,
    /// Characters of a string, items of an array, keys of an object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    /// `hashed` only: the value's hash under the document's key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// What became of a gated stream's records.
#[derive(Debug, Serialize)]
pub struct GateCounts {
    /// Records that went on, including those passed under `on_violation: warn`.
    pub passed: u64,
    /// Records withheld and dead-lettered.
    pub blocked: u64,
    /// Records that broke the contract and went on under `on_violation: warn`.
    pub warned: u64,
    /// The Kafka gate only: records with no value (tombstones), which go on
    /// unjudged and are not among the rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstones: Option<u64>,
}

/// A contract change, classified. Contracts are metadata, so nothing here is
/// masked: the messages quote the contracts, never data.
#[derive(Debug, Serialize)]
pub struct DiffEntry {
    /// The proposed contract's id.
    pub contract_id: String,
    pub old_version: String,
    pub new_version: String,
    /// The threshold the verdict was decided by (`--fail-on`).
    pub fail_on: &'static str,
    /// The worst change's severity; absent when nothing changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_severity: Option<Severity>,
    pub changes: Vec<Change>,
    /// Who the changes break, when consumer manifests were given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer_impact: Option<ConsumerImpactReport>,
    /// Rules on either side that were not compared (`--allow-unenforced`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unenforced: Vec<UnenforcedRule>,
}

/// What produced a document, which decides its plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Producer {
    /// A command run to a verdict (`check`, `diff`): the `ci` plane when a CI
    /// provider is detected, `cli` otherwise.
    Command,
    /// The stream gate: the `gate` plane, in CI or not.
    Gate,
    /// The Python face: the `python` plane, in CI or not.
    Python,
}

/// A run's clock: when it started, and a monotonic instant to time it by.
#[derive(Debug, Clone, Copy)]
pub struct RunClock {
    started: chrono::DateTime<chrono::Utc>,
    at: Instant,
}

impl RunClock {
    pub fn start() -> Self {
        RunClock {
            started: chrono::Utc::now(),
            at: Instant::now(),
        }
    }

    /// When the run started and how long it has taken so far.
    pub fn stop(&self) -> RunTiming {
        RunTiming {
            started: self.started,
            elapsed: self.at.elapsed(),
        }
    }
}

/// When a run started and how long it took.
#[derive(Debug, Clone, Copy)]
pub struct RunTiming {
    pub started: chrono::DateTime<chrono::Utc>,
    pub elapsed: Duration,
}

/// Where and when a run happened.
#[derive(Debug, Serialize)]
pub struct RunContext {
    /// What produced the document: `cli` or `ci` for a command, `gate` for
    /// the stream gate, `python` for the Python face.
    pub plane: &'static str,
    /// When the run started: RFC 3339, UTC.
    pub started_at: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiContext>,
}

impl RunContext {
    /// The context of a run `producer` made, timed by `timing`, its CI
    /// provider read from the environment.
    pub fn capture(producer: Producer, timing: RunTiming) -> Self {
        Self::new(producer, timing, CiContext::from_env())
    }

    pub fn new(producer: Producer, timing: RunTiming, ci: Option<CiContext>) -> Self {
        let plane = match producer {
            Producer::Command if ci.is_some() => "ci",
            Producer::Command => "cli",
            Producer::Gate => "gate",
            Producer::Python => "python",
        };
        RunContext {
            plane,
            started_at: timing
                .started
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            duration_ms: u64::try_from(timing.elapsed.as_millis()).unwrap_or(u64::MAX),
            ci,
        }
    }
}

/// The CI run a document came from, from the provider's own variables. These
/// are the only variables read, and only when a document is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CiContext {
    /// `github-actions`, `gitlab-ci`, or `unknown` for a CI that says only
    /// `CI=true`.
    pub provider: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// The full ref: `refs/heads/…`, `refs/tags/…` or `refs/pull/…`.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
}

impl CiContext {
    /// The CI provider of this process, if any.
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// The CI provider as `var` (a variable's value by name) describes it.
    pub fn from_lookup(var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let get = |key: &str| var(key).filter(|v| !v.is_empty());
        if get("GITHUB_ACTIONS").as_deref() == Some("true") {
            let repository = get("GITHUB_REPOSITORY");
            let run_url = match (get("GITHUB_SERVER_URL"), &repository, get("GITHUB_RUN_ID")) {
                (Some(server), Some(repo), Some(id)) => Some(format!(
                    "{}/{repo}/actions/runs/{id}",
                    server.trim_end_matches('/')
                )),
                _ => None,
            };
            return Some(CiContext {
                provider: "github-actions",
                repository,
                sha: get("GITHUB_SHA"),
                git_ref: get("GITHUB_REF"),
                run_url,
            });
        }
        if get("GITLAB_CI").as_deref() == Some("true") {
            // GitLab names the ref bare; spell it out as GitHub does, so the
            // field means one thing whichever CI wrote it.
            let git_ref = get("CI_COMMIT_TAG")
                .map(|tag| format!("refs/tags/{tag}"))
                .or_else(|| get("CI_COMMIT_BRANCH").map(|b| format!("refs/heads/{b}")));
            return Some(CiContext {
                provider: "gitlab-ci",
                repository: get("CI_PROJECT_PATH"),
                sha: get("CI_COMMIT_SHA"),
                git_ref,
                run_url: get("CI_JOB_URL"),
            });
        }
        if get("CI").is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1") {
            return Some(CiContext {
                provider: "unknown",
                repository: None,
                sha: None,
                git_ref: None,
                run_url: None,
            });
        }
        None
    }
}

impl ReportDocument {
    fn new(kind: Kind, verdict: Outcome, run: RunContext, body: Body) -> Self {
        ReportDocument {
            report: REPORT_REVISION,
            kind,
            verdict,
            engine: Engine {
                name: "covenant",
                version: env!("CARGO_PKG_VERSION"),
            },
            run,
            body,
        }
    }

    /// The document for a `check` run over `reports`.
    pub fn for_check(
        reports: &[CheckReport],
        budget: u64,
        verdict: Outcome,
        sample_mode: &SampleMode,
        run: RunContext,
    ) -> Self {
        let body = Body::Data {
            budget,
            violations: reports.iter().map(|r| r.violations).sum(),
            sample_mode: sample_mode.name(),
            sample_key: sample_key(sample_mode),
            checks: reports
                .iter()
                .map(|r| CheckEntry::from_report(r, sample_mode))
                .collect(),
            gate: None,
        };
        Self::new(Kind::Check, verdict, run, body)
    }

    /// The document for a gate run: `report` is the stream's verdict, as a
    /// check of it would give it, and `stats` what became of its records;
    /// `tombstones`, for the Kafka gate, the records it passed on unjudged.
    #[allow(clippy::too_many_arguments)]
    pub fn for_gate(
        report: &CheckReport,
        stats: &GateStats,
        tombstones: Option<u64>,
        budget: u64,
        verdict: Outcome,
        sample_mode: &SampleMode,
        run: RunContext,
    ) -> Self {
        let body = Body::Data {
            budget,
            violations: report.violations,
            sample_mode: sample_mode.name(),
            sample_key: sample_key(sample_mode),
            checks: vec![CheckEntry::from_report(report, sample_mode)],
            gate: Some(GateCounts {
                passed: stats.passed,
                blocked: stats.blocked,
                warned: stats.warned,
                tombstones,
            }),
        };
        Self::new(Kind::Gate, verdict, run, body)
    }

    /// The document for a `diff` run between two versions of `contract_id`,
    /// its verdict decided by `fail_on`.
    pub fn for_diff(
        contract_id: &str,
        report: &DiffReport,
        fail_on: &'static str,
        verdict: Outcome,
        run: RunContext,
    ) -> Self {
        let body = Body::Diff {
            diff: DiffEntry {
                contract_id: contract_id.to_string(),
                old_version: report.old_version.clone(),
                new_version: report.new_version.clone(),
                fail_on,
                max_severity: report.max_severity(),
                changes: report.changes.clone(),
                consumer_impact: report.consumer_impact.clone(),
                unenforced: report.unenforced.clone(),
            },
        };
        Self::new(Kind::Diff, verdict, run, body)
    }

    /// The document as it is written and sent: pretty JSON, one line end.
    pub fn to_json(&self) -> Vec<u8> {
        let mut json = serde_json::to_vec_pretty(self).expect("report documents serialize");
        json.push(b'\n');
        json
    }

    /// Write the document to `path`.
    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_json()).map_err(|source| CovenantError::Io {
            path: path.display().to_string(),
            source,
        })
    }
}

fn sample_key(sample_mode: &SampleMode) -> Option<String> {
    match sample_mode {
        SampleMode::Hashed(key) => Some(key.id()),
        _ => None,
    }
}

/// Refuse, before a run starts, a document path whose directory does not
/// exist: a gate reads its stream for as long as it runs, and should not
/// learn at the end that its verdict has nowhere to go.
pub fn ensure_writable_dir(path: &Path) -> Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    if dir.is_dir() {
        return Ok(());
    }
    Err(CovenantError::Io {
        path: path.display().to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("directory {} does not exist", dir.display()),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ci(vars: &[(&str, &str)]) -> Option<CiContext> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        CiContext::from_lookup(|k| vars.get(k).cloned())
    }

    #[test]
    fn github_actions_names_the_repository_the_commit_the_ref_and_the_run() {
        let c = ci(&[
            ("GITHUB_ACTIONS", "true"),
            ("CI", "true"),
            ("GITHUB_REPOSITORY", "acme/shop"),
            ("GITHUB_SHA", "0123abc"),
            ("GITHUB_REF", "refs/pull/7/merge"),
            ("GITHUB_SERVER_URL", "https://github.com/"),
            ("GITHUB_RUN_ID", "42"),
        ])
        .unwrap();
        assert_eq!(c.provider, "github-actions");
        assert_eq!(c.repository.as_deref(), Some("acme/shop"));
        assert_eq!(c.sha.as_deref(), Some("0123abc"));
        assert_eq!(c.git_ref.as_deref(), Some("refs/pull/7/merge"));
        assert_eq!(
            c.run_url.as_deref(),
            Some("https://github.com/acme/shop/actions/runs/42")
        );
    }

    #[test]
    fn gitlab_spells_its_ref_out_in_full() {
        let branch = ci(&[
            ("GITLAB_CI", "true"),
            ("CI_PROJECT_PATH", "acme/shop"),
            ("CI_COMMIT_SHA", "0123abc"),
            ("CI_COMMIT_BRANCH", "main"),
            ("CI_JOB_URL", "https://gitlab.com/acme/shop/-/jobs/9"),
        ])
        .unwrap();
        assert_eq!(branch.provider, "gitlab-ci");
        assert_eq!(branch.git_ref.as_deref(), Some("refs/heads/main"));
        assert_eq!(
            branch.run_url.as_deref(),
            Some("https://gitlab.com/acme/shop/-/jobs/9")
        );
        let tag = ci(&[
            ("GITLAB_CI", "true"),
            ("CI_COMMIT_TAG", "v1.2.0"),
            ("CI_COMMIT_BRANCH", "main"),
        ])
        .unwrap();
        assert_eq!(tag.git_ref.as_deref(), Some("refs/tags/v1.2.0"));
    }

    #[test]
    fn a_ci_that_says_only_ci_true_is_unknown_and_no_ci_is_none() {
        assert_eq!(ci(&[("CI", "true")]).unwrap().provider, "unknown");
        assert_eq!(ci(&[("CI", "1")]).unwrap().provider, "unknown");
        assert!(ci(&[("CI", "false")]).is_none());
        assert!(ci(&[]).is_none());
        // An empty variable is an unset one.
        assert!(ci(&[("GITHUB_ACTIONS", "")]).is_none());
    }

    #[test]
    fn a_command_is_on_the_ci_plane_only_in_ci_and_the_others_keep_theirs() {
        let timing = RunTiming {
            started: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            elapsed: Duration::from_millis(1500),
        };
        let local = RunContext::new(Producer::Command, timing, None);
        assert_eq!(local.plane, "cli");
        assert_eq!(local.started_at, "1970-01-01T00:00:00.000Z");
        assert_eq!(local.duration_ms, 1500);
        let in_ci = || ci(&[("CI", "true")]);
        assert_eq!(
            RunContext::new(Producer::Command, timing, in_ci()).plane,
            "ci"
        );
        for (producer, plane) in [(Producer::Gate, "gate"), (Producer::Python, "python")] {
            assert_eq!(RunContext::new(producer, timing, None).plane, plane);
            let run = RunContext::new(producer, timing, in_ci());
            assert_eq!(run.plane, plane);
            assert!(
                run.ci.is_some(),
                "a {plane} run in CI still names the CI run"
            );
        }
    }

    #[test]
    fn data_is_judged_against_the_budget_and_a_warn_contract_never_fails() {
        assert_eq!(Outcome::judge(0, 0, false), Outcome::Pass);
        assert_eq!(Outcome::judge(3, 3, false), Outcome::Pass);
        assert_eq!(Outcome::judge(4, 3, false), Outcome::Fail);
        assert_eq!(Outcome::judge(4, 3, true), Outcome::Warn);
        assert_eq!(Outcome::judge(3, 3, true), Outcome::Pass);
    }

    #[test]
    fn a_sample_key_is_long_enough_named_by_its_id_and_never_shown() {
        assert!(SampleKey::new(b"fifteen bytes!!").is_err());
        let key = SampleKey::new(b"0123456789abcdef-a").unwrap();
        let other = SampleKey::new(b"0123456789abcdef-b").unwrap();
        // Pinned: a hash that changed between releases would stop two runs
        // from saying "the same bad value" (Python's hmac gives the same).
        assert_eq!(key.id(), "b07ddba8cfbf0b9c");
        let btc = Observed::string("BTC");
        assert_eq!(key.hash(&btc), "35a460dfd1c5a5111443b4edde57ba33");
        assert_ne!(key.id(), other.id());
        // Debug output names the key by its id, and never shows the key.
        let shown = format!("{key:?}");
        assert!(shown.contains(&key.id()), "{shown}");
        assert!(!shown.contains("0123456789abcdef"), "{shown}");
        assert_eq!(
            key.hash(&btc),
            key.hash(&Observed::json(&serde_json::json!("BTC")))
        );
        assert_ne!(key.hash(&btc), key.hash(&Observed::string("ETH")));
        assert_ne!(key.hash(&btc), other.hash(&btc));
        // A string and a number that read alike are different values.
        assert_ne!(
            key.hash(&Observed::string("5")),
            key.hash(&Observed::integer(5))
        );
    }

    #[test]
    fn a_report_path_in_a_missing_directory_is_refused_up_front() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ensure_writable_dir(&dir.path().join("r.json")).is_ok());
        assert!(ensure_writable_dir(Path::new("r.json")).is_ok());
        let err = ensure_writable_dir(&dir.path().join("missing/r.json")).unwrap_err();
        assert!(err.to_string().contains("missing"), "{err}");
    }
}
