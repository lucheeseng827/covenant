//! covenant-python — the engine as a Python module, `covenant_data`.
//!
//! Marshalling only: every verdict comes from the engine's own paths, so a
//! report here is the report `covenant check` gives for the same data. Records
//! go through the NDJSON reader the CLI uses; Arrow data (polars, pyarrow,
//! DuckDB — anything exporting the Arrow PyCapsule interface) through the
//! Arrow engine, batch by batch, zero-copy.
//!
//! pyo3 is optional and behind the off-by-default `extension-module` feature,
//! so a bare `cargo build` never links libpython. The wheel is built with
//! maturin (`pyproject.toml` turns the feature on) and, thanks to
//! `abi3-py39`, one wheel covers CPython 3.9+. With the feature off this is a
//! plain library exposing the same [`Loaded`] core, for native tests.

use std::io::Cursor;
use std::path::Path;

use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::ArrowError;
use covenant::compile::{CompiledContract, CompiledModel};
use covenant::engine::arrow::{validate_source_batch, SchemaFindings};
use covenant::engine::UniqueTracker;
use covenant::error::CovenantError;
use covenant::protocol::{Outcome, Producer, ReportDocument, RunContext, RunTiming, SampleMode};
use covenant::report::{CheckReport, Collector, ReportHeader};
use covenant::sources;
use covenant::spec::{Contract, LoadedContract, OnViolation, UnenforcedRule};

/// A contract read and compiled once, with the model it checks.
pub struct Loaded {
    pub compiled: CompiledContract,
    pub model: String,
    /// Rules the reader could not enforce, when the caller allowed a partial
    /// contract. Every report carries them.
    pub unenforced: Vec<UnenforcedRule>,
}

impl Loaded {
    /// Read a contract file (`covenant: 1` or ODCS), as `covenant check -c` does.
    pub fn from_path(
        path: &Path,
        model: Option<&str>,
        allow_unenforced: bool,
    ) -> Result<Self, CovenantError> {
        let origin = path.display().to_string();
        Self::new(Contract::load_path(path)?, &origin, model, allow_unenforced)
    }

    /// Read a contract from text; `origin` names it in errors.
    pub fn from_text(
        text: &str,
        origin: &str,
        model: Option<&str>,
        allow_unenforced: bool,
    ) -> Result<Self, CovenantError> {
        Self::new(
            Contract::load(text, origin)?,
            origin,
            model,
            allow_unenforced,
        )
    }

    fn new(
        loaded: LoadedContract,
        origin: &str,
        model: Option<&str>,
        allow_unenforced: bool,
    ) -> Result<Self, CovenantError> {
        let (contract, unenforced) = if allow_unenforced {
            let unenforced = loaded.unenforced.clone();
            (loaded.contract, unenforced)
        } else {
            (loaded.into_enforceable(origin)?, Vec::new())
        };
        let compiled = CompiledContract::compile(&contract)?;
        let model = compiled.resolve_model(model)?.name.clone();
        Ok(Loaded {
            compiled,
            model,
            unenforced,
        })
    }

    pub fn model(&self) -> &CompiledModel {
        &self.compiled.models[&self.model]
    }

    /// Violations the contract tolerates before a check fails.
    pub fn budget(&self) -> u64 {
        self.compiled.policy.max_violations
    }

    /// `on_violation: warn`: violations are reported and never fail a run.
    pub fn warn_only(&self) -> bool {
        self.compiled.policy.on_violation == OnViolation::Warn
    }

    /// The verdict, decided as `covenant check` decides it.
    pub fn passed(&self, report: &CheckReport) -> bool {
        self.warn_only() || report.passed(self.budget())
    }

    /// The verdict as the report document says it: pass, fail, or warn for a
    /// contract that only warns.
    pub fn outcome(&self, report: &CheckReport) -> Outcome {
        Outcome::judge(report.violations, self.budget(), self.warn_only())
    }

    /// Check a file: NDJSON, CSV or Parquet, by extension.
    pub fn check_path(&self, path: &Path) -> Result<CheckReport, CovenantError> {
        let report = sources::check_path(&self.compiled, self.model(), path, None)?;
        Ok(self.finish(report))
    }

    /// Check NDJSON text: one JSON object per line, through the same reader
    /// the CLI streams files through.
    pub fn check_ndjson(&self, ndjson: &str, source: &str) -> Result<CheckReport, CovenantError> {
        let model = self.model();
        let mut collector = Collector::new(self.compiled.policy.sample_violations);
        let mut unique = UniqueTracker::new(model);
        let rows = sources::check_ndjson_reader(
            model,
            Cursor::new(ndjson.as_bytes()),
            &mut unique,
            &mut collector,
        )?;
        Ok(self.finish(collector.into_report(self.header(source), rows)))
    }

    /// Check Arrow record batches through the Arrow engine, as the CLI checks
    /// a Parquet file: batch by batch, uniqueness across all of them, and a
    /// stream with no rows still held to its schema.
    pub fn check_batches<R: RecordBatchReader>(
        &self,
        reader: R,
        source: &str,
    ) -> Result<CheckReport, ArrowError> {
        let model = self.model();
        let schema = reader.schema();
        let mut collector = Collector::new(self.compiled.policy.sample_violations);
        let mut unique = UniqueTracker::new(model);
        let mut rows: u64 = 0;
        // One stream is one source: a schema problem counts once, however
        // many batches carry it, as it does for the command's files.
        let mut schema_findings = SchemaFindings::new();
        let mut saw_batch = false;
        for batch in reader {
            let batch = batch?;
            saw_batch = true;
            rows += validate_source_batch(
                model,
                &batch,
                rows,
                Some(&mut unique),
                &mut schema_findings,
                &mut collector,
            );
        }
        // A stream with no batch at all is still held to its schema.
        if !saw_batch {
            validate_source_batch(
                model,
                &RecordBatch::new_empty(schema),
                0,
                Some(&mut unique),
                &mut schema_findings,
                &mut collector,
            );
        }
        Ok(self.finish(collector.into_report(self.header(source), rows)))
    }

    fn header(&self, source: &str) -> ReportHeader {
        ReportHeader {
            contract_id: self.compiled.id.clone(),
            contract_version: self.compiled.version.clone(),
            owner: self.compiled.owner.clone(),
            model: self.model.clone(),
            source: source.to_string(),
        }
    }

    /// A verdict over part of a contract says which part it skipped.
    fn finish(&self, mut report: CheckReport) -> CheckReport {
        report.unenforced = self.unenforced.clone();
        report
    }
}

/// A check's `covenant-report/v1` document on the `python` plane: the one
/// `covenant check --report-json` writes, with the CI provider read from the
/// environment as the document is made.
pub fn report_document(
    report: &CheckReport,
    budget: u64,
    outcome: Outcome,
    timing: RunTiming,
    samples: &SampleMode,
) -> ReportDocument {
    ReportDocument::for_check(
        std::slice::from_ref(report),
        budget,
        outcome,
        samples,
        RunContext::capture(Producer::Python, timing),
    )
}

/// Run the `covenant` command line in this process: `argv[0]` is the program
/// name. Returns the exit code; `--help`, `--version` and usage errors print
/// as the binary prints them.
pub fn cli(argv: Vec<String>) -> i32 {
    use clap::Parser;
    use std::io::Write;
    // Parsed like the binary parses, except that `--help`, `--version` and a
    // usage error hand back their exit code instead of exiting the host
    // process.
    let code = match covenant::cli::Cli::try_parse_from(argv) {
        Ok(cli) => covenant::cli::run(cli),
        Err(e) => {
            let _ = e.print();
            e.exit_code()
        }
    };
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    code
}

#[cfg(feature = "extension-module")]
mod glue {
    use std::ffi::CStr;
    use std::path::PathBuf;

    use arrow_array::ffi::{from_ffi, FFI_ArrowArray, FFI_ArrowSchema};
    use arrow_array::ffi_stream::{ArrowArrayStreamReader, FFI_ArrowArrayStream};
    use arrow_array::{RecordBatch, RecordBatchIterator, StructArray};
    use covenant::protocol::{Outcome, RunClock, RunTiming, SampleKey, SampleMode};
    use covenant::report::CheckReport;
    use pyo3::create_exception;
    use pyo3::exceptions::{PyException, PyTypeError, PyValueError};
    use pyo3::prelude::*;
    use pyo3::types::{PyCapsule, PyDict, PyList, PyTuple};

    use super::Loaded;

    create_exception!(
        covenant_data,
        CovenantError,
        PyException,
        "A contract that cannot be read, compiled or enforced, or data that cannot be \
         read. A verdict is never an error: a failing check returns a Report."
    );

    fn err(e: impl std::fmt::Display) -> PyErr {
        CovenantError::new_err(e.to_string())
    }

    const STREAM: &CStr = c"arrow_array_stream";
    const SCHEMA: &CStr = c"arrow_schema";
    const ARRAY: &CStr = c"arrow_array";

    /// A compiled contract. Load it once and check as much data as you like.
    #[pyclass(frozen, module = "covenant_data")]
    struct Contract {
        inner: Loaded,
    }

    #[pymethods]
    impl Contract {
        /// Load a contract file: `covenant: 1` or ODCS v3.
        #[new]
        #[pyo3(signature = (path, *, model = None, allow_unenforced = false))]
        fn new(path: PathBuf, model: Option<&str>, allow_unenforced: bool) -> PyResult<Self> {
            let inner = Loaded::from_path(&path, model, allow_unenforced).map_err(err)?;
            Ok(Contract { inner })
        }

        /// Load a contract from its text; `name` stands for it in errors.
        #[staticmethod]
        #[pyo3(signature = (text, *, name = "<string>", model = None, allow_unenforced = false))]
        fn from_text(
            text: &str,
            name: &str,
            model: Option<&str>,
            allow_unenforced: bool,
        ) -> PyResult<Self> {
            let inner = Loaded::from_text(text, name, model, allow_unenforced).map_err(err)?;
            Ok(Contract { inner })
        }

        /// Check data against the contract and return a Report.
        ///
        /// `data` is any object exporting Arrow through the PyCapsule interface
        /// (a polars or pyarrow frame or batch, a DuckDB relation — zero-copy),
        /// a list of dicts (records), or the path of an NDJSON, CSV or Parquet
        /// file.
        #[pyo3(signature = (data, *, source = None))]
        fn check(
            &self,
            py: Python<'_>,
            data: &Bound<'_, PyAny>,
            source: Option<String>,
        ) -> PyResult<Report> {
            let clock = RunClock::start();
            let report = check_any(py, &self.inner, data, source)?;
            Ok(Report {
                passed: self.inner.passed(&report),
                budget: self.inner.budget(),
                outcome: self.inner.outcome(&report),
                timing: clock.stop(),
                report,
            })
        }

        #[getter]
        fn id(&self) -> String {
            self.inner.compiled.id.clone()
        }

        #[getter]
        fn version(&self) -> String {
            self.inner.compiled.version.clone()
        }

        #[getter]
        fn model(&self) -> String {
            self.inner.model.clone()
        }

        /// Rules not enforced, when the contract was loaded with
        /// `allow_unenforced=True`: dicts with `path`, `rule` and `reason`.
        #[getter]
        fn unenforced(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
            to_python(py, &self.inner.unenforced)
        }

        fn __repr__(&self) -> String {
            format!(
                "<Contract {} v{}, model {}{}>",
                self.inner.compiled.id,
                self.inner.compiled.version,
                self.inner.model,
                if self.inner.unenforced.is_empty() {
                    ""
                } else {
                    " (partial)"
                }
            )
        }
    }

    /// The outcome of a check: the same report `covenant check` prints.
    #[pyclass(frozen, module = "covenant_data")]
    struct Report {
        report: CheckReport,
        passed: bool,
        budget: u64,
        outcome: Outcome,
        /// When the check started and how long it took, for its document.
        timing: RunTiming,
    }

    impl Report {
        fn document(&self, samples: &str) -> PyResult<covenant::protocol::ReportDocument> {
            let samples = match samples {
                "masked" => SampleMode::Masked,
                "hashed" => SampleMode::Hashed(SampleKey::from_env().map_err(err)?),
                "none" => SampleMode::None,
                other => {
                    return Err(PyValueError::new_err(format!(
                        "samples is \"masked\", \"hashed\" or \"none\", not {other:?}"
                    )))
                }
            };
            Ok(super::report_document(
                &self.report,
                self.budget,
                self.outcome,
                self.timing,
                &samples,
            ))
        }
    }

    #[pymethods]
    impl Report {
        /// Whether the data passes: no more violations than the contract's
        /// budget, or a contract that only warns.
        #[getter]
        fn passed(&self) -> bool {
            self.passed
        }

        #[getter]
        fn rows(&self) -> u64 {
            self.report.rows
        }

        #[getter]
        fn violations(&self) -> u64 {
            self.report.violations
        }

        /// Whether rules were skipped: the verdict covers only part of the
        /// contract.
        #[getter]
        fn partial(&self) -> bool {
            !self.report.unenforced.is_empty()
        }

        /// The report as `covenant check --format json` writes it.
        fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
            to_python(py, &self.report)
        }

        fn to_json(&self) -> String {
            serde_json::to_string(&self.report).expect("reports serialize")
        }

        /// The check as a `covenant-report/v1` document, the one
        /// `covenant check --report-json` writes, on the `python` plane: made to
        /// leave the machine, so its samples never carry a value. `samples` is
        /// "masked" (each sample's field, rule and row, and the value's type
        /// and length), "hashed" (masked, and a keyed hash of each value; the
        /// key from COVENANT_SAMPLE_KEY) or "none" (the counts only).
        #[pyo3(signature = (*, samples = "masked"))]
        fn report_document(&self, py: Python<'_>, samples: &str) -> PyResult<Py<PyAny>> {
            to_python(py, &self.document(samples)?)
        }

        /// Write the check's `covenant-report/v1` document to `path`, as
        /// `covenant check --report-json` does.
        #[pyo3(signature = (path, *, samples = "masked"))]
        fn write_report(&self, path: PathBuf, samples: &str) -> PyResult<()> {
            self.document(samples)?.write(&path).map_err(err)
        }

        fn __bool__(&self) -> bool {
            self.passed
        }

        fn __str__(&self) -> String {
            self.report.render_human(self.budget)
        }

        fn __repr__(&self) -> String {
            format!(
                "<Report {}{} — {} rows, {} violations>",
                if self.passed { "PASS" } else { "FAIL" },
                if self.report.unenforced.is_empty() {
                    ""
                } else {
                    " (partial)"
                },
                self.report.rows,
                self.report.violations
            )
        }
    }

    /// Any serializable value, as the Python the json module would build.
    fn to_python(py: Python<'_>, value: &impl serde::Serialize) -> PyResult<Py<PyAny>> {
        let text = serde_json::to_string(value).expect("serializes");
        Ok(py.import("json")?.call_method1("loads", (text,))?.unbind())
    }

    fn check_any(
        py: Python<'_>,
        loaded: &Loaded,
        data: &Bound<'_, PyAny>,
        source: Option<String>,
    ) -> PyResult<CheckReport> {
        let kind = || -> String {
            data.get_type()
                .fully_qualified_name()
                .map(|n| n.to_string())
                .unwrap_or_else(|_| "<arrow>".into())
        };
        if data.hasattr("__arrow_c_stream__")? {
            let capsule = data.call_method1("__arrow_c_stream__", (py.None(),))?;
            let capsule = capsule.cast::<PyCapsule>()?;
            let ptr = capsule.pointer_checked(Some(STREAM))?;
            // Moves the stream out of the capsule, leaving it released: the
            // capsule's destructor then has nothing left to free.
            let stream = unsafe { FFI_ArrowArrayStream::from_raw(ptr.as_ptr().cast()) };
            let reader = ArrowArrayStreamReader::try_new(stream).map_err(err)?;
            let source = source.unwrap_or_else(kind);
            return py
                .detach(|| loaded.check_batches(reader, &source))
                .map_err(err);
        }
        if data.hasattr("__arrow_c_array__")? {
            let pair = data.call_method1("__arrow_c_array__", (py.None(),))?;
            let pair = pair.cast::<PyTuple>()?;
            let schema = pair.get_item(0)?;
            let array = pair.get_item(1)?;
            let schema = schema.cast::<PyCapsule>()?;
            let array = array.cast::<PyCapsule>()?;
            let schema_ptr = schema.pointer_checked(Some(SCHEMA))?;
            let array_ptr = array.pointer_checked(Some(ARRAY))?;
            // The schema is borrowed (its capsule frees it); the array is
            // moved out, as with the stream.
            let batch = unsafe {
                let schema = &*(schema_ptr.as_ptr() as *const FFI_ArrowSchema);
                let array = FFI_ArrowArray::from_raw(array_ptr.as_ptr().cast());
                from_ffi(array, schema).map_err(err)?
            };
            if !matches!(batch.data_type(), arrow_schema::DataType::Struct(_)) {
                return Err(PyTypeError::new_err(format!(
                    "{} exports an Arrow {} array, not a table of rows",
                    kind(),
                    batch.data_type()
                )));
            }
            let batch = RecordBatch::from(StructArray::from(batch));
            let schema = batch.schema();
            let reader = RecordBatchIterator::new([Ok(batch)], schema);
            let source = source.unwrap_or_else(kind);
            return py
                .detach(|| loaded.check_batches(reader, &source))
                .map_err(err);
        }
        if let Ok(records) = data.cast::<PyList>() {
            // One JSON object per line, for the reader the CLI uses. NaN has no
            // JSON form, so it is refused here rather than read as a string.
            let json = py.import("json")?;
            let kwargs = PyDict::new(py);
            kwargs.set_item("allow_nan", false)?;
            let mut ndjson = String::new();
            for record in records.iter() {
                let line: String = json
                    .call_method("dumps", (record,), Some(&kwargs))?
                    .extract()?;
                ndjson.push_str(&line);
                ndjson.push('\n');
            }
            let source = source.unwrap_or_else(|| "<records>".into());
            return py
                .detach(|| loaded.check_ndjson(&ndjson, &source))
                .map_err(err);
        }
        if let Ok(path) = data.extract::<PathBuf>() {
            if source.is_some() {
                return Err(PyTypeError::new_err(
                    "source names in-memory data; a file is named by its path",
                ));
            }
            return py.detach(|| loaded.check_path(&path)).map_err(err);
        }
        Err(PyTypeError::new_err(format!(
            "cannot check a {}: pass an object that exports Arrow (a polars or pyarrow \
             frame, a DuckDB relation), a list of dicts, or the path of an NDJSON, CSV or \
             Parquet file. For pandas, pyarrow.Table.from_pandas(df) exports Arrow.",
            kind()
        )))
    }

    /// Run the `covenant` command line in this process; returns its exit code.
    #[pyfunction]
    fn cli(py: Python<'_>, argv: Vec<String>) -> i32 {
        py.detach(|| super::cli(argv))
    }

    #[pymodule]
    fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add_class::<Contract>()?;
        m.add_class::<Report>()?;
        m.add_function(wrap_pyfunction!(cli, m)?)?;
        m.add("CovenantError", m.py().get_type::<CovenantError>())?;
        m.add("__version__", env!("CARGO_PKG_VERSION"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Loaded;
    use std::path::Path;

    fn orders() -> Loaded {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        Loaded::from_path(&root.join("examples/contracts/orders.yaml"), None, false).unwrap()
    }

    #[test]
    fn records_get_the_report_the_cli_gives_for_the_same_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let file = root.join("examples/data/orders_bad.ndjson");
        let contract = orders();
        let from_file = contract.check_path(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let from_text = contract
            .check_ndjson(&text, &file.display().to_string())
            .unwrap();
        assert_eq!(
            serde_json::to_value(&from_file).unwrap(),
            serde_json::to_value(&from_text).unwrap()
        );
        assert!(!contract.passed(&from_text));
    }

    #[test]
    fn a_stream_of_empty_batches_is_held_to_its_schema_once() {
        use arrow_array::{ArrayRef, RecordBatch, RecordBatchIterator, StringArray};
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;

        // Only order_id: the other three required fields are missing.
        let schema = Arc::new(Schema::new(vec![Field::new(
            "order_id",
            DataType::Utf8,
            true,
        )]));
        let empty: ArrayRef = Arc::new(StringArray::from(Vec::<&str>::new()));
        let batch = RecordBatch::try_new(schema.clone(), vec![empty]).unwrap();
        let contract = orders();
        let none = contract
            .check_batches(RecordBatchIterator::new([], schema.clone()), "none")
            .unwrap();
        let one = contract
            .check_batches(RecordBatchIterator::new([Ok(batch)], schema), "one")
            .unwrap();
        assert_eq!((none.rows, none.violations), (0, 3));
        assert_eq!((one.rows, one.violations), (0, 3));
    }

    #[test]
    fn a_partial_contract_is_refused_unless_allowed_and_then_says_so() {
        let odcs = "apiVersion: v3.1.0\nkind: DataContract\nid: c\nversion: 1.0.0\nschema:\n  - name: t\n    properties:\n      - name: a\n        logicalType: string\n      - name: b\n        logicalType: array\n";
        let refused = Loaded::from_text(odcs, "c.odcs.yaml", None, false)
            .err()
            .expect("refused");
        assert!(
            refused.to_string().contains("schema.t.properties.b"),
            "{refused}"
        );
        let partial = Loaded::from_text(odcs, "c.odcs.yaml", None, true).unwrap();
        let report = partial
            .check_ndjson("{\"a\": \"x\"}\n", "<records>")
            .unwrap();
        assert_eq!(report.unenforced.len(), 1);
        assert!(partial.passed(&report));
    }

    #[test]
    fn a_check_writes_the_commands_document_on_the_python_plane() {
        use covenant::protocol::{Outcome, RunClock, SampleMode};

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let contract = orders();
        let clock = RunClock::start();
        let report = contract
            .check_path(&root.join("examples/data/orders_bad.ndjson"))
            .unwrap();
        let outcome = contract.outcome(&report);
        assert_eq!(outcome, Outcome::Fail);
        assert!(!contract.passed(&report));
        let doc = super::report_document(
            &report,
            contract.budget(),
            outcome,
            clock.stop(),
            &SampleMode::Masked,
        );
        let doc = serde_json::to_value(&doc).unwrap();
        assert_eq!(doc["kind"], "check");
        assert_eq!(doc["verdict"], "fail");
        assert_eq!(doc["run"]["plane"], "python");
        assert_eq!(doc["violations"], report.violations);
        for sample in doc["checks"][0]["samples"].as_array().unwrap() {
            let keys: Vec<&String> = sample.as_object().unwrap().keys().collect();
            assert!(
                keys.iter()
                    .all(|k| { ["field", "rule", "row", "type", "length"].contains(&k.as_str()) }),
                "{keys:?}"
            );
        }
    }

    #[test]
    fn the_cli_runs_in_process_and_returns_its_exit_code() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let arg = |p: &str| root.join(p).display().to_string();
        let code = super::cli(vec![
            "covenant".into(),
            "validate".into(),
            arg("examples/contracts/orders.yaml"),
        ]);
        assert_eq!(code, 0);
        assert_eq!(super::cli(vec!["covenant".into(), "--version".into()]), 0);
        assert_eq!(
            super::cli(vec!["covenant".into(), "no-such-command".into()]),
            2
        );
    }
}
