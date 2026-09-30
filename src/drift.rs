//! Drift between two profiles of one contract model: what changed in the
//! data while the contract stayed put. `check` answers "does this data keep
//! its promises?"; drift answers "is it still the data we used to get?" —
//! the monitoring that falls out of enforcement, because the profiles were
//! collected at the enforcement point anyway.
//!
//! Every finding is one number against one threshold, with a sentence a
//! person can act on. Nothing is learned or trained:
//!
//! - **volume** — rows per run, relative change;
//! - **null rate**, **missing rate**, **invalid rate** — change in points;
//! - **distinct** — for a field whose values were (nearly) all distinct, a
//!   falling share: values started repeating. For a low-cardinality field,
//!   the count, once both sides read ten values per distinct value — below
//!   that, a smaller sample simply shows fewer values. In between, the share
//!   depends on how many rows were read, so it is not judged;
//! - **distribution** — for numbers, the population stability index (PSI)
//!   over the baseline's deciles; for booleans and `allowed:` fields, the
//!   Jensen–Shannon distance between the value shares.
//!
//! Distribution and distinct comparisons need [`MIN_VALUES`] values on both
//! sides; with fewer, the field is listed as not compared rather than judged
//! on noise.

use serde::Serialize;

use crate::error::{CovenantError, Result};
use crate::profile::{FieldProfile, Profile};

/// Values a side needs before its distribution or distinct count is judged.
pub const MIN_VALUES: u64 = 100;

/// Below this many distinct values a field's distinct *count* is compared.
const LOW_CARDINALITY: f64 = 1_000.0;

/// At or above this distinct share, a field counts as (nearly) unique.
const UNIQUE_SHARE: f64 = 0.9;

/// Values per distinct value a side needs before its distinct count says
/// more about the data than about the sample size.
const VALUES_PER_DISTINCT: f64 = 10.0;

/// Floor for a bin's share in PSI, so an empty bin scores large, not infinite.
const PSI_FLOOR: f64 = 1e-4;

/// How much change each metric tolerates before it is a finding.
#[derive(Debug, Clone, Copy)]
pub struct DriftThresholds {
    /// Change in a null, missing or invalid rate, in points (0.05 = 5 points).
    pub rate: f64,
    /// Population stability index for numeric fields.
    pub psi: f64,
    /// Jensen–Shannon distance (0–1) for categorical fields.
    pub js: f64,
    /// Change in the distinct share (points), or relative change in the
    /// distinct count for low-cardinality fields.
    pub distinct: f64,
    /// Relative change in rows per run.
    pub volume: f64,
}

impl Default for DriftThresholds {
    fn default() -> Self {
        DriftThresholds {
            rate: 0.05,
            psi: 0.25,
            js: 0.1,
            distinct: 0.1,
            volume: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// New metrics arrive in minor releases.
#[non_exhaustive]
pub enum Metric {
    Volume,
    NullRate,
    MissingRate,
    InvalidRate,
    Distinct,
    Distribution,
}

/// One change past its threshold.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// `None` for the whole model (volume).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub metric: Metric,
    /// The measured statistic on each side (rates, shares, counts), or the
    /// median for a numeric distribution.
    pub baseline: f64,
    pub current: f64,
    /// What was compared against the threshold: a difference, a PSI, a
    /// distance.
    pub score: f64,
    pub threshold: f64,
    pub message: String,
}

/// A field, or one of its metrics, that could not be compared.
#[derive(Debug, Clone, Serialize)]
pub struct NotCompared {
    pub field: String,
    pub reason: String,
}

/// What side of the comparison a profile was.
#[derive(Debug, Clone, Serialize)]
pub struct Side {
    pub contract_version: String,
    pub runs: u64,
    pub rows: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    pub contract: String,
    pub model: String,
    pub baseline: Side,
    pub current: Side,
    /// Fields compared on at least one metric.
    pub compared: Vec<String>,
    pub findings: Vec<Finding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_compared: Vec<NotCompared>,
}

impl DriftReport {
    pub fn drifted(&self) -> bool {
        !self.findings.is_empty()
    }

    pub fn render_human(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let side = |x: &Side| {
            format!(
                "v{}, {} run{}, {} row{}",
                x.contract_version,
                x.runs,
                if x.runs == 1 { "" } else { "s" },
                group(x.rows),
                if x.rows == 1 { "" } else { "s" }
            )
        };
        let _ = writeln!(
            s,
            "{} {}/{} — baseline {} · current {}",
            if self.drifted() { "DRIFT" } else { "STABLE" },
            self.contract,
            self.model,
            side(&self.baseline),
            side(&self.current)
        );
        let width = self
            .findings
            .iter()
            .map(|f| f.field.as_deref().unwrap_or("(model)").len())
            .max()
            .unwrap_or(0);
        for f in &self.findings {
            let _ = writeln!(
                s,
                "  {:<width$}  {}",
                f.field.as_deref().unwrap_or("(model)"),
                f.message
            );
        }
        let _ = writeln!(
            s,
            "  {} finding{} over {} field{}",
            self.findings.len(),
            if self.findings.len() == 1 { "" } else { "s" },
            self.compared.len(),
            if self.compared.len() == 1 { "" } else { "s" }
        );
        for n in &self.not_compared {
            let _ = writeln!(s, "  not compared: {} ({})", n.field, n.reason);
        }
        s
    }
}

/// Compare `current` against `baseline`. Both must profile the same contract
/// model; contract versions may differ.
pub fn drift(baseline: &Profile, current: &Profile, t: &DriftThresholds) -> Result<DriftReport> {
    if baseline.contract != current.contract || baseline.model != current.model {
        return Err(CovenantError::ProfileInvalid {
            path: format!("{}/{}", current.contract, current.model),
            message: format!(
                "cannot compare with a baseline of {}/{}: drift compares profiles of one contract model",
                baseline.contract, baseline.model
            ),
        });
    }
    let mut findings = Vec::new();
    let mut compared = Vec::new();
    let mut not_compared = Vec::new();

    // Volume: rows per run, so a baseline merged from many runs compares
    // with a single run.
    let per_run = |p: &Profile| p.rows as f64 / p.runs.max(1) as f64;
    let (b, c) = (per_run(baseline), per_run(current));
    if b > 0.0 {
        let change = (c - b) / b;
        if change.abs() > t.volume {
            findings.push(Finding {
                field: None,
                metric: Metric::Volume,
                baseline: b,
                current: c,
                score: change,
                threshold: t.volume,
                message: format!(
                    "rows per run {} from {} to {} ({}; threshold ±{})",
                    if change < 0.0 { "fell" } else { "rose" },
                    group(b.round() as u64),
                    group(c.round() as u64),
                    signed_pct(change),
                    pct(t.volume)
                ),
            });
        }
    }

    for (name, cur) in &current.fields {
        let Some(base) = baseline.fields.get(name) else {
            not_compared.push(NotCompared {
                field: name.clone(),
                reason: "not in the baseline".into(),
            });
            continue;
        };
        if base.ty != cur.ty {
            not_compared.push(NotCompared {
                field: name.clone(),
                reason: format!("type changed from {} to {}", base.ty.name(), cur.ty.name()),
            });
            continue;
        }
        if base.rows == 0 || cur.rows == 0 {
            not_compared.push(NotCompared {
                field: name.clone(),
                reason: "no rows on one side".into(),
            });
            continue;
        }
        compared.push(name.clone());
        field_findings(name, base, cur, t, &mut findings, &mut not_compared);
    }
    for name in baseline.fields.keys() {
        if !current.fields.contains_key(name) {
            not_compared.push(NotCompared {
                field: name.clone(),
                reason: "not in the current profile".into(),
            });
        }
    }

    Ok(DriftReport {
        contract: current.contract.clone(),
        model: current.model.clone(),
        baseline: Side {
            contract_version: baseline.contract_version.clone(),
            runs: baseline.runs,
            rows: baseline.rows,
        },
        current: Side {
            contract_version: current.contract_version.clone(),
            runs: current.runs,
            rows: current.rows,
        },
        compared,
        findings,
        not_compared,
    })
}

fn field_findings(
    name: &str,
    base: &FieldProfile,
    cur: &FieldProfile,
    t: &DriftThresholds,
    findings: &mut Vec<Finding>,
    not_compared: &mut Vec<NotCompared>,
) {
    let rate = |part: u64, whole: u64| part as f64 / whole as f64;
    let not_of_type = format!("share of values that are not {}", cur.ty.name());
    let rates = [
        (
            Metric::NullRate,
            "null rate",
            rate(base.nulls, base.rows),
            rate(cur.nulls, cur.rows),
        ),
        (
            Metric::MissingRate,
            "missing rate",
            rate(base.rows - base.present, base.rows),
            rate(cur.rows - cur.present, cur.rows),
        ),
        (
            Metric::InvalidRate,
            not_of_type.as_str(),
            rate(base.invalid, base.rows),
            rate(cur.invalid, cur.rows),
        ),
    ];
    for (metric, what, b, c) in rates {
        let change = c - b;
        if change.abs() > t.rate {
            findings.push(Finding {
                field: Some(name.to_string()),
                metric,
                baseline: b,
                current: c,
                score: change,
                threshold: t.rate,
                message: format!(
                    "{what} {} from {} to {} (threshold ±{} points)",
                    if change < 0.0 { "fell" } else { "rose" },
                    pct(b),
                    pct(c),
                    points(t.rate)
                ),
            });
        }
    }

    let (bv, cv) = (base.valid(), cur.valid());
    if bv < MIN_VALUES || cv < MIN_VALUES {
        not_compared.push(NotCompared {
            field: name.to_string(),
            reason: format!(
                "{bv} and {cv} values: distribution and distinct values need {MIN_VALUES} on each side"
            ),
        });
        return;
    }

    // Categorical: Jensen–Shannon distance between the value shares.
    if let (Some(bc), Some(cc)) = (&base.values, &cur.values) {
        let keys: std::collections::BTreeSet<&String> =
            bc.counts.keys().chain(cc.counts.keys()).collect();
        let share = |counts: &std::collections::BTreeMap<String, u64>, other: u64| {
            let total = (counts.values().sum::<u64>() + other).max(1) as f64;
            let mut v: Vec<f64> = keys
                .iter()
                .map(|k| *counts.get(*k).unwrap_or(&0) as f64 / total)
                .collect();
            v.push(other as f64 / total);
            v
        };
        let (p, q) = (share(&bc.counts, bc.other), share(&cc.counts, cc.other));
        let distance = js_distance(&p, &q);
        if distance > t.js {
            // Name the value whose share moved most.
            let (i, _) = p
                .iter()
                .zip(&q)
                .map(|(a, b)| (b - a).abs())
                .enumerate()
                .fold(
                    (0, -1.0),
                    |best, (i, d)| if d > best.1 { (i, d) } else { best },
                );
            let label = keys
                .iter()
                .nth(i)
                .map(|k| format!("{k:?}"))
                .unwrap_or_else(|| "values outside the allowed set".into());
            findings.push(Finding {
                field: Some(name.to_string()),
                metric: Metric::Distribution,
                baseline: p[i],
                current: q[i],
                score: distance,
                threshold: t.js,
                message: format!(
                    "{label} {} from {} to {} of values (Jensen–Shannon distance {:.2}; threshold {})",
                    if q[i] < p[i] { "fell" } else { "rose" },
                    pct(p[i]),
                    pct(q[i]),
                    distance,
                    t.js
                ),
            });
        }
        return;
    }

    // Numeric: PSI over the baseline's deciles.
    if let (Some(bn), Some(cn)) = (&base.numbers, &cur.numbers) {
        if bn.count >= MIN_VALUES && cn.count >= MIN_VALUES {
            let mut edges: Vec<f64> = (1..10)
                .filter_map(|d| bn.quantiles.quantile(f64::from(d) / 10.0))
                .collect();
            edges.dedup();
            let masses = |q: &crate::sketch::Quantiles| {
                let mut below = 0.0;
                let mut out = Vec::with_capacity(edges.len() + 1);
                for &e in &edges {
                    let c = q.cdf(e);
                    out.push(c - below);
                    below = c;
                }
                out.push(1.0 - below);
                out
            };
            let (p, q) = (masses(&bn.quantiles), masses(&cn.quantiles));
            let psi: f64 = p
                .iter()
                .zip(&q)
                .map(|(&a, &b)| {
                    let (a, b) = (a.max(PSI_FLOOR), b.max(PSI_FLOOR));
                    (b - a) * (b / a).ln()
                })
                .sum();
            if psi > t.psi {
                let median = |q: &crate::sketch::Quantiles| q.quantile(0.5).unwrap_or(0.0);
                let (bm, cm) = (median(&bn.quantiles), median(&cn.quantiles));
                findings.push(Finding {
                    field: Some(name.to_string()),
                    metric: Metric::Distribution,
                    baseline: bm,
                    current: cm,
                    score: psi,
                    threshold: t.psi,
                    message: format!(
                        "values shifted: median {} → {}, p95 {} → {} (PSI {:.2} over the baseline's deciles; threshold {})",
                        num(bm),
                        num(cm),
                        num(bn.quantiles.quantile(0.95).unwrap_or(bm)),
                        num(cn.quantiles.quantile(0.95).unwrap_or(cm)),
                        psi,
                        t.psi
                    ),
                });
            }
        }
    }

    // Distinct values, where both sides kept a distinct sketch.
    if let (Some(bh), Some(ch)) = (&base.distinct, &cur.distinct) {
        let (bd, cd) = (bh.estimate().round(), ch.estimate().round());
        let (bs, cs) = ((bd / bv as f64).min(1.0), (cd / cv as f64).min(1.0));
        if bs >= UNIQUE_SHARE {
            // A (nearly) unique field keeps its share at any sample size, so
            // a falling share means values started repeating.
            let change = cs - bs;
            if -change > t.distinct {
                findings.push(Finding {
                    field: Some(name.to_string()),
                    metric: Metric::Distinct,
                    baseline: bs,
                    current: cs,
                    score: change,
                    threshold: t.distinct,
                    message: format!(
                        "distinct values went from {} to {} of values — values started repeating \
                         (threshold ±{} points)",
                        pct(bs),
                        pct(cs),
                        points(t.distinct)
                    ),
                });
            }
        } else if bd < LOW_CARDINALITY && (bv.min(cv) as f64) >= VALUES_PER_DISTINCT * bd.max(cd) {
            let change = (cd - bd) / bd.max(1.0);
            if change.abs() > t.distinct && (cd - bd).abs() >= 2.0 {
                findings.push(Finding {
                    field: Some(name.to_string()),
                    metric: Metric::Distinct,
                    baseline: bd,
                    current: cd,
                    score: change,
                    threshold: t.distinct,
                    message: format!(
                        "distinct values {} from {} to {} ({}; threshold ±{})",
                        if change < 0.0 { "fell" } else { "rose" },
                        group(bd.round() as u64),
                        group(cd.round() as u64),
                        signed_pct(change),
                        pct(t.distinct)
                    ),
                });
            }
        }
    }
}

/// Jensen–Shannon distance (base 2, so 0 = same shares, 1 = disjoint).
fn js_distance(p: &[f64], q: &[f64]) -> f64 {
    let kl = |a: &[f64], m: &[f64]| -> f64 {
        a.iter()
            .zip(m)
            .filter(|(&x, _)| x > 0.0)
            .map(|(&x, &y)| x * (x / y).log2())
            .sum()
    };
    let m: Vec<f64> = p.iter().zip(q).map(|(a, b)| (a + b) / 2.0).collect();
    (0.5 * kl(p, &m) + 0.5 * kl(q, &m)).max(0.0).sqrt()
}

fn pct(r: f64) -> String {
    let s = format!("{:.1}", r * 100.0);
    format!("{}%", s.trim_end_matches(".0"))
}

fn signed_pct(r: f64) -> String {
    let s = pct(r.abs());
    if r < 0.0 {
        format!("−{s}")
    } else {
        format!("+{s}")
    }
}

fn points(r: f64) -> String {
    let s = format!("{:.1}", r * 100.0);
    s.trim_end_matches(".0").to_string()
}

fn num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        let g = group(x.abs() as u64);
        if x < 0.0 {
            format!("-{g}")
        } else {
            g
        }
    } else {
        let digits = (4 - x.abs().log10().floor() as i32 - 1).clamp(0, 6) as usize;
        let s = format!("{x:.digits$}");
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_distance_spans_zero_to_one() {
        assert!(js_distance(&[0.5, 0.5], &[0.5, 0.5]).abs() < 1e-12);
        assert!((js_distance(&[1.0, 0.0], &[0.0, 1.0]) - 1.0).abs() < 1e-12);
        let d = js_distance(&[0.6, 0.4], &[0.2, 0.8]);
        assert!(d > 0.2 && d < 0.5, "{d}");
    }
}
