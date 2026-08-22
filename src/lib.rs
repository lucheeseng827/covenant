//! # Covenant — data-contract enforcement runtime
//!
//! Most data-contract tooling stops at *describing* the contract; the YAML
//! gets published and nothing ever runs the assertions where the data flows.
//! Covenant is the enforcement half: parse a contract once, compile it into
//! a hot-path validator, and run it at every boundary the data crosses —
//!
//! - **CI**: [`sources::check_path`] / `covenant check` gates NDJSON, CSV,
//!   and Parquet files; `covenant diff` blocks breaking contract changes
//!   pre-merge with producer/consumer impact classification.
//! - **Streams**: [`gate::run`] / `covenant gate` filters NDJSON inline
//!   (stdin → clean stdout, violations → dead-letter envelopes) — pipe it
//!   through `kcat` and it is a Kafka SMT.
//! - **Arrow pipelines**: [`engine::arrow::validate_batch`] validates
//!   `RecordBatch`es in-process for embedding into Arrow-native systems.
//!
//! ```no_run
//! use covenant::compile::CompiledContract;
//! use covenant::spec::Contract;
//!
//! let doc = Contract::from_path(std::path::Path::new("orders.yaml"))?;
//! let compiled = CompiledContract::compile(&doc)?;
//! let model = compiled.resolve_model(None)?;
//! let report = covenant::sources::check_path(
//!     &compiled, model, std::path::Path::new("orders.parquet"), None)?;
//! assert!(report.passed(compiled.policy.max_violations));
//! # Ok::<(), covenant::error::CovenantError>(())
//! ```

pub mod cli;
pub mod compile;
pub mod consumers;
pub mod diff;
pub mod engine;
pub mod error;
pub mod gate;
pub mod infer;
pub mod report;
#[cfg(feature = "serve")]
pub mod serve;
pub mod sources;
pub mod spec;

pub use compile::CompiledContract;
pub use error::{CovenantError, Result};
pub use report::CheckReport;
pub use spec::Contract;
