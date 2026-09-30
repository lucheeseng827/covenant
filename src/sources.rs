//! Data sources for `covenant check`: NDJSON goes through the row engine
//! record-by-record (streaming, constant memory apart from `unique` sets);
//! CSV and Parquet are read into Arrow `RecordBatch`es and go through the
//! columnar engine — one engine per representation, no duplicated rules.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
#[cfg(feature = "arrow")]
use std::sync::Arc;

#[cfg(feature = "arrow")]
use arrow_array::RecordBatch;
#[cfg(feature = "arrow")]
use arrow_csv::reader::Format;
#[cfg(feature = "arrow")]
use arrow_schema::{DataType, Field as ArrowField, Schema};
#[cfg(feature = "arrow")]
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::compile::{CompiledContract, CompiledModel};
#[cfg(feature = "arrow")]
use crate::engine::arrow::{self as arrow_engine, SchemaFindings};
use crate::engine::{row as row_engine, UniqueTracker};
use crate::error::{CovenantError, Result};
use crate::profile::Profiler;
use crate::report::{CheckReport, Collector, ReportHeader, Rule, Violation};
#[cfg(feature = "arrow")]
use crate::spec::FieldType;

/// How many rows arrow-csv sniffs to infer column types.
#[cfg(feature = "arrow")]
const CSV_INFER_ROWS: usize = 1000;
/// Batch size for columnar readers.
#[cfg(feature = "arrow")]
const BATCH_ROWS: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Physical format of a checked data file.
pub enum DataFormat {
    Ndjson,
    Csv,
    Parquet,
}

impl DataFormat {
    /// Infer the format from the file extension (.ndjson/.jsonl/.json,
    /// .csv, .parquet); unknown extensions are a hard error, never a guess.
    pub fn infer(path: &Path) -> Result<DataFormat> {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("ndjson") | Some("jsonl") | Some("json") => Ok(DataFormat::Ndjson),
            Some("csv") => Ok(DataFormat::Csv),
            Some("parquet") => Ok(DataFormat::Parquet),
            _ => Err(CovenantError::UnknownFormat {
                path: path.display().to_string(),
            }),
        }
    }
}

/// Check one data file against one model of a compiled contract.
pub fn check_path(
    contract: &CompiledContract,
    model: &CompiledModel,
    path: &Path,
    format: Option<DataFormat>,
) -> Result<CheckReport> {
    check_path_profiled(contract, model, path, format, None)
}

/// [`check_path`], also feeding every record or batch the engines validate
/// to `profiler` — the same read, not a second pass over the file.
pub fn check_path_profiled(
    contract: &CompiledContract,
    model: &CompiledModel,
    path: &Path,
    format: Option<DataFormat>,
    profiler: Option<&mut Profiler>,
) -> Result<CheckReport> {
    let format = match format {
        Some(f) => f,
        None => DataFormat::infer(path)?,
    };
    let header = ReportHeader {
        contract_id: contract.id.clone(),
        contract_version: contract.version.clone(),
        owner: contract.owner.clone(),
        model: model.name.clone(),
        source: path.display().to_string(),
    };
    let mut collector = Collector::new(contract.policy.sample_violations);
    let mut unique = UniqueTracker::new(model);

    let rows = match format {
        DataFormat::Ndjson => {
            let file = File::open(path).map_err(|e| CovenantError::Io {
                path: path.display().to_string(),
                source: e,
            })?;
            check_ndjson_reader_profiled(
                model,
                BufReader::new(file),
                &mut unique,
                &mut collector,
                profiler,
            )?
        }
        #[cfg(feature = "arrow")]
        DataFormat::Csv => check_csv(model, path, &mut unique, &mut collector, profiler)?,
        #[cfg(feature = "arrow")]
        DataFormat::Parquet => check_parquet(model, path, &mut unique, &mut collector, profiler)?,
        #[cfg(not(feature = "arrow"))]
        DataFormat::Csv | DataFormat::Parquet => return Err(without_arrow(path, format)),
    };

    Ok(collector.into_report(header, rows))
}

/// Stream NDJSON through the row engine. Blank lines are skipped (trailing
/// newline files are the norm); a line that fails to parse is a violation,
/// not a hard error — bad serialization at the boundary is exactly what the
/// gate exists to catch.
pub fn check_ndjson_reader<R: BufRead>(
    model: &CompiledModel,
    reader: R,
    unique: &mut UniqueTracker,
    out: &mut Collector,
) -> Result<u64> {
    check_ndjson_reader_profiled(model, reader, unique, out, None)
}

/// [`check_ndjson_reader`], also profiling each record it validates.
pub fn check_ndjson_reader_profiled<R: BufRead>(
    model: &CompiledModel,
    reader: R,
    unique: &mut UniqueTracker,
    out: &mut Collector,
    mut profiler: Option<&mut Profiler>,
) -> Result<u64> {
    let mut rows: u64 = 0;
    let mut scratch: Vec<Violation> = Vec::new();
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| CovenantError::Io {
            path: "<ndjson input>".to_string(),
            source: e,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let row = rows;
        rows += 1;
        match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(record) => {
                scratch.clear();
                match profiler.as_deref_mut() {
                    // Validation hands each value it looks up to the profiler.
                    Some(p) if record.is_object() => {
                        p.start_record();
                        row_engine::validate_record_observed(
                            model,
                            &record,
                            row,
                            Some(unique),
                            &mut scratch,
                            |idx, v| p.observe_field(&model.fields[idx], idx, v),
                        );
                    }
                    Some(p) => {
                        p.observe_unreadable();
                        row_engine::validate_record(
                            model,
                            &record,
                            row,
                            Some(unique),
                            &mut scratch,
                        );
                    }
                    None => {
                        row_engine::validate_record(
                            model,
                            &record,
                            row,
                            Some(unique),
                            &mut scratch,
                        );
                    }
                }
                for v in scratch.drain(..) {
                    out.push(v);
                }
            }
            Err(e) => {
                if let Some(p) = profiler.as_deref_mut() {
                    p.observe_unreadable();
                }
                out.push(Violation {
                    model: model.name.clone(),
                    field: None,
                    rule: Rule::RecordNotObject,
                    row: Some(row),
                    value: Some(crate::report::truncate(&line, 64)),
                    message: format!("row {row} (line {}): invalid JSON: {e}", line_no + 1),
                    observed: None,
                });
            }
        }
    }
    Ok(rows)
}

/// A build without the `arrow` feature has no columnar engine to read CSV or
/// Parquet with: refused, never read some other, weaker way.
#[cfg(not(feature = "arrow"))]
pub(crate) fn without_arrow(path: &Path, format: DataFormat) -> CovenantError {
    let name = match format {
        DataFormat::Ndjson => "NDJSON",
        DataFormat::Csv => "CSV",
        DataFormat::Parquet => "Parquet",
    };
    CovenantError::DataRead {
        path: path.display().to_string(),
        message: format!(
            "reading {name} needs the `arrow` feature, which this build of covenant leaves out"
        ),
    }
}

#[cfg(feature = "arrow")]
fn check_csv(
    model: &CompiledModel,
    path: &Path,
    unique: &mut UniqueTracker,
    out: &mut Collector,
    mut profiler: Option<&mut Profiler>,
) -> Result<u64> {
    let io_err = |e: std::io::Error| CovenantError::Io {
        path: path.display().to_string(),
        source: e,
    };
    let data_err = |e: arrow_schema::ArrowError| CovenantError::DataRead {
        path: path.display().to_string(),
        message: e.to_string(),
    };

    // Inference is used only to discover the header columns; the read schema
    // comes from the CONTRACT. Letting inference pick types poisons the
    // check both ways: a digit-only string column (leading-zero IDs, zip
    // codes) infers as Int64 and then mismatches its `string` contract type,
    // and a column that goes non-numeric after the inference window blows up
    // the reader mid-file. Declared columns get their contract type;
    // undeclared columns stay Utf8 (strict mode flags them by name anyway).
    let mut file = File::open(path).map_err(io_err)?;
    let format = Format::default().with_header(true);
    let (inferred, _) = format
        .infer_schema(&mut file, Some(CSV_INFER_ROWS))
        .map_err(data_err)?;
    let fields: Vec<ArrowField> = inferred
        .fields()
        .iter()
        .map(|f| {
            let contract_ty = model.field_index.get(f.name()).map(|&i| model.fields[i].ty);
            let dt = match contract_ty {
                Some(FieldType::Integer) => DataType::Int64,
                Some(FieldType::Float) => DataType::Float64,
                Some(FieldType::Boolean) => DataType::Boolean,
                // string / timestamp / date / uuid validate their string
                // rendering per value; undeclared columns stay opaque text.
                _ => DataType::Utf8,
            };
            ArrowField::new(f.name(), dt, true)
        })
        .collect();
    let schema = Arc::new(Schema::new(fields));

    // infer_schema consumed the reader; reopen for the actual pass.
    let file = File::open(path).map_err(io_err)?;
    let reader = arrow_csv::ReaderBuilder::new(schema.clone())
        .with_format(format)
        .with_batch_size(BATCH_ROWS)
        .build(file)
        .map_err(data_err)?;

    // One file is one source: a schema problem counts once, not per batch.
    let mut schema_findings = SchemaFindings::new();
    let mut rows: u64 = 0;
    for batch in reader {
        match batch {
            Ok(batch) => {
                rows += arrow_engine::validate_source_batch(
                    model,
                    &batch,
                    rows,
                    Some(unique),
                    &mut schema_findings,
                    out,
                );
                if let Some(p) = profiler.as_deref_mut() {
                    p.observe_batch(model, &batch);
                }
            }
            // A cell that can't parse as its contract type IS bad data — a
            // violation (exit 1), not a runtime error (exit 2). arrow-csv
            // cannot resume after a parse error, so the check reports it and
            // stops; counts cover the rows read up to that point.
            Err(e) => {
                out.push(Violation {
                    model: model.name.clone(),
                    field: None,
                    rule: Rule::TypeMismatch,
                    row: None,
                    value: None,
                    message: format!(
                        "CSV value does not parse as its contract type (checking stopped here): {e}"
                    ),
                    observed: None,
                });
                break;
            }
        }
    }
    if rows == 0 {
        // Zero data rows must still fail schema promises (a header-only CSV
        // missing a required column is not "clean").
        arrow_engine::validate_source_batch(
            model,
            &RecordBatch::new_empty(schema),
            0,
            Some(unique),
            &mut schema_findings,
            out,
        );
    }
    Ok(rows)
}

#[cfg(feature = "arrow")]
fn check_parquet(
    model: &CompiledModel,
    path: &Path,
    unique: &mut UniqueTracker,
    out: &mut Collector,
    mut profiler: Option<&mut Profiler>,
) -> Result<u64> {
    let file = File::open(path).map_err(|e| CovenantError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let data_err = |m: String| CovenantError::DataRead {
        path: path.display().to_string(),
        message: m,
    };
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| data_err(e.to_string()))?
        .with_batch_size(BATCH_ROWS);
    let schema = builder.schema().clone();
    let reader = builder.build().map_err(|e| data_err(e.to_string()))?;

    // One file is one source: a schema problem counts once, not per batch.
    let mut schema_findings = SchemaFindings::new();
    let mut rows: u64 = 0;
    for batch in reader {
        let batch = batch.map_err(|e| data_err(e.to_string()))?;
        rows += arrow_engine::validate_source_batch(
            model,
            &batch,
            rows,
            Some(unique),
            &mut schema_findings,
            out,
        );
        if let Some(p) = profiler.as_deref_mut() {
            p.observe_batch(model, &batch);
        }
    }
    if rows == 0 {
        // Zero-row files still carry a schema and must honor it.
        arrow_engine::validate_source_batch(
            model,
            &RecordBatch::new_empty(schema),
            0,
            Some(unique),
            &mut schema_findings,
            out,
        );
    }
    Ok(rows)
}
