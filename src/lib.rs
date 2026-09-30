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
//!   through `kcat` and it is a Kafka SMT, or build the `kafka` feature and
//!   it consumes and produces topics itself. [`gate::RecordGate`] is the
//!   same judgement one record at a time, for embedding in a stream
//!   processor.
//! - **Arrow pipelines**: [`engine::arrow::validate_source_batch`] validates
//!   a source's `RecordBatch`es in-process for embedding into Arrow-native
//!   systems ([`engine::arrow::validate_batch`] for a single batch).
//!
//! Embedders start from [`prelude`]: it is the stable surface, and the only
//! one the change policy (`docs/API.md`) covers.
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
pub mod conformance;
pub mod consumers;
pub mod diff;
pub mod drift;
pub mod engine;
pub mod error;
pub mod gate;
pub mod infer;
#[cfg(feature = "kafka")]
pub mod kafka;
pub mod lineage;
pub mod mcp;
pub mod odcs;
pub mod postgres;
pub mod prelude;
pub mod profile;
pub mod protocol;
pub mod report;
pub mod send;
#[cfg(feature = "serve")]
pub mod serve;
pub mod sketch;
pub mod sources;
pub mod spec;

pub use compile::CompiledContract;
pub use error::{CovenantError, Result};
pub use report::CheckReport;
pub use spec::Contract;
