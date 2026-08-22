use thiserror::Error;

/// Errors that stop a run outright (exit code 2 territory), as opposed to
/// contract *violations*, which are data findings reported through
/// [`crate::report`] (exit code 1 territory).
#[derive(Debug, Error)]
pub enum CovenantError {
    // Verb-neutral on purpose: this variant wraps reads AND writes (DLQ,
    // init, gate output) — "failed to read" would send a user with a
    // write-permission problem hunting the wrong bug.
    #[error("I/O error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("contract parse error in {path}: {message}")]
    ContractParse { path: String, message: String },

    /// The contract parsed but failed its own lint (`covenant validate` findings
    /// promoted to hard errors because enforcement against a broken contract
    /// would report nonsense).
    #[error("contract {id} is invalid: {details}")]
    ContractInvalid { id: String, details: String },

    #[error("model {model:?} not found in contract {contract_id:?} (available: {available})")]
    ModelNotFound {
        contract_id: String,
        model: String,
        available: String,
    },

    #[error(
        "contract {contract_id:?} has {count} models; pass --model to pick one (available: {available})"
    )]
    ModelAmbiguous {
        contract_id: String,
        count: usize,
        available: String,
    },

    /// A consumer manifest failed to parse or validate. Hard error rather
    /// than a skipped file: a half-loaded manifest set would report "nobody
    /// breaks" exactly when someone does.
    #[error("consumer manifest error in {path}: {message}")]
    ManifestInvalid { path: String, message: String },

    #[error("usage error: {message}")]
    Usage { message: String },

    #[error("cannot infer data format for {path}: unknown extension (expected .ndjson/.jsonl/.json, .csv, or .parquet)")]
    UnknownFormat { path: String },

    #[error("{path}: {message}")]
    DataRead { path: String, message: String },
}

pub type Result<T> = std::result::Result<T, CovenantError>;
