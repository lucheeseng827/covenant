use thiserror::Error;

/// Errors that stop a run outright (exit code 2 territory), as opposed to
/// contract *violations*, which are data findings reported through
/// [`crate::report`] (exit code 1 territory).
#[derive(Debug, Error)]
// New failure kinds arrive in minor releases.
#[non_exhaustive]
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

    /// The contract carries rules this runtime cannot enforce yet (ODCS
    /// features it does not implement). Refused rather than half-enforced:
    /// a PASS against a contract whose rules were quietly skipped would
    /// certify data nobody checked.
    #[error(
        "{path}: {count} contract rule(s) cannot be enforced yet:\n{details}\n  \
         Rerun with --allow-unenforced to check everything else; the report is then marked partial."
    )]
    ContractUnenforced {
        path: String,
        count: usize,
        details: String,
    },

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

    /// A profile that cannot be read, or two that cannot be combined.
    #[error("profile error in {path}: {message}")]
    ProfileInvalid { path: String, message: String },

    /// Contract rules with no exact equivalent in an engine's own dialect
    /// (`covenant export`), refused rather than exported approximately.
    #[error(
        "{count} contract rule(s) have no exact {dialect} equivalent yet:\n{details}\n  \
         Rerun with --allow-unenforced to export everything else; the output then lists what it leaves out."
    )]
    DialectUnenforced {
        dialect: String,
        count: usize,
        details: String,
    },

    /// A contract the dialect cannot express at all, such as a name the
    /// engine would silently truncate.
    #[error("cannot export to {dialect}: {message}")]
    Dialect { dialect: String, message: String },

    /// A gate's transport failed in a way no retry mends: brokers that do
    /// not answer, a topic that does not exist, a record the brokers did not
    /// acknowledge. Nothing after the failure is committed.
    #[error("{transport}: {message}")]
    Transport { transport: String, message: String },
}

pub type Result<T> = std::result::Result<T, CovenantError>;
