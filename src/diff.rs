//! Contract-to-contract diff with breaking-change classification — the CI
//! side of enforcement. `covenant diff old.yaml new.yaml --fail-on breaking`
//! is the pre-merge gate that stops a producer from shipping a contract
//! change its consumers never agreed to.
//!
//! Every change is classified on two axes:
//! - **severity** — `breaking` (someone's pipeline stops working), `risky`
//!   (data that used to pass may now fail, or a guarantee someone may rely
//!   on was weakened), `info` (additive/benign);
//! - **impact** — whether producers, consumers, or both feel it. Removing a
//!   field breaks consumers; requiring a new field breaks producers;
//!   changing a type breaks both.
//!
//! The classifier also enforces the semver discipline: breaking → major
//! bump, risky → minor bump, anything → some bump. An insufficient bump is
//! itself reported as a breaking finding.

use serde::Serialize;

use crate::spec::{Contract, Field, FieldType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
/// How much a contract change hurts, ordered so `max()` yields the
/// run verdict: `info < risky < breaking`.
pub enum Severity {
    Info,
    Risky,
    Breaking,
}

impl Severity {
    /// Lowercase severity name (used in reports).
    pub fn name(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Risky => "risky",
            Severity::Breaking => "breaking",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
/// Who feels a contract change: the side whose data starts failing
/// (producers), the side whose assumptions break (consumers), or both.
pub enum Impact {
    Producers,
    Consumers,
    Both,
}

impl Impact {
    /// Lowercase impact name (used in reports).
    pub fn name(self) -> &'static str {
        match self {
            Impact::Producers => "producers",
            Impact::Consumers => "consumers",
            Impact::Both => "both",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
/// One classified difference between two contract versions.
pub struct Change {
    pub severity: Severity,
    pub impact: Impact,
    /// Dotted location, e.g. `models.orders.fields.amount`.
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
/// The full classified diff between two contract versions.
pub struct DiffReport {
    pub old_version: String,
    pub new_version: String,
    pub changes: Vec<Change>,
    /// The named blast radius — present only when consumer manifests were
    /// supplied (`--consumers <dir>`), so "not checked" and "nobody breaks"
    /// stay distinguishable. See [`crate::consumers`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer_impact: Option<crate::consumers::ConsumerImpactReport>,
    /// Rules on either side that were not compared because the run allowed
    /// unenforced rules (`--allow-unenforced`). A change to one of them is
    /// invisible here, so non-empty means this is a partial diff.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unenforced: Vec<crate::spec::UnenforcedRule>,
}

impl DiffReport {
    /// The worst severity in the diff — what `--fail-on` compares against.
    pub fn max_severity(&self) -> Option<Severity> {
        self.changes.iter().map(|c| c.severity).max()
    }

    /// Human rendering for terminals and CI logs.
    pub fn render_human(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "contract diff: v{} -> v{} ({} change{}){}",
            self.old_version,
            self.new_version,
            self.changes.len(),
            if self.changes.len() == 1 { "" } else { "s" },
            if self.unenforced.is_empty() {
                ""
            } else {
                " (partial)"
            }
        );
        for c in &self.changes {
            let _ = writeln!(
                s,
                "  [{severity:<8}] ({impact:<9}) {path}: {message}",
                severity = c.severity.name(),
                impact = c.impact.name(),
                path = c.path,
                message = c.message,
            );
        }
        if self.changes.is_empty() {
            let _ = writeln!(s, "  no changes");
        }
        if !self.unenforced.is_empty() {
            let _ = writeln!(
                s,
                "  not compared (--allow-unenforced): {} contract rule(s)\n{}",
                self.unenforced.len(),
                crate::spec::render_unenforced(&self.unenforced)
                    .lines()
                    .map(|l| format!("  {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        if let Some(ci) = &self.consumer_impact {
            let _ = writeln!(
                s,
                "impacted consumers ({} manifest{}, {} consume {}):",
                ci.manifests,
                if ci.manifests == 1 { "" } else { "s" },
                ci.consumers_of_contract,
                ci.contract,
            );
            for c in &ci.impacted {
                // No named fields: a models.* hit is scoped to that model;
                // only policy/other targets are genuinely contract-wide.
                let via = if !c.fields.is_empty() {
                    format!("via {}", c.fields.join(", "))
                } else if c.changes.iter().all(|p| p.starts_with("models.")) {
                    "model-scoped".to_string()
                } else {
                    "contract-wide".to_string()
                };
                let owner = c
                    .owner
                    .as_deref()
                    .map(|o| format!(" ({o})"))
                    .unwrap_or_default();
                let _ = writeln!(
                    s,
                    "  [{severity:<8}] {id}{owner} {via} — {changes}",
                    severity = c.severity.name(),
                    id = c.consumer,
                    changes = c.changes.join(", "),
                );
            }
            if ci.impacted.is_empty() {
                let _ = writeln!(s, "  no declared consumer is affected");
            }
            if !ci.unaffected.is_empty() {
                let _ = writeln!(s, "  unaffected: {}", ci.unaffected.join(", "));
            }
        }
        s
    }
}

/// Diff two contracts (old = what consumers rely on today, new = proposed).
pub fn diff(old: &Contract, new: &Contract) -> DiffReport {
    let mut changes = Vec::new();

    // Models removed / added.
    for (name, old_model) in &old.models {
        let mpath = format!("models.{name}");
        match new.models.get(name) {
            None => changes.push(Change {
                severity: Severity::Breaking,
                impact: Impact::Consumers,
                path: mpath,
                message: "model removed — consumers reading it lose their source".into(),
            }),
            Some(new_model) => {
                diff_model(name, old_model, new_model, &mut changes);
            }
        }
    }
    for name in new.models.keys() {
        if !old.models.contains_key(name) {
            changes.push(Change {
                severity: Severity::Info,
                impact: Impact::Consumers,
                path: format!("models.{name}"),
                message: "model added".into(),
            });
        }
    }

    // Policy is load-bearing (it decides whether enforcement blocks at all),
    // so weakening it is a diffable change like any other.
    diff_policy(old, new, &mut changes);

    // Semver discipline: the version must move, and must move enough.
    enforce_version_bump(old, new, &mut changes);

    DiffReport {
        old_version: old.version.clone(),
        new_version: new.version.clone(),
        changes,
        consumer_impact: None,
        unenforced: Vec::new(),
    }
}

fn diff_policy(old: &Contract, new: &Contract, changes: &mut Vec<Change>) {
    use crate::spec::OnViolation;
    match (old.policy.on_violation, new.policy.on_violation) {
        (OnViolation::Block, OnViolation::Warn) => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Consumers,
            path: "policy.on_violation".to_string(),
            message: "enforcement weakened block -> warn — violating data now flows through".into(),
        }),
        (OnViolation::Warn, OnViolation::Block) => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Producers,
            path: "policy.on_violation".to_string(),
            message: "enforcement tightened warn -> block — violating records start being withheld"
                .into(),
        }),
        _ => {}
    }
    if old.policy.max_violations != new.policy.max_violations {
        let (impact, direction) = if new.policy.max_violations > old.policy.max_violations {
            (Impact::Consumers, "raised")
        } else {
            (Impact::Producers, "lowered")
        };
        changes.push(Change {
            severity: Severity::Risky,
            impact,
            path: "policy.max_violations".to_string(),
            message: format!(
                "violation budget {direction} {} -> {}",
                old.policy.max_violations, new.policy.max_violations
            ),
        });
    }
    // policy.sample_violations is a reporting knob, not a promise — no finding.
}

fn diff_model(
    model_name: &str,
    old: &crate::spec::Model,
    new: &crate::spec::Model,
    changes: &mut Vec<Change>,
) {
    let mpath = format!("models.{model_name}");

    if !old.strict && new.strict {
        changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Producers,
            path: mpath.clone(),
            message:
                "model became strict — undeclared fields producers already send become violations"
                    .into(),
        });
    } else if old.strict && !new.strict {
        changes.push(Change {
            severity: Severity::Info,
            impact: Impact::Consumers,
            path: mpath.clone(),
            message: "model is no longer strict — undeclared fields may appear".into(),
        });
    }

    for (fname, old_field) in &old.fields {
        let fpath = format!("{mpath}.fields.{fname}");
        match new.fields.get(fname) {
            None => changes.push(Change {
                severity: Severity::Breaking,
                impact: Impact::Consumers,
                path: fpath,
                // Deprecation is advisory, never a license to break: the
                // removal stays BREAKING, the message just credits the
                // migration window that existed.
                message: if old_field.deprecated.is_some() {
                    "field removed — consumers reading it break (was deprecated)".into()
                } else {
                    "field removed — consumers reading it break".into()
                },
            }),
            Some(new_field) => diff_field(&fpath, old_field, new_field, changes),
        }
    }
    for (fname, new_field) in &new.fields {
        if !old.fields.contains_key(fname) {
            let fpath = format!("{mpath}.fields.{fname}");
            if new_field.required {
                changes.push(Change {
                    severity: Severity::Breaking,
                    impact: Impact::Producers,
                    path: fpath,
                    message: "required field added — existing producers don't send it".into(),
                });
            } else {
                changes.push(Change {
                    severity: Severity::Info,
                    impact: Impact::Consumers,
                    path: fpath,
                    message: "optional field added".into(),
                });
            }
        }
    }
}

fn diff_field(fpath: &str, old: &Field, new: &Field, changes: &mut Vec<Change>) {
    // Type change.
    if old.ty != new.ty {
        // Integer → float is representational widening: every conforming
        // producer value still passes, but consumers' decoders change.
        let (severity, impact, extra) =
            if old.ty == FieldType::Integer && new.ty == FieldType::Float {
                (
                    Severity::Risky,
                    Impact::Consumers,
                    " (integer→float widening)",
                )
            } else {
                (Severity::Breaking, Impact::Both, "")
            };
        changes.push(Change {
            severity,
            impact,
            path: fpath.to_string(),
            message: format!("type changed {} -> {}{extra}", old.ty.name(), new.ty.name()),
        });
    }

    // Deprecation transitions — advisory, so info-level, but consumer-facing:
    // a newly deprecated field is the migration starting gun.
    if old.deprecated.is_none() && new.deprecated.is_some() {
        let note = new.deprecated.as_deref().unwrap_or_default();
        changes.push(Change {
            severity: Severity::Info,
            impact: Impact::Consumers,
            path: fpath.to_string(),
            message: if note.is_empty() {
                "field marked deprecated — consumers should plan migration".into()
            } else {
                format!("field marked deprecated ({note}) — consumers should plan migration")
            },
        });
    } else if old.deprecated.is_some() && new.deprecated.is_none() {
        changes.push(Change {
            severity: Severity::Info,
            impact: Impact::Consumers,
            path: fpath.to_string(),
            message: "field is no longer deprecated".into(),
        });
    } else if let (Some(o), Some(n)) = (&old.deprecated, &new.deprecated) {
        // The guidance itself changed — readers planning a migration
        // against the old note need to hear the new one.
        if o != n {
            changes.push(Change {
                severity: Severity::Info,
                impact: Impact::Consumers,
                path: fpath.to_string(),
                message: format!("deprecation note changed ({o:?} -> {n:?})"),
            });
        }
    }

    // Presence / nullness.
    if !old.required && new.required {
        changes.push(Change {
            severity: Severity::Breaking,
            impact: Impact::Producers,
            path: fpath.to_string(),
            message: "field became required — producers omitting it start failing".into(),
        });
    } else if old.required && !new.required {
        changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Consumers,
            path: fpath.to_string(),
            message: "field is no longer required — consumers assuming presence may break".into(),
        });
    }
    if !old.nullable && new.nullable {
        changes.push(Change {
            severity: Severity::Breaking,
            impact: Impact::Consumers,
            path: fpath.to_string(),
            message: "field became nullable — consumers assuming non-null may break".into(),
        });
    } else if old.nullable && !new.nullable {
        changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Producers,
            path: fpath.to_string(),
            message: "field is no longer nullable — nulls producers send start failing".into(),
        });
    }

    // Constraint tightening hits producers (their data starts failing);
    // loosening hits consumers (a guarantee they may rely on is gone).
    constraint_change(
        fpath,
        "pattern",
        old.pattern.as_deref(),
        new.pattern.as_deref(),
        changes,
    );
    numeric_bound_change(fpath, "min", old.min, new.min, true, changes);
    numeric_bound_change(fpath, "max", old.max, new.max, false, changes);
    length_bound_change(
        fpath,
        "min_length",
        old.min_length,
        new.min_length,
        true,
        changes,
    );
    length_bound_change(
        fpath,
        "max_length",
        old.max_length,
        new.max_length,
        false,
        changes,
    );
    allowed_change(fpath, old, new, changes);
    format_change(fpath, old, new, changes);

    if !old.unique && new.unique {
        changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Producers,
            path: fpath.to_string(),
            message: "field became unique — duplicate values producers send start failing".into(),
        });
    } else if old.unique && !new.unique {
        changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Consumers,
            path: fpath.to_string(),
            message: "uniqueness dropped — consumers deduplicating by this field may break".into(),
        });
    }
}

/// Pattern is opaque (regex inclusion is undecidable) — any change is
/// classified conservatively: added/changed hits producers, removed hits
/// consumers.
fn constraint_change(
    fpath: &str,
    name: &str,
    old: Option<&str>,
    new: Option<&str>,
    changes: &mut Vec<Change>,
) {
    match (old, new) {
        (None, Some(n)) => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Producers,
            path: fpath.to_string(),
            message: format!("{name} constraint added ({n:?}) — existing values may not match"),
        }),
        (Some(_), None) => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Consumers,
            path: fpath.to_string(),
            message: format!("{name} constraint removed — the shape guarantee is gone"),
        }),
        (Some(o), Some(n)) if o != n => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Both,
            path: fpath.to_string(),
            message: format!("{name} constraint changed ({o:?} -> {n:?})"),
        }),
        _ => {}
    }
}

/// `is_lower_bound`: for min, a *raise* tightens; for max, a *lower* tightens.
fn numeric_bound_change(
    fpath: &str,
    name: &str,
    old: Option<f64>,
    new: Option<f64>,
    is_lower_bound: bool,
    changes: &mut Vec<Change>,
) {
    // NaN has no ordering; without this guard partial_cmp's None would fall
    // through to Equal and mislabel the change as "loosened". (The linter
    // rejects non-finite bounds, but diff also runs on unlinted files.)
    if old.is_some_and(f64::is_nan) || new.is_some_and(f64::is_nan) {
        changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Both,
            path: fpath.to_string(),
            message: format!("{name} bound is not a number — the change cannot be classified"),
        });
        return;
    }
    bound_change(fpath, name, old, new, is_lower_bound, changes, |a, b| {
        a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn length_bound_change(
    fpath: &str,
    name: &str,
    old: Option<usize>,
    new: Option<usize>,
    is_lower_bound: bool,
    changes: &mut Vec<Change>,
) {
    bound_change(fpath, name, old, new, is_lower_bound, changes, |a, b| {
        a.cmp(&b)
    });
}

fn bound_change<T: Copy + PartialEq + std::fmt::Display>(
    fpath: &str,
    name: &str,
    old: Option<T>,
    new: Option<T>,
    is_lower_bound: bool,
    changes: &mut Vec<Change>,
    cmp: impl Fn(T, T) -> std::cmp::Ordering,
) {
    use std::cmp::Ordering;
    match (old, new) {
        (None, Some(n)) => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Producers,
            path: fpath.to_string(),
            message: format!("{name} constraint added ({n}) — existing values may fall outside it"),
        }),
        (Some(o), None) => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Consumers,
            path: fpath.to_string(),
            message: format!("{name} constraint removed (was {o}) — the bound guarantee is gone"),
        }),
        (Some(o), Some(n)) if o != n => {
            let tightened = match cmp(n, o) {
                Ordering::Greater => is_lower_bound,
                Ordering::Less => !is_lower_bound,
                Ordering::Equal => false,
            };
            if tightened {
                changes.push(Change {
                    severity: Severity::Risky,
                    impact: Impact::Producers,
                    path: fpath.to_string(),
                    message: format!(
                        "{name} tightened {o} -> {n} — conforming values may start failing"
                    ),
                });
            } else {
                changes.push(Change {
                    severity: Severity::Risky,
                    impact: Impact::Consumers,
                    path: fpath.to_string(),
                    message: format!(
                        "{name} loosened {o} -> {n} — consumers relying on the old bound may break"
                    ),
                });
            }
        }
        _ => {}
    }
}

fn allowed_change(fpath: &str, old: &Field, new: &Field, changes: &mut Vec<Change>) {
    match (&old.allowed, &new.allowed) {
        (None, Some(vals)) => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Producers,
            path: fpath.to_string(),
            message: format!(
                "allowed set added ({} values) — values outside it start failing",
                vals.len()
            ),
        }),
        (Some(_), None) => changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Consumers,
            path: fpath.to_string(),
            message: "allowed set removed — the closed-set guarantee is gone".into(),
        }),
        (Some(o), Some(n)) => {
            let removed: Vec<String> = o
                .iter()
                .filter(|v| !n.iter().any(|nv| allowed_value_eq(v, nv)))
                .map(|v| v.to_string())
                .collect();
            let added: Vec<String> = n
                .iter()
                .filter(|v| !o.iter().any(|ov| allowed_value_eq(v, ov)))
                .map(|v| v.to_string())
                .collect();
            if !removed.is_empty() {
                changes.push(Change {
                    severity: Severity::Risky,
                    impact: Impact::Producers,
                    path: fpath.to_string(),
                    message: format!("allowed set narrowed — removed: {}", removed.join(", ")),
                });
            }
            if !added.is_empty() {
                changes.push(Change {
                    severity: Severity::Risky,
                    impact: Impact::Consumers,
                    path: fpath.to_string(),
                    message: format!(
                        "allowed set widened — added: {} (consumers switching on values may not handle them)",
                        added.join(", ")
                    ),
                });
            }
        }
        (None, None) => {}
    }
}

/// Allowed-set membership compares numbers by value, not representation —
/// YAML `1` and `1.0` deserialize as distinct serde_json Numbers, and raw
/// Value equality would report a phantom narrow+widen for a no-op edit.
fn allowed_value_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

fn format_change(fpath: &str, old: &Field, new: &Field, changes: &mut Vec<Change>) {
    let old_fmt = old.format.map(|f| f.name());
    let new_fmt = new.format.map(|f| f.name());
    constraint_change(fpath, "format", old_fmt, new_fmt, changes);
}

fn enforce_version_bump(old: &Contract, new: &Contract, changes: &mut Vec<Change>) {
    // A version the classifier can't parse must not silently disable the
    // bump discipline — diff also runs on files nobody linted, and silence
    // here is exactly the enforcement hole the tool exists to close.
    let (old_v, new_v) = match (
        semver::Version::parse(&old.version),
        semver::Version::parse(&new.version),
    ) {
        (Ok(o), Ok(n)) => (o, n),
        _ => {
            if !changes.is_empty() {
                changes.push(Change {
                    severity: Severity::Risky,
                    impact: Impact::Both,
                    path: "version".to_string(),
                    message: format!(
                        "version {:?} -> {:?} is not semver — the bump discipline cannot be enforced",
                        old.version, new.version
                    ),
                });
            }
            return;
        }
    };

    // Iterating a prerelease of the same base version (2.0.0-alpha.1 ->
    // 2.0.0-alpha.2, or -alpha -> final) is the phase where breaking changes
    // are expected; the bump requirement is considered satisfied there.
    let prerelease_iteration = !old_v.pre.is_empty()
        && new_v > old_v
        && new_v.major == old_v.major
        && new_v.minor == old_v.minor
        && new_v.patch == old_v.patch;

    let max = changes.iter().map(|c| c.severity).max();
    let required: Option<(&str, bool)> = match max {
        Some(Severity::Breaking) => {
            Some(("major", new_v.major > old_v.major || prerelease_iteration))
        }
        Some(Severity::Risky) => Some((
            "minor",
            new_v.major > old_v.major
                || (new_v.major == old_v.major && new_v.minor > old_v.minor)
                || prerelease_iteration,
        )),
        Some(Severity::Info) => Some(("patch", new_v > old_v)),
        None => None,
    };
    if let Some((bump, satisfied)) = required {
        if !satisfied {
            changes.push(Change {
                severity: Severity::Breaking,
                impact: Impact::Both,
                path: "version".to_string(),
                message: format!(
                    "changes require at least a {bump} version bump, but version went {} -> {}",
                    old.version, new.version
                ),
            });
        }
    } else if new_v < old_v {
        changes.push(Change {
            severity: Severity::Risky,
            impact: Impact::Both,
            path: "version".to_string(),
            message: format!("version went backwards {} -> {}", old.version, new.version),
        });
    }
}
