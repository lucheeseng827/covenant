# covenant_data — architecture

The Python face adds no engine of its own. A `Contract` is compiled by the code
the `covenant` command uses, the data goes to the engine the command would pick
for it, and a `Report` wraps the command's own report. A verdict here is the
verdict the command gives, and `tests/test_covenant_data.py` holds the two equal.

## Where the data goes

```mermaid
flowchart LR
    CALL["covenant_data.check(data, contract)"] --> WHAT{"data is"}
    WHAT -->|"__arrow_c_stream__\npolars, pyarrow, DuckDB"| STREAM["ArrowArrayStreamReader\narrow-rs C Data Interface"]
    WHAT -->|"__arrow_c_array__\none struct array"| ARRAY["one RecordBatch"]
    WHAT -->|"a list of dicts"| NDJSON["json.dumps per record\nNaN refused"]
    WHAT -->|"str or PathLike"| PATH["sources::check_path"]
    WHAT -->|"anything else"| TYPEERR["TypeError"]
    STREAM --> BATCHES["Loaded::check_batches\nuniqueness and schema findings\nspan the whole stream"]
    ARRAY --> BATCHES
    BATCHES --> ARR["engine::arrow"]
    NDJSON --> READER["the command's NDJSON reader"]
    READER --> ROW["engine::row"]
    PATH -->|NDJSON| ROW
    PATH -->|"CSV, Parquet"| ARR
    ARR --> REPORT["CheckReport, wrapped as Report"]
    ROW --> REPORT
```

## A check call

```mermaid
sequenceDiagram
    participant Py as Python caller
    participant G as covenant_data (PyO3)
    participant L as Loaded contract
    participant E as engine
    Py->>G: Contract("orders.yaml")
    G->>L: parse + lint + compile, once
    L-->>G: compiled, or CovenantError (a refused contract is not a verdict)
    Py->>G: contract.check(frame)
    G->>G: take the Arrow stream out of its capsule, no copy
    G->>L: check_batches(reader), GIL released
    loop per batch
        L->>E: validate_source_batch
    end
    L-->>G: CheckReport
    G-->>Py: Report: passed, rows, violations, str(), to_dict()
```

## Decisions

- **The command's readers, not new ones.** Records become NDJSON lines and go
  through the reader `covenant check` uses for an NDJSON file, and a path goes
  through the command's own `check_path`, so the module and the command share
  every step after the bytes.
- **arrow-rs's C Data Interface.** The PyCapsule protocol is imported with
  arrow-rs's own `ffi` module; `pyo3-arrow` would do the same import but pin a
  different pyo3 and add NumPy to the build.
- **The GIL is released** while the engine runs, so other Python threads keep
  going during a long check.
- **An error is never a verdict.** `CovenantError` means the contract or the
  data could not be read; a failing check returns a `Report` whose `passed` is
  false, as the command exits 1 rather than 2.
- **One wheel.** abi3 from CPython 3.9, with the `covenant` command as a console
  script that runs the same code in-process.
