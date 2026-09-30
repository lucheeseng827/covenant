//! The stable engine API, in one import.
//!
//! What this module exports is what the change policy in `docs/API.md`
//! covers: load and compile a contract, check records, batches and files,
//! gate a stream, classify a contract change, profile data and compare
//! profiles. Everything reachable only through other paths — the CLI, the
//! console server, the MCP server, the ODCS reader's and the sketches'
//! internals — is internal and may change in any release.
//!
//! ```no_run
//! use covenant::prelude::*;
//!
//! # fn main() -> Result<()> {
//! let contract = CompiledContract::compile(&Contract::from_path("orders.yaml".as_ref())?)?;
//! let model = contract.resolve_model(None)?;
//!
//! // Check a file, and keep a profile of what it held.
//! let mut profiler = Profiler::new(&contract, model);
//! let report = check_path_profiled(&contract, model, "orders.parquet".as_ref(), None, Some(&mut profiler))?;
//! assert!(report.passed(contract.policy.max_violations), "{}", report.render_human(0));
//!
//! // Has the data drifted from last week's?
//! let baseline = Profile::from_path("baseline.json".as_ref())?;
//! let drifted = drift(&baseline, &profiler.finish(), &DriftThresholds::default())?;
//! println!("{}", drifted.render_human());
//! # Ok(())
//! # }
//! ```

pub use crate::compile::{CompiledContract, CompiledModel};
pub use crate::diff::{diff, Change, DiffReport, Impact, Severity};
pub use crate::drift::{drift, DriftReport, DriftThresholds, Finding, Metric, NotCompared};
#[cfg(feature = "arrow")]
pub use crate::engine::arrow::{validate_batch, validate_source_batch, SchemaFindings};
pub use crate::engine::row::validate_record;
pub use crate::engine::UniqueTracker;
pub use crate::error::{CovenantError, Result};
pub use crate::gate::{run as gate, DlqEnvelope, GateOutcome, GateStats, RecordGate, Verdict};
pub use crate::profile::{Profile, ProfileSummary, Profiler};
pub use crate::report::{CheckReport, Collector, ReportHeader, Rule, RuleCount, Violation};
pub use crate::sources::{check_path, check_path_profiled, DataFormat};
pub use crate::spec::{
    Contract, LintFinding, LintLevel, LoadedContract, SourceFormat, UnenforcedRule,
};
