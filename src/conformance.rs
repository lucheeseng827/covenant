//! Conformance vectors: engine-neutral test cases pinning what the ODCS
//! standard's rules mean. Each is one small contract, a handful of records,
//! the verdict the standard implies, and — where attribution is not up to
//! the engine — which records fail. Formats spread on vectors, not prose:
//! any engine can run these, and `covenant conformance` runs them here.
//!
//! A vector pins only what the standard's text settles. Where the text
//! allows two readings, the case is left out and written up as a question
//! (`docs/CONFORMANCE.md`) rather than decided by whichever engine got there
//! first.
//!
//! An engine that cannot express a vector's rules reports it unsupported,
//! which is not a failure: Covenant does so exactly when its ODCS reader
//! lists the contract's rules as unenforced.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compile::CompiledContract;
use crate::engine::{row as row_engine, UniqueTracker};
use crate::error::{CovenantError, Result};
use crate::spec::{Contract, UnenforcedRule};

/// The format revision every vector carries in its `vector:` key.
pub const VECTOR_FORMAT: u32 = 1;

/// One conformance vector.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vector {
    pub vector: u32,
    /// Stable name, the vector's path in the suite without its extension.
    pub id: String,
    /// The revision of the standard whose text the vector pins.
    pub standard: String,
    /// Where in the standard.
    pub cites: String,
    pub description: String,
    #[serde(default)]
    pub note: Option<String>,
    /// A complete ODCS document with one schema object.
    pub contract: String,
    /// The data, as JSON objects; a missing key is a record without the property.
    pub records: Vec<Value>,
    pub expect: Expect,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub verdict: Verdict,
    /// Records (0-based) an engine must report, when that is not the
    /// engine's choice. Absent for rules like uniqueness, where engines
    /// legitimately name different records for the same duplicate.
    #[serde(default)]
    pub failing: Option<Vec<u64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Fail,
}

/// How a vector went for this engine.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail {
        expected: Verdict,
        got: Verdict,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_failing: Option<Vec<u64>>,
        got_failing: Vec<u64>,
    },
    /// The engine cannot express these rules of the vector's contract.
    Unsupported {
        rules: Vec<UnenforcedRule>,
    },
    /// The vector itself could not be run (its contract does not load).
    Error {
        message: String,
    },
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Pass => "PASS",
            Outcome::Fail { .. } => "FAIL",
            Outcome::Unsupported { .. } => "UNSUPPORTED",
            Outcome::Error { .. } => "ERROR",
        }
    }
}

/// Run one vector through this engine.
pub fn run(v: &Vector) -> Outcome {
    let loaded = match Contract::load(&v.contract, &v.id) {
        Ok(l) => l,
        Err(e) => {
            return Outcome::Error {
                message: e.to_string(),
            }
        }
    };
    if !loaded.unenforced.is_empty() {
        return Outcome::Unsupported {
            rules: loaded.unenforced,
        };
    }
    let compiled = match CompiledContract::compile(&loaded.contract) {
        Ok(c) => c,
        Err(e) => {
            return Outcome::Error {
                message: e.to_string(),
            }
        }
    };
    let model = match compiled.resolve_model(None) {
        Ok(m) => m,
        Err(e) => {
            return Outcome::Error {
                message: e.to_string(),
            }
        }
    };
    let mut unique = UniqueTracker::new(model);
    let mut scratch = Vec::new();
    let mut failing = BTreeSet::new();
    for (i, record) in v.records.iter().enumerate() {
        scratch.clear();
        if !row_engine::validate_record(model, record, i as u64, Some(&mut unique), &mut scratch) {
            failing.insert(i as u64);
        }
    }
    let violations = failing.len() as u64;
    let got = if violations > compiled.policy.max_violations {
        Verdict::Fail
    } else {
        Verdict::Pass
    };
    let got_failing: Vec<u64> = failing.into_iter().collect();
    let failing_ok = v.expect.failing.as_ref().is_none_or(|expected| {
        let mut e = expected.clone();
        e.sort_unstable();
        e.dedup();
        e == got_failing
    });
    if got == v.expect.verdict && failing_ok {
        Outcome::Pass
    } else {
        Outcome::Fail {
            expected: v.expect.verdict,
            got,
            expected_failing: v.expect.failing.clone(),
            got_failing,
        }
    }
}

/// A vector file, read and checked.
pub fn load(path: &Path) -> Result<Vector> {
    let text = std::fs::read_to_string(path).map_err(|e| CovenantError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let invalid = |message: String| CovenantError::DataRead {
        path: path.display().to_string(),
        message,
    };
    let v: Vector =
        serde_yaml::from_str(&text).map_err(|e| invalid(format!("not a vector: {e}")))?;
    if v.vector != VECTOR_FORMAT {
        return Err(invalid(format!(
            "vector format {} is not supported (this release reads {VECTOR_FORMAT})",
            v.vector
        )));
    }
    if let Some(i) = v.records.iter().position(|r| !r.is_object()) {
        return Err(invalid(format!("record {i} is not an object")));
    }
    if let Some(f) = &v.expect.failing {
        if let Some(i) = f.iter().find(|&&i| i as usize >= v.records.len()) {
            return Err(invalid(format!(
                "expect.failing names record {i}, which does not exist"
            )));
        }
        if !f.is_empty() && v.expect.verdict == Verdict::Pass {
            return Err(invalid(
                "a passing verdict cannot name failing records".into(),
            ));
        }
    }
    Ok(v)
}

/// Every vector under `paths` (files, or directories searched recursively
/// for `.yaml`/`.yml`), in path order.
pub fn load_all(paths: &[PathBuf]) -> Result<Vec<(PathBuf, Vector)>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        let entries = std::fs::read_dir(dir).map_err(|e| CovenantError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;
        for entry in entries {
            let path = entry
                .map_err(|e| CovenantError::Io {
                    path: dir.display().to_string(),
                    source: e,
                })?
                .path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("yaml" | "yml")
            ) {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut files)?;
        } else {
            files.push(p.clone());
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|f| load(&f).map(|v| (f, v)))
        .collect()
}

/// One vector's result, for reports.
#[derive(Debug, Clone, Serialize)]
pub struct VectorResult {
    pub id: String,
    pub path: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// A suite run.
#[derive(Debug, Clone, Serialize)]
pub struct SuiteReport {
    pub vectors: usize,
    pub pass: usize,
    pub fail: usize,
    pub unsupported: usize,
    pub error: usize,
    pub results: Vec<VectorResult>,
}

impl SuiteReport {
    pub fn run(vectors: &[(PathBuf, Vector)]) -> SuiteReport {
        let results: Vec<VectorResult> = vectors
            .iter()
            .map(|(path, v)| VectorResult {
                id: v.id.clone(),
                path: path.display().to_string(),
                outcome: run(v),
            })
            .collect();
        let count = |label: &str| {
            results
                .iter()
                .filter(|r| r.outcome.label() == label)
                .count()
        };
        SuiteReport {
            vectors: results.len(),
            pass: count("PASS"),
            fail: count("FAIL"),
            unsupported: count("UNSUPPORTED"),
            error: count("ERROR"),
            results,
        }
    }

    /// Whether every vector this engine supports came out as the standard says.
    pub fn conforms(&self) -> bool {
        self.fail == 0 && self.error == 0
    }

    pub fn render_human(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let list = |v: &[u64]| {
            format!(
                "[{}]",
                v.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
            )
        };
        let verdict = |v: Verdict| match v {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
        };
        for r in &self.results {
            let detail = match &r.outcome {
                Outcome::Pass => String::new(),
                Outcome::Fail {
                    expected,
                    got,
                    expected_failing,
                    got_failing,
                } => format!(
                    " — expected {}{}, got {} with records {}",
                    verdict(*expected),
                    expected_failing
                        .as_ref()
                        .map(|f| format!(" with records {}", list(f)))
                        .unwrap_or_default(),
                    verdict(*got),
                    list(got_failing)
                ),
                Outcome::Unsupported { rules } => format!(
                    " — {}",
                    rules
                        .iter()
                        .map(|u| format!("{} ({})", u.path, u.rule))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
                Outcome::Error { message } => format!(" — {message}"),
            };
            let _ = writeln!(s, "{:<12} {}{detail}", r.outcome.label(), r.id);
        }
        let _ = writeln!(
            s,
            "{} vectors: {} pass, {} fail, {} unsupported{}",
            self.vectors,
            self.pass,
            self.fail,
            self.unsupported,
            if self.error > 0 {
                format!(", {} error", self.error)
            } else {
                String::new()
            }
        );
        s
    }
}
