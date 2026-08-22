//! Consumer manifests + blast radius — the second half of the merge gate.
//!
//! `covenant diff` classifies *what* changed; this module answers *who
//! breaks*. Consumers declare what they actually read in a small manifest
//! (`consumer: 1`), kept in their own repo or a shared `consumers/`
//! directory, and `covenant diff --consumers <dir>` intersects the diff's
//! consumer-impacting changes with those declarations to produce an exact,
//! named impact list — "this change breaks finance_daily_rollup, owned by
//! finance-eng" — instead of the generic "impact: consumers" axis alone.
//!
//! ```yaml
//! consumer: 1
//! id: finance_daily_rollup
//! owner: finance-eng@acme.io
//! consumes:
//!   - contract: orders
//!     fields: [customer_email, currency]   # omit `fields` = whole model
//! ```
//!
//! Matching rules (deliberately conservative):
//! - Only changes whose impact is `consumers` or `both` can hit a consumer —
//!   producer-side changes (a tightened bound, a new required field) don't
//!   break readers.
//! - A field-level change hits the consumers that declared *that* field
//!   (or consume the whole model).
//! - A model-level change (model removed, strictness) hits every consumer
//!   of that model; a policy change hits every consumer of the contract —
//!   the enforcement guarantee itself weakened.
//! - The semver-bump finding (`path: version`) is process discipline, not a
//!   data-shape change, and is excluded from the blast radius.
//! - When dotted model/field names make a change path ambiguous, every
//!   interpretation counts — ambiguity can only widen the blast radius,
//!   never hide a broken consumer (see [`classify_targets`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diff::{DiffReport, Impact, Severity};
use crate::error::{CovenantError, Result};
use crate::spec::{Contract, LintFinding, LintLevel};

/// One consumer's declaration of what it reads — the manifest document
/// (`consumer: 1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumerManifest {
    /// Manifest spec revision (this runtime speaks 1).
    pub consumer: u32,
    /// The consumer's identity — a job, dashboard, or service name.
    pub id: String,
    /// Who to notify when this consumer is in a change's blast radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What this consumer reads, per contract.
    pub consumes: Vec<Consumption>,
}

/// One (contract, model, fields) block a consumer reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consumption {
    /// The contract id (`Contract::id`) being consumed.
    pub contract: String,
    /// Model within the contract. Omitted = every model of the contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The fields actually read. Omitted = the whole model (a replicator or
    /// SELECT-* consumer). An explicit empty list is rejected as ambiguous.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    /// The contract version (semver) this consumption was last reviewed
    /// against. `covenant consumer-check` fails when the contract's breaking
    /// segment moves past the pin — the MAJOR version, or the MINOR version
    /// while the major is still 0 (pre-1.0 minors are breaking, semver
    /// item 4) — the consumer must re-review and re-pin. Non-breaking drift
    /// is a warning. Omitted = no drift check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<String>,
}

impl ConsumerManifest {
    /// Parse and validate a manifest from YAML (JSON parses through the same
    /// path). Invalid manifests are hard errors — a half-parsed blast radius
    /// would report "nobody breaks" exactly when someone does.
    pub fn parse(source: &str, origin: &str) -> Result<ConsumerManifest> {
        let manifest: ConsumerManifest =
            serde_yaml::from_str(source).map_err(|e| CovenantError::ManifestInvalid {
                path: origin.to_string(),
                message: e.to_string(),
            })?;
        manifest.validate(origin)?;
        Ok(manifest)
    }

    /// Read and parse a manifest file.
    pub fn from_path(path: &Path) -> Result<ConsumerManifest> {
        let text = std::fs::read_to_string(path).map_err(|e| CovenantError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        ConsumerManifest::parse(&text, &path.display().to_string())
    }

    fn validate(&self, origin: &str) -> Result<()> {
        let err = |message: String| {
            Err(CovenantError::ManifestInvalid {
                path: origin.to_string(),
                message,
            })
        };
        if self.consumer != 1 {
            return err(format!(
                "unsupported manifest revision {} (this runtime speaks 1)",
                self.consumer
            ));
        }
        if self.id.trim().is_empty() {
            return err("consumer id must be non-empty".into());
        }
        if self.consumes.is_empty() {
            return err("manifest declares no `consumes` blocks".into());
        }
        for (i, c) in self.consumes.iter().enumerate() {
            if c.contract.trim().is_empty() {
                return err(format!("consumes[{i}].contract must be non-empty"));
            }
            // A blank selector would match no model at all and silently
            // shrink the blast radius — reject it like every other typo.
            if c.model.as_deref().is_some_and(|m| m.trim().is_empty()) {
                return err(format!(
                    "consumes[{i}].model is blank — omit `model` to consume every \
                     model of the contract, or name one"
                ));
            }
            if let Some(fields) = &c.fields {
                if fields.is_empty() {
                    return err(format!(
                        "consumes[{i}].fields is an empty list — omit `fields` to consume \
                         the whole model, or list the fields actually read"
                    ));
                }
                if let Some(f) = fields.iter().find(|f| f.trim().is_empty()) {
                    return err(format!("consumes[{i}] declares an empty field name {f:?}"));
                }
            }
            if let Some(pin) = &c.verified {
                if semver::Version::parse(pin).is_err() {
                    return err(format!(
                        "consumes[{i}].verified {pin:?} is not valid semver (expected e.g. 1.2.0)"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Does any `consumes` block name this contract id?
    pub fn consumes_contract(&self, contract_id: &str) -> bool {
        self.consumes.iter().any(|c| c.contract == contract_id)
    }

    /// Verify this manifest against a contract — the consumer-side gate.
    /// Only blocks naming the contract's id are checked; each must resolve
    /// (the named model exists, every declared field exists) and its version
    /// pin must not have been left behind by a major bump. Errors mean the
    /// manifest is stale or wrong; warnings mean drift worth reviewing.
    pub fn verify_against(&self, contract: &Contract) -> Vec<LintFinding> {
        let mut out = Vec::new();
        let err = |path: String, message: String| LintFinding {
            level: LintLevel::Error,
            path,
            message,
        };
        let warn = |path: String, message: String| LintFinding {
            level: LintLevel::Warning,
            path,
            message,
        };

        for (i, c) in self.consumes.iter().enumerate() {
            if c.contract != contract.id {
                continue;
            }

            // Resolve which models this block reads.
            let models: Vec<&str> = match c.model.as_deref() {
                Some(m) => {
                    if contract.models.contains_key(m) {
                        vec![m]
                    } else {
                        out.push(err(
                            format!("consumes[{i}].model"),
                            format!(
                                "model {m:?} not in contract {:?} (available: {})",
                                contract.id,
                                contract
                                    .models
                                    .keys()
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            ),
                        ));
                        continue;
                    }
                }
                None => contract.models.keys().map(String::as_str).collect(),
            };

            // Every declared field must exist in at least one of the
            // block's models — a missing field means the consumer reads
            // something the contract no longer enforces.
            if let Some(fields) = &c.fields {
                let mut seen: Vec<&str> = Vec::new();
                for f in fields {
                    if seen.contains(&f.as_str()) {
                        out.push(warn(
                            format!("consumes[{i}].fields.{f}"),
                            "declared more than once".into(),
                        ));
                        continue;
                    }
                    seen.push(f);
                    let exists = models
                        .iter()
                        .any(|m| contract.models[*m].fields.contains_key(f.as_str()));
                    if !exists {
                        out.push(err(
                            format!("consumes[{i}].fields.{f}"),
                            format!(
                                "field {f:?} is not enforced by {} of contract {:?} — \
                                 the manifest is stale or the field was removed",
                                if models.len() == 1 {
                                    format!("model {:?}", models[0])
                                } else {
                                    "any model".to_string()
                                },
                                contract.id,
                            ),
                        ));
                    } else if let Some(note) = models.iter().find_map(|m| {
                        contract.models[*m]
                            .fields
                            .get(f.as_str())
                            .and_then(|fd| fd.deprecated.as_ref())
                    }) {
                        // The consumer-side half of the deprecation
                        // countdown: every CI run reminds the reader that
                        // this field is on its way out.
                        out.push(warn(
                            format!("consumes[{i}].fields.{f}"),
                            if note.is_empty() {
                                format!("field {f:?} is deprecated — plan migration")
                            } else {
                                format!("field {f:?} is deprecated ({note}) — plan migration")
                            },
                        ));
                    }
                }
            } else {
                // A whole-model block reads every field, deprecated ones
                // included — the reader gets the same countdown reminder the
                // field-listing consumer gets, just anchored at the block.
                let mut deprecated: Vec<&str> = Vec::new();
                for m in &models {
                    for (fname, fd) in &contract.models[*m].fields {
                        if fd.deprecated.is_some() && !deprecated.contains(&fname.as_str()) {
                            deprecated.push(fname);
                        }
                    }
                }
                if !deprecated.is_empty() {
                    out.push(warn(
                        format!("consumes[{i}]"),
                        format!(
                            "this block reads whole models, including deprecated \
                             field(s) {} — plan migration",
                            deprecated.join(", "),
                        ),
                    ));
                }
            }

            // Version-pin drift.
            if let Some(pin) = &c.verified {
                let path = format!("consumes[{i}].verified");
                // Both parse: the manifest pin was validated at load; a
                // contract with a bad version fails its own lint elsewhere.
                match (
                    semver::Version::parse(pin),
                    semver::Version::parse(&contract.version),
                ) {
                    (Ok(pinned), Ok(current)) => {
                        // Pre-1.0 versions break on MINOR bumps (semver spec
                        // item 4), so the breaking segment is the major — or
                        // (0, minor) while the major is still 0. A consumer
                        // pinned at 0.1.x must fail against 0.2.0.
                        let breaking_segment =
                            |v: &semver::Version| (v.major, if v.major == 0 { v.minor } else { 0 });
                        if breaking_segment(&current) > breaking_segment(&pinned) {
                            let bump = if current.major > pinned.major {
                                "major"
                            } else {
                                "0.x minor (breaking pre-1.0)"
                            };
                            out.push(err(
                                path,
                                format!(
                                    "pinned at v{pinned} but the contract is v{current} — a {bump} \
                                     bump means breaking changes shipped; re-review and update `verified`"
                                ),
                            ));
                        } else if pinned > current {
                            out.push(warn(
                                path,
                                format!(
                                    "pinned at v{pinned}, ahead of the contract being checked \
                                     (v{current}) — verifying against an old contract copy?"
                                ),
                            ));
                        } else if current > pinned {
                            out.push(warn(
                                path,
                                format!(
                                    "contract moved v{pinned} -> v{current} since last review \
                                     (non-breaking by semver; re-pin after a look)"
                                ),
                            ));
                        }
                    }
                    (Err(_), _) | (_, Err(_)) => {
                        out.push(err(
                            path,
                            "version pin or contract version is not semver".into(),
                        ));
                    }
                }
            }
        }
        out
    }
}

/// Load every manifest under `dir` (recursive; `.yaml`/`.yml`/`.json`),
/// in deterministic path order. A directory with no manifest files is an
/// error, not an empty set — `--fail-on breaking-with-consumers` against a
/// mistyped path must not silently pass everything.
pub fn load_dir(dir: &Path) -> Result<Vec<ConsumerManifest>> {
    Ok(load_dir_entries(dir)?.into_iter().map(|(_, m)| m).collect())
}

/// [`load_dir`], keeping each manifest's source path (for reports that name
/// the file, e.g. `covenant consumer-check`).
pub fn load_dir_entries(dir: &Path) -> Result<Vec<(PathBuf, ConsumerManifest)>> {
    let mut files = Vec::new();
    collect_files(dir, &mut files)?;
    if files.is_empty() {
        return Err(CovenantError::ManifestInvalid {
            path: dir.display().to_string(),
            message: "no consumer manifests found (*.yaml/*.yml/*.json)".into(),
        });
    }
    files.sort();
    files
        .into_iter()
        .map(|p| ConsumerManifest::from_path(&p).map(|m| (p, m)))
        .collect()
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let io_err = |e: std::io::Error| CovenantError::Io {
        path: dir.display().to_string(),
        source: e,
    };
    for entry in std::fs::read_dir(dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        // Recurse only into real directories — a symlinked directory could
        // point at an ancestor and recurse forever. Symlinked manifest
        // *files* are fine (path.is_file() follows the link).
        let file_type = entry.file_type().map_err(io_err)?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(&path, out)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yaml" | "yml" | "json")
        ) && path.is_file()
        {
            out.push(path);
        }
    }
    Ok(())
}

/// One manifest's verification verdict against a contract — the unit of
/// `covenant consumer-check` output and `POST /v1/consumers/verify`.
#[derive(Debug, Serialize)]
pub struct VerifyResult {
    pub consumer: String,
    /// The manifest's file path (CLI runs); absent for API-submitted
    /// manifests, which have no path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub consumes_contract: bool,
    pub findings: Vec<LintFinding>,
}

/// The one envelope for consumer verification — shared by the CLI's JSON
/// output and the serve endpoint so a script works against either, and a
/// stored artifact records WHICH contract version graded the pins.
#[derive(Debug, Serialize)]
pub struct ConsumerCheckReport {
    pub contract: String,
    pub version: String,
    pub results: Vec<VerifyResult>,
}

/// One consumer hit by the diff, with the evidence: worst severity, the
/// declared fields that were touched, and the change paths responsible.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactedConsumer {
    pub consumer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Worst severity among the changes that hit this consumer.
    pub severity: Severity,
    /// Declared fields hit by field-level changes (empty when the hit is
    /// model- or contract-wide).
    pub fields: Vec<String>,
    /// Paths of the changes that hit this consumer.
    pub changes: Vec<String>,
}

/// The blast radius attached to a [`DiffReport`] when manifests were
/// supplied. `manifests` vs `consumers_of_contract` distinguishes "nobody
/// consumes this contract" from "nobody was checked".
#[derive(Debug, Clone, Serialize)]
pub struct ConsumerImpactReport {
    /// The contract id the manifests were matched against.
    pub contract: String,
    /// Manifests scanned (whether or not they consume this contract).
    pub manifests: usize,
    /// Manifests with at least one `consumes` block for this contract.
    pub consumers_of_contract: usize,
    /// Consumers hit by at least one consumer-impacting change,
    /// worst severity first.
    pub impacted: Vec<ImpactedConsumer>,
    /// Consumers of this contract that no change touches.
    pub unaffected: Vec<String>,
}

/// What part of the contract a classified change targets, resolved by exact
/// path match against the real model/field names (never by string parsing —
/// names may themselves contain dots).
enum Target<'a> {
    Field(&'a str, &'a str),
    Model(&'a str),
    /// Policy changes and any future consumer-impacting change without a
    /// narrower address: everyone consuming the contract is affected.
    ContractWide,
}

/// Every (model, field) interpretation a change path can name. Dotted model
/// or field names can make two distinct targets render to the same path
/// (`models.a.b.fields.fields.c` is model `a.b` field `fields.c` AND model
/// `a.b.fields` field `c`), so the matcher keeps ALL candidates and a
/// consumer is hit when ANY of them matches — ambiguity can only widen the
/// blast radius (fail closed), never hide a broken consumer. The semver-bump
/// finding (`version`) returns no targets: process discipline, not a shape
/// change.
fn classify_targets<'a>(path: &str, old: &'a Contract, new: &'a Contract) -> Vec<Target<'a>> {
    if path == "version" {
        return Vec::new();
    }
    let mut model_names: Vec<&str> = old.models.keys().map(String::as_str).collect();
    for name in new.models.keys() {
        if !model_names.contains(&name.as_str()) {
            model_names.push(name);
        }
    }
    let mut out = Vec::new();
    for mname in model_names {
        if path == format!("models.{mname}") {
            out.push(Target::Model(mname));
        }
        let old_fields = old.models.get(mname).map(|m| m.fields.keys());
        let new_fields = new.models.get(mname).map(|m| m.fields.keys());
        for fname in old_fields
            .into_iter()
            .flatten()
            .chain(new_fields.into_iter().flatten())
        {
            if path == format!("models.{mname}.fields.{fname}")
                && !out.iter().any(
                    |t| matches!(t, Target::Field(m, f) if *m == mname && *f == fname.as_str()),
                )
            {
                out.push(Target::Field(mname, fname));
            }
        }
    }
    if out.is_empty() {
        out.push(Target::ContractWide);
    }
    out
}

/// Compute the blast radius for a diff and attach it to the report. The
/// manifests are matched against the diffed contract's id (old or new side).
pub fn annotate(
    report: &mut DiffReport,
    old: &Contract,
    new: &Contract,
    manifests: &[ConsumerManifest],
) {
    let consumes_this = |m: &ConsumerManifest| {
        m.consumes
            .iter()
            .any(|c| c.contract == old.id || c.contract == new.id)
    };

    let mut impacted: Vec<ImpactedConsumer> = Vec::new();
    for manifest in manifests.iter().filter(|m| consumes_this(m)) {
        let mut severity: Option<Severity> = None;
        let mut fields: Vec<String> = Vec::new();
        let mut changes: Vec<String> = Vec::new();

        for change in &report.changes {
            if change.impact == Impact::Producers {
                continue;
            }
            let targets = classify_targets(&change.path, old, new);
            let mut hit = false;
            for target in &targets {
                let matched = manifest
                    .consumes
                    .iter()
                    .filter(|c| c.contract == old.id || c.contract == new.id)
                    .any(|c| match target {
                        Target::ContractWide => true,
                        Target::Model(m) => c.model.as_deref().is_none_or(|cm| cm == *m),
                        Target::Field(m, f) => {
                            c.model.as_deref().is_none_or(|cm| cm == *m)
                                && c.fields.as_ref().is_none_or(|fs| fs.iter().any(|d| d == f))
                        }
                    });
                if matched {
                    hit = true;
                    if let Target::Field(_, f) = target {
                        if !fields.iter().any(|x| x == f) {
                            fields.push((*f).to_string());
                        }
                    }
                }
            }
            if hit {
                severity = severity.max(Some(change.severity));
                if !changes.contains(&change.path) {
                    changes.push(change.path.clone());
                }
            }
        }

        if let Some(severity) = severity {
            impacted.push(ImpactedConsumer {
                consumer: manifest.id.clone(),
                owner: manifest.owner.clone(),
                severity,
                fields,
                changes,
            });
        }
    }

    // Worst first, then by name, so CI logs lead with what blocks the merge.
    impacted.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.consumer.cmp(&b.consumer))
    });

    let unaffected: Vec<String> = manifests
        .iter()
        .filter(|m| consumes_this(m) && !impacted.iter().any(|i| i.consumer == m.id))
        .map(|m| m.id.clone())
        .collect();

    report.consumer_impact = Some(ConsumerImpactReport {
        contract: old.id.clone(),
        manifests: manifests.len(),
        consumers_of_contract: manifests.iter().filter(|m| consumes_this(m)).count(),
        impacted,
        unaffected,
    });
}

/// Restrict a contract to what one consumer reads of one model — the basis
/// of `covenant check --as-consumer`: constraints on undeclared fields are
/// dropped and strict mode is switched off (undeclared fields in the data
/// are the producer's concern, not this consumer's).
///
/// The manifest must be consistent with the contract first
/// ([`ConsumerManifest::verify_against`] returns no errors) — scoping to a
/// stale declaration would validate fields that no longer exist. Callers
/// get a hard error, pointing at `covenant consumer-check`, otherwise.
pub fn scope_contract_to_consumer(
    doc: &Contract,
    model_name: &str,
    manifest: &ConsumerManifest,
    manifest_origin: &str,
) -> Result<Contract> {
    let stale = |details: String| CovenantError::ManifestInvalid {
        path: manifest_origin.to_string(),
        message: details,
    };

    let errors: Vec<String> = manifest
        .verify_against(doc)
        .into_iter()
        .filter(|f| f.level == LintLevel::Error)
        .map(|f| format!("{}: {}", f.path, f.message))
        .collect();
    if !errors.is_empty() {
        return Err(stale(format!(
            "manifest is inconsistent with contract {:?} — run `covenant consumer-check` \
             ({})",
            doc.id,
            errors.join("; "),
        )));
    }

    // The blocks this consumer holds on (contract, model).
    let blocks: Vec<&Consumption> = manifest
        .consumes
        .iter()
        .filter(|c| c.contract == doc.id && c.model.as_deref().is_none_or(|m| m == model_name))
        .collect();
    if blocks.is_empty() {
        return Err(stale(format!(
            "consumer {:?} declares nothing for model {:?} of contract {:?} — \
             nothing to check as this consumer",
            manifest.id, model_name, doc.id,
        )));
    }

    let mut scoped = doc.clone();
    // This is a pub API — a bad model name is an error, never a panic
    // (a serve-style caller could pass a request-supplied name).
    let Some(model) = scoped.models.get_mut(model_name) else {
        return Err(CovenantError::ModelNotFound {
            contract_id: doc.id.clone(),
            model: model_name.to_string(),
            available: doc.models.keys().cloned().collect::<Vec<_>>().join(", "),
        });
    };
    // Undeclared fields appearing in the data are not this consumer's
    // problem — never fail strict-mode on the producer's behalf.
    model.strict = false;
    // Any whole-model block (`fields` omitted) keeps every field; otherwise
    // keep the union of the declared field lists.
    if !blocks.iter().any(|c| c.fields.is_none()) {
        let declared: Vec<&String> = blocks
            .iter()
            .flat_map(|c| c.fields.as_ref().expect("whole-model blocks handled above"))
            .collect();
        model.fields.retain(|name, _| declared.contains(&name));
    }
    // A model-omitted block can hold fields that live entirely in ANOTHER
    // model (verify_against accepts any-model fields) — scoping THIS model
    // to it would retain nothing and blame the contract for having an
    // empty model. Same diagnosis as declaring nothing at all.
    if model.fields.is_empty() {
        return Err(stale(format!(
            "consumer {:?} declares no fields of model {:?} of contract {:?} — \
             nothing to check as this consumer",
            manifest.id, model_name, doc.id,
        )));
    }
    Ok(scoped)
}
