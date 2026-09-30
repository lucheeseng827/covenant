# covenant-data

Data contracts, enforced where the data flows. This is the Python face of
[Covenant](https://github.com/lucheeseng827/covenant): the same engine as the `covenant` command, in-process.

## Quickstart

```python
import covenant_data

report = covenant_data.check(df, "orders.yaml")   # polars, pyarrow, DuckDB: zero-copy
print(report)                                    # each rule, row and value that broke it
assert report.passed
```

## What it checks

- **Frames**: anything that exports Arrow through the PyCapsule interface — polars and
  pyarrow frames and batches, DuckDB relations — is checked batch by batch, without a copy.
  For pandas, `pyarrow.Table.from_pandas(df)`.
- **Records**: a list of dicts is checked exactly as `covenant check` checks an NDJSON file.
- **Files**: the path of an NDJSON, CSV or Parquet file.
- **Contracts**: `covenant: 1` documents and ODCS v3 contracts, exact or refused — a rule
  Covenant cannot enforce refuses the contract unless `allow_unenforced=True`, and then every
  report says it is partial.

`Contract("orders.yaml")` loads and compiles a contract once, for many checks.
`covenant_data.testing.assert_conforms(df, "orders.yaml")` fails a test with the report as
its message. The wheel also installs the `covenant` command.

## A report to keep

`report.write_report("covenant-report.json")` writes the check as the `covenant-report/v1`
document `covenant check --report-json` writes, on the `python` plane, and
`report.report_document()` returns it as a dict. It is made to leave the machine, so its
samples say where each violation was (field, rule and row) and the value's type and length,
never the value. `samples="hashed"` adds a hash of each value keyed with the
`COVENANT_SAMPLE_KEY` environment variable, so two reports can say "the same bad value"
without either saying what it was; `samples="none"` keeps the counts only. In CI it names the
repository, commit, ref and run. `covenant push report.json --to <url>` sends a written report.
[`docs/REPORT.md`](https://github.com/lucheeseng827/covenant/blob/main/docs/REPORT.md)
describes every field.
