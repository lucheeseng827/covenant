# Changelog

## Unreleased

## 0.2.0 — 2026-10-01

### The report: `covenant-report/v1`

- `check --report-json <path>` also writes the run's verdict as a versioned JSON document for
  CI artifacts, catalogs and services: the verdict (`pass`, `fail` or `warn`, as the exit code
  says), the exact counts per field and rule, and where and when the run happened. In GitHub
  Actions or GitLab CI the run names its repository, commit, full ref and run URL. The terminal
  output and the exit code are unchanged; a report that cannot be written fails the run with
  exit 2.
- The document is made to leave the machine, so its samples never carry a value:
  `--report-samples masked` (the default) keeps each sample's field, rule and row and the
  value's `type` and `length`, and `none` the counts only. Messages are left out too, because
  they quote the value.
- `--report-samples hashed` (`samples="hashed"` in Python) adds each value's `hash`: the HMAC-SHA256,
  cut to 16 bytes, of the SHA-256 of its canonical form, under a key from `COVENANT_SAMPLE_KEY`
  (at least 16 bytes; never read from the command line, never written). Under one key the same
  value has the same hash in every run and every format, so two documents can say "the same
  bad value" without either saying what it was; the document's `sample_key` is the key's id,
  and a run asked for hashes without a key stops before it starts (exit 2).
- `schema/covenant-report.v1.json` is the JSON Schema. It refuses a sample with a value or a
  message. Golden files in `tests/golden/report/` pin the document, and the tests validate every
  document they write against the schema. `docs/REPORT.md` describes the fields. The format has
  its own revision key (`report: 1`) and, within it, only grows.
- The CI provider's variables are read only when a document is made; `COVENANT_SAMPLE_KEY`,
  `COVENANT_TOKEN`, `OPENLINEAGE_API_KEY` and the proxy variables only when their feature is
  used.
- `gate --report-json <path>` writes the same document when its stream ends (`kind: gate`,
  plane `gate`): the stream's counts per field and rule, what became of its records (`passed`,
  `blocked`, `warned`) and masked samples whose rows are the dead letters' rows. It checks the
  document's directory before it reads the first record. `--report-samples` works as it does on
  `check`; `--subprocess` writes the document too, and keeps stderr for its replies.
- The gate over Kafka (`--brokers`) writes it when its run ends (a signal, or `--exit-at-end`):
  its source is the topic, `kafka://<first bootstrap server>/<topic>`, its rows are the records
  judged, and `gate.tombstones` counts the records with no value, which go on unjudged.
- `diff --report-json <path>` writes the classified change (`kind: diff`): the two versions,
  every change with its severity, impact and path, the `--fail-on` threshold and verdict and,
  with `--consumers`, who each change breaks.
- The Python face writes it too: `Report.report_document(samples="masked")` returns the document
  as a dict and `Report.write_report(path)` writes it, on the `python` plane, timed by the check
  it reports. It is the command's document for the same data, but for its plane and timings.
- `run.plane` names what produced the document: `cli` or `ci` for a command, `gate` and
  `python`, which keep their plane in CI and name the CI run in `run.ci`. The schema holds each
  kind to its body, refuses a CI context on a `cli` run and requires one on a `ci` run.
- `--report-to <url>` on `check`, `gate` and `diff` POSTs the document as JSON, byte for byte what
  `--report-json` writes, with the token from `COVENANT_TOKEN` as `Authorization: Bearer`. A
  document that cannot be sent is one warning; the exit code stands. `covenant push <file>…
  --to <url>` sends documents already written, and exits 2 if one is not a document or was not
  delivered. The URL is checked before the run: `https` anywhere, plain `http` to this machine
  only, no credentials in it. Redirects are not followed; the environment's proxy is used off
  this machine only.
- `--openlineage <url>` on `check` and the Kafka gate sends the run to an OpenLineage endpoint
  (Marquez, or any service that takes the standard's HTTP events) as START and COMPLETE
  events (OpenLineage 2-0-2) with one run id, a UUIDv7 derived from the document. Each source is
  an input dataset named as OpenLineage names files and topics, carrying a
  `dataQualityAssertions` facet (one assertion per declared rule and column, whether it held,
  at `error` or, under `on_violation: warn`, `warn`) and `dataQualityMetrics` (the rows read). No
  values and no samples. The key comes from `OPENLINEAGE_API_KEY`. The tests hold the events
  to the published OpenLineage schemas, copied in `tests/openlineage/`.
- A new `marquez` CI job holds `--openlineage` to a running catalog: Marquez 0.51.1, the
  OpenLineage reference implementation, keeps the run and shows the dataset's quality as its
  passing and failing rules, each failing rule by column, with a later run's verdict in place
  of an earlier one (`tests/marquez_test.rs`, ignored unless `COVENANT_TEST_MARQUEZ` names a
  Marquez). The Kafka gate's topic was checked by hand the same way.
- Sending is the new `send` feature, on by default: ureq 3 over rustls, with the Mozilla root
  certificates compiled in (the system trust store is not read). WASI builds leave it out.
- `Violation` gains `observed`: the offending value's type, length and digest, never
  serialized, which the report's samples are made from. It is outside the stable surface for
  now ([`docs/API.md`](docs/API.md)).

### Stream processors

- `RecordGate`, in the prelude, is the gate one record at a time: `judge(bytes)` gives a
  `Verdict` (`Pass`, `Block` or `Warn`), `violations()` and `dead_letter()` say why, and it keeps
  what spans a stream: `unique` keys, row numbers, the counts and the violation budget. The
  line gate and `--subprocess` run on it, and so does every integration that embeds the gate,
  so none of them can judge a record differently.
- `covenant gate --brokers … --from … --to …` (the new, off-by-default `kafka` feature) gates
  topic to topic. A record that goes on keeps its key, value, headers and timestamp and lands
  in the partition a Java producer would pick for its key. Dead letters go to `--dlq-topic`,
  with the record's key and headers plus `covenant.source.topic`, `.partition` and `.offset`,
  or else to `--dlq` or stderr. Tombstones go on unjudged. Delivery is at-least-once: an offset
  is committed only after the brokers acknowledge everything produced for the records before
  it, every `--commit-interval-ms` (1000), before a rebalance revokes partitions, and on
  SIGINT or SIGTERM. A refused record stops the gate with exit 2 and nothing is committed after
  it. The gate never creates topics. Connection settings go through `-X key=value` or
  `--kafka-config`; TLS, SASL PLAIN/SCRAM/OAUTHBEARER and every compression codec are built
  in. `--exit-at-end` suits backfills and tests. The tests in `tests/kafka_test.rs` run
  against a real broker in CI (Apache Kafka), and were checked against Redpanda too. One of
  them rebalances two gates with only the revoke path committing and finds every record
  exactly once.
- `integrations/redpanda-transform` builds the gate as a Redpanda Data Transform, WebAssembly
  the broker runs on every record written to a topic. The contract is compiled in at build
  time (`COVENANT_CONTRACT`), with the engine's own load, lint and compile, so a contract the
  transform could not enforce exactly fails the build. Records that go on reach the first
  output topic unchanged; dead letters, with the record's key and headers, go to the output
  topic `COVENANT_DLQ_TOPIC` names, and without it the transform does not start; tombstones go
  on unjudged. A transform's state is per partition and does not survive a restart, so a model
  with `unique` fields builds only with `COVENANT_NO_UNIQUE=1`. A 256 KiB stack keeps the module
  (1.4 MB) within Redpanda's default 2 MiB per instance. `rpk transform build` and
  `rpk transform deploy` work from its directory. `e2e.py` deploys the module into a real
  Redpanda and requires the same passed records and dead letters as `covenant gate --no-unique`
  on the same input; CI runs it against Redpanda 26.2.3.
- `integrations/jvm` brings the gate to the JVM without native code. `embed/` builds the
  engine as a WebAssembly reactor module with a small C ABI, where one instance holds one
  gate. `covenant-jvm` runs that module on Chicory, compiled to JVM bytecode, through `Gate`
  (`open`, `judge`, `deadLetter`, `stats`), with `UniqueKeys` choosing whether a model's
  `unique` fields are refused, skipped or tracked. A warmed-up gate judges about 55,000
  records a second. `covenant-connect` is a Kafka Connect transformation shaded into one jar.
  A blocked record fails with its dead letter as the message, so Connect's error handling
  and a sink's dead letter queue take it from there; `on.block=drop` drops it instead. A
  record under `on_violation: warn` goes on with a `covenant.dead_letter` header, and a
  tombstone goes on unjudged. The library is held to `covenant gate` on the demo orders.
  `e2e_connect.py` runs a real Connect worker and requires its sink and dead letter queue to
  match the command. CI runs both against Apache Kafka 4.1.0.
- `covenant-kroxylicious` is a Kroxylicious filter: the gate in a Kafka proxy, at produce time.
  A batch holding a record that breaks the contract is refused whole, with `INVALID_RECORD`
  and one record error per broken record carrying its dead letter. A Java producer fails that
  record with an `InvalidRecordException` whose message is the dead letter. Other partitions
  of the request go on, and so do ungated topics and tombstones. Requests that name topics by
  id are resolved to names first. The contract is compiled when the proxy starts, so one that
  cannot be enforced stops the proxy from starting. `e2e_kroxylicious.py` produces through a
  real Kroxylicious 0.23.0 proxy with an idempotent producer and requires the broker to hold
  exactly what `covenant gate` passes, the producer to be refused exactly what it blocks, and
  a request over two partitions to keep the clean one.
- `integrations/arroyo` has two Rust UDFs for Arroyo SQL that link the engine:
  `covenant_verdict(record)` returns `'pass'`, `'block'` or `'warn'`, and
  `covenant_dead_letter(record)` returns the dead letter `covenant gate` writes, or `NULL`.
  `render.py` compiles a contract into their sources after checking it with the engine. Each call
  judges one record on its own, so a model with `unique` fields fails the UDF's first call unless
  it was rendered with `--no-unique`. `e2e_arroyo.py` does the following against Arroyo 0.15.0:
  it starts a real cluster, registers the UDFs so that Arroyo's own compiler builds them against
  the published UDF plugin, runs a SQL pipeline over the demo orders, and requires the same
  records and dead letters as `covenant gate`.
- `CovenantError::Transport` reports a transport failure that no retry fixes, such as
  brokers that do not answer, a missing topic, or a record the brokers refused.
- `covenant gate --subprocess` answers every line with exactly one line, flushed at once: the
  record on stdout if it goes on, its dead-letter envelope on stderr if it is withheld, a blank
  line for a blank one. Redpanda Connect's `subprocess` processor speaks this: a stdout reply
  replaces the message and a stderr reply marks it failed, so `errored()` routes withheld
  records and `error()` carries their envelope. `--dlq` still receives every dead letter; no
  summary follows the stream, since stderr is the reply channel. Checked with Redpanda Connect
  4.70.0; the WASM golden tests cover the mode.

### Schema violations count once per source

- The columnar engine counts a schema-level violation (a required column missing, a column of
  the wrong type or of a type a declared constraint cannot be evaluated on, such as `allowed` on
  a native timestamp, an undeclared column in a strict model) once per source. Before, a CSV or
  Parquet file read in several batches of 8192 rows counted it once per batch, so the count,
  and under a `max_violations` budget the verdict, depended on the file's size; a Python frame
  in several chunks did the same. Row-level counts were never affected.
- `validate_source_batch` and `SchemaFindings` (both in the prelude) check a source that
  arrives as several batches. `validate_batch` still checks one batch as a whole source, so
  calling it per batch keeps the per-batch count.

### WASM

- The `covenant` command builds for `wasm32-wasip1` unchanged (`cargo build --release --target
  wasm32-wasip1`): `covenant.wasm`, about 4 MB, runs under any WASI runtime with the arguments,
  output and exit codes of the native binary, reading and writing the directories it is granted.
- A new `wasm` CI job builds both and compares them byte for byte — the example data, a CSV
  through the Arrow engine, the stream gate over stdin, `diff`, `export postgres`, the
  conformance vectors and a written profile — running the WASM build under Node's built-in WASI
  (`.github/scripts/wasm-run.mjs`).
- `wasm-run.mjs` turns off V8's fast API calls. Node 22's WASI can start a garbage collection
  inside one (its allocator reports memory from `path_open`), and V8 then segfaults walking
  the stack, at a point that moves with the module's size (seen with Node 22.22, where Node 20
  and 21 ran the same build).
- `js/covenant.mjs` runs `covenant.wasm` from JavaScript, in browsers and Node 20+, over an
  in-memory filesystem, with no dependencies: `load`, then `run(args, { files, stdin })` or
  `check({ contract, data })` for the JSON report. `js/golden.mjs` holds it to the native build
  on the same cases, and CI runs it.
- `js/playground.html`: paste a contract (`covenant: 1` or ODCS) and some CSV or NDJSON, get the
  command's report. The engine runs in the page; nothing is uploaded.

### Python

- `python/` builds `covenant_data`, the engine as a Python module (PyO3, maturin; one abi3
  wheel for CPython 3.9+, published as `covenant-data` once released). `covenant_data.check(df,
  "orders.yaml")` checks anything that exports Arrow through the PyCapsule interface — polars
  and pyarrow frames and batches, DuckDB relations — batch by batch without a copy, through the
  Arrow engine; a list of dicts exactly as `covenant check` checks an NDJSON file; and a path
  as a file. `Contract` compiles a contract once for many checks; `Report` carries `passed`,
  `rows`, `violations`, `partial`, the human rendering as `str()` and the JSON report as
  `to_dict()`; `CovenantError` means the contract or the data could not be read, never a
  failing verdict.
- `covenant_data.testing.assert_conforms(data, contract)` fails a pytest or unittest test with
  the report as its message, and the wheel installs the `covenant` command.
- The tests compare the module's reports with the command's on the same data, and run in CI on
  the oldest and newest CPython the wheel claims.

### The `arrow` feature

- The Arrow engine and the CSV and Parquet readers are now the `arrow` feature, on by default.
  Default builds are unchanged. `default-features = false` leaves the row engine alone, with no
  Arrow linked and no Arrow version constraint: the Redpanda transform, the JVM module and the
  Arroyo UDFs build that way. The Arroyo UDFs need it, because Arrow 58's chrono clashes with
  the Arrow the UDF plugin is built on. Without the feature, `check` and `infer` refuse CSV and
  Parquet with a message that names it. `validate_batch`, `validate_source_batch`,
  `SchemaFindings` and `Profiler::observe_batch` exist only with it.
- The engine asks chrono for `now` itself, where it had relied on the Arrow crates to turn on
  chrono's clock.

### Breaking

- The package is now **`covenant-data`**, since the name `covenant` is taken on crates.io,
  PyPI and npm. The library and the binary are still
  `covenant`, so `use covenant::…` and the `covenant` command are unchanged. A git dependency
  declared as `covenant = { git = … }` becomes `covenant-data = { git = … }`, or keeps its
  key with `package = "covenant-data"`.
- `report::Rule` and `error::CovenantError` are `#[non_exhaustive]`, like the new
  `drift::Metric` and `spec::SourceFormat`: new variants arrive in minor releases, so a
  `match` on them needs a wildcard arm.

### Engine API

- `covenant::prelude` is the stable surface for embedders, in one import: load and compile a
  contract, check records, batches and files, gate a stream, classify a contract change,
  profile data and compare profiles. `docs/API.md` states the change policy: until 1.0 each
  break is listed here under **Breaking**; from 1.0 the prelude only grows within a major
  version, and a removal is deprecated at least 90 days before. Formats (`covenant: 1`,
  `consumer: 1`, `profile: 1`, `vector: 1`), exit codes and JSON output fields are held to
  the same standard.
- `tests/api_surface.rs` names every prelude item with its full signature, so a change that
  would break an embedder fails to compile unless that file changes with it.

### Postgres: the contract as a table's own constraints

- `covenant export postgres -c <contract> [--model M] [--table schema.table] [-o file]` prints a
  `CREATE TABLE` whose column types, `NOT NULL`, `CHECK` and `UNIQUE` constraints reject the
  rows `covenant check` reports. Each constraint is named `<column>.<rule>`, so a rejected
  insert names the rule it broke. A rule without an exact equivalent — a pattern using `\d`,
  `\w`, `\s` or `\b`, `allowed` or `unique` on a timestamp — refuses the export (exit 2);
  `--allow-unenforced` exports the rest and lists what the table leaves out.
- Exact means: for values of the column types, inserted one row at a time, a row is rejected
  exactly when the engines report it given the rows the table holds. `tests/postgres_test.rs`
  checks this against a real Postgres on the ODCS conformance vectors, on edge values (NaN,
  the infinities, `-0.0`, integers past float precision, combining characters, Unicode
  whitespace in emails and URIs) and on a table of translated patterns; a new `postgres` CI
  job runs it. `docs/POSTGRES.md` has the mapping.
- Found on the way: the Arrow engine keyed `-0.0` and `0.0` apart for `unique`, where the row
  engine (and Postgres) count them as one value. It now keys them alike.

### ODCS conformance vectors

- `conformance/odcs/`: 36 engine-neutral test vectors for ODCS v3.1.0 — a contract, a few
  records and the verdict the standard implies, with the failing records where the standard
  fixes them — covering logical types, `required`, `unique`, primary keys, `enum`, the string
  and number options, dates and timestamps, and `library` quality rules. Each cites the
  section of the standard it pins.
- `covenant conformance <paths>…` runs them: exit 0 when every vector Covenant supports
  agrees, 1 when one disagrees, 2 on a malformed vector. Today: 30 pass, 6 unsupported
  (`multipleOf`, exclusive bounds on numbers, thresholds in rows and in percent,
  multi-column duplicates, `rowCount`), 0 fail. `--format json` for other tooling.
- Where the standard's text allows two readings, the case is left out of the suite and listed
  in `docs/CONFORMANCE.md` as a question for the standard, with what Covenant does meanwhile.
- One such question changed the reader: the standard defines `mustBeBetween` both as `∈`
  (bounds included) and as `mustBeGreaterThan` plus `mustBeLessThan` (bounds excluded). A
  `mustBeBetween` whose meaning differs between the two, such as `[0, 0]`, is now refused
  rather than enforced under one of them; for zero tolerance, write `mustBe: 0`.

### ODCS revisions

- The reader lists the ODCS revisions it has been reviewed against — v3.0.0, v3.0.1, v3.0.2,
  v3.1.0 and v3.2.0 (`odcs::VERSIONS`) — and `docs/ODCS.md` records what each changed and how
  it maps. A contract of any other v3 revision is refused, naming its `apiVersion`, because a
  rule that revision introduced would otherwise go unchecked; under `--allow-unenforced` it is
  read as v3.2.0 and the result is marked partial. Before, any `v3.*` was read as a known
  revision.
- Two v3.2.0 features are now handled instead of read past. A property with
  `semanticType: measure` is an aggregate over the records, not a column: it is not checked as
  one (`validate` notes it), and a rule on it is refused. Variables (`${NAME}`,
  `${NAME:-default}`) are not resolved yet: a column name, pattern or allowed value that holds
  one is refused instead of being matched as literal text.
- A weekly `ODCS watch` workflow finds the revisions the standard has published, opens an
  issue to review each one Covenant does not read yet, and fails once one has gone 60 days
  without support. It also runs the reader over the standard's published examples: today all
  42 read.

### Profiles and drift

- `covenant check --profile <path>` also writes a profile of the checked data, collected in
  the check's own read. Per contract field it records presence, nulls, values not of the
  contract type, distinct values (HyperLogLog, about 1.6 % error), numeric min, max, mean and
  quantiles (a log-bucketed histogram, within 0.78 %), string lengths, timestamp and date
  ranges, and exact counts for booleans and `allowed:` fields (values outside the set are
  counted together, never by value). Other strings contribute lengths and distinct counts
  only.
- The same records give the same profile in NDJSON, CSV and Parquet, bit for bit (a columnar
  file's absent values count as nulls). Merges are exact and independent of order:
  `covenant profile merge` folds runs, partitions or days into a baseline, and
  `covenant profile show` summarizes one (`--format json` gives rates and quantiles as data).
- `covenant drift <baseline> <current>` compares two profiles: rows per run; null, missing
  and invalid rates; distinct values; PSI for numeric distributions; and Jensen–Shannon
  distance for categorical ones. Each finding is one number against one threshold
  (`--rate`, `--volume`, `--psi`, `--js`, `--distinct`), explained in a sentence. Exit 1 on
  drift. Too little data to judge is listed as not compared.
- Cost, off unless asked for, counted in instructions on a 1M-row, seven-field benchmark:
  +8.6 % for NDJSON, +16.5 % for CSV, +17.6 % for Parquet.
- Library: `profile::Profiler`, `sources::check_path_profiled` and
  `check_ndjson_reader_profiled`, `engine::row::validate_record_observed` (the row engine
  hands each value it looks up to an observer), `sketch::{Hll, Quantiles}`, `drift::drift`.
  `docs/PROFILES.md` describes the format and the metrics.

### MCP server

- `covenant mcp` serves `validate`, `check` (files or inline records, optionally writing a
  profile), `diff` (with the consumer blast radius), `explain` (what a contract requires,
  field by field, and which ODCS rules it does not enforce) and `drift` as Model Context
  Protocol tools over stdio, protocol revisions 2024-11-05 to 2025-06-18. A verdict is a
  result, even a failing one; unreadable input is a tool error (`isError: true`).

### ODCS v3 contracts

- `validate`, `check`, `gate`, `diff`, `consumer-check` and `serve` read Open Data Contract
  Standard documents (v3.0.0 to v3.2.0), recognised by `apiVersion` and
  `kind: DataContract`, alongside `covenant: 1`. Logical types, `required`, `unique`, primary
  keys, `enum`, string length/pattern/format options, integer bounds (exclusive ones
  exactly, in v3.0's flag form and v3.1's value form), and zero-tolerance `library` quality
  rules map onto the existing rules. Data is matched by `physicalName` when a property has
  one. The mapping is in `docs/ODCS.md`.
- Exact or refused: a rule without an exact equivalent — thresholds, `rowCount`, `sql`,
  `custom` and `text` rules, nested types, relationships — refuses the contract with exit 2,
  naming each rule's path. `--allow-unenforced` (a global flag) checks everything else and
  marks the result partial: `check` reads `PASS (partial)` or `FAIL (partial)`, `diff` lists
  the rules it did not compare, their JSON reports and `covenant serve`'s check reports carry
  an `unenforced` list, and `validate` reports those rules as warnings instead of errors.
- `POST /v1/validate` lints ODCS documents too, with unenforceable rules as errors, and
  `GET /v1/contract` lists the served contract's unenforced rules.
- Library: `Contract::load` and `Contract::load_path` return the contract with its unenforced
  rules and the reader's notes, and `LoadedContract::lint` turns both into findings;
  `Contract::parse` and `Contract::from_path` keep refusing. `CheckReport` and `DiffReport`
  gain an `unenforced` list, and `serve::AppState` a required `unenforced` field.
- Tests load 40 of the ODCS project's published example contracts: of the 34 that define a
  schema, 15 are enforced in full.

### Minimum supported Rust

- Declares a minimum supported Rust version: **1.85** (`rust-version` in `Cargo.toml`, set by
  the arrow/parquet 58 and clap 4.6 floors). A new `msrv` CI job reads the field and builds
  with exactly that toolchain, default and `serve` features, so the declaration cannot drift.

### Signed releases

- Every image `release.yml` pushes is signed and carries build provenance, with no key for
  anyone to hold: cosign signs the pushed digest keylessly (Sigstore, from the release job's
  GitHub identity), `actions/attest-build-provenance` records how it was built (SLSA), and the
  job verifies both with the commands `SECURITY.md` gives users before it reports success. The
  image's index stays a plain multi-arch manifest list. A publish by hand has to run on the
  release tag, so every image on Docker Hub is signed by a run on its tag and passes those
  commands. A manual run with `rehearse` takes the whole path against the throwaway registry
  ttl.sh, so it is tested before a release depends on it. `0.1.0` predates this and is unsigned.

## 0.1.0 — 2026-08-22

### `covenant infer` — draft a contract from real data

- The on-ramp: every other command assumes a contract exists; `covenant
  infer <data>...` reads a sample (NDJSON, CSV, Parquet — the same readers
  `check` uses) and drafts one. Round-trip is the load-bearing property:
  the draft parses, compiles, lints clean, and accepts the data it came
  from, all enforced by tests.
- Honest about evidence. Types, presence, and nullability are emitted as
  rules; enums, patterns, and ranges are emitted with a `# confirm:` note
  naming what they were inferred from. `unique` is NEVER drafted (a sample
  cannot prove it, and a wrong one fails clean data in production) — it is
  suggested as a note instead.
- Refusals are explicit: mixed-type columns widen to `string` and say so,
  nested objects are flagged rather than flattened, an all-null column
  admits it is a guess, and inferring from zero records is an error rather
  than an empty contract that guards nothing.
- Shape detection reuses `compile::shape`, so a type `infer` drafts is a
  type the engines accept. Drafts start at version `0.1.0` with `owner:` a
  TODO. `--sample`, `--max-enum`, `--no-ranges`, `--id`, `--model`,
  `--out` (never overwrites), and `--format json` (contract + notes as
  data) tune it.

### Advisory `deprecated:` field marker

- Fields may carry `deprecated: "<migration note>"` (or a bare
  `deprecated: true`) — the one advisory marker: enforcement is unchanged.
  `covenant diff` reports the transition (either direction) as
  info/consumers, a deprecated field's later removal says "(was
  deprecated)" while staying breaking, and `covenant consumer-check` warns
  every reader of a deprecated field with the note. The console's field
  detail shows the marker, and `consumer-check` warns each declaring reader.

### Phase-2 serve endpoints: gate stats + DLQ reader

- `covenant gate --stats <path>` — the stats sidecar: the gate periodically
  (and at end of stream) writes an atomic (tmp+rename) JSON snapshot with
  run counters, exact per-(field, rule) violation counts, a bucketed p99 of
  per-record parse+validate latency, and rolling 10-second throughput
  buckets. Write failures warn once and disable the sink — observability
  never takes down enforcement. The gate stays a plain pipe process.
- `covenant serve --gate-stats <path>` → `GET /v1/gate/stats`, and
  `covenant serve --dlq <path>` → `GET /v1/dlq?limit=N` (bounded tail-read,
  newest first; torn concurrent-append lines are counted in `skipped`).
  Both answer honestly when unwired (`configured: false` + hint) or when
  the file isn't there yet (`note`), and the snapshot's age is reported so
  a stopped gate is visible.
- DLQ envelopes now carry an RFC 3339 `ts` (additive; readers must treat
  it as optional).
- Console: the Stream-gate and Dead-letters screens render live data —
  stats row, throughput bars, per-rule rates, blocked tail, DLQ list +
  record inspector. Unwired taps show a "demo · not wired" chip carrying
  the server's own fix-it hint; the replay button on live data says it
  needs Phase 3 and does nothing.

### Consumer-side enforcement (two-sided contracts)

- `covenant consumer-check <manifests...> -c contract.yaml` — verify
  manifests against a contract in the *consumer's* CI: every declared field
  must still be enforced, named models must exist, and the new optional
  `verified: <semver>` pin per `consumes` block is graded (a breaking bump
  past the pin = error — major, or 0.x minor since pre-1.0 minors are
  breaking by semver; non-breaking drift = warning; pin ahead of the
  checked contract = warning). Exit codes 0/1/2 as everywhere.
- `covenant check --as-consumer manifest.yaml` — scope a data check to one
  consumer's declared fields: constraints on undeclared fields are dropped
  and strict mode is off. A manifest inconsistent with the contract refuses
  to scope (hard error pointing at `consumer-check`) rather than validating
  fields that no longer exist.
- `POST /v1/consumers/verify` — the API form of `consumer-check` against
  the served contract.
- Example manifests now carry `verified:` pins.

### Consumer blast radius (added after the console)

- Consumer manifests (`consumer: 1`, `src/consumers.rs`): consumers declare
  the contract fields they actually read; `covenant diff --consumers <dir>`
  intersects the classified diff with those declarations and reports the
  named blast radius — who breaks, owned by whom, via which fields.
  Producer-side changes never hit readers; the semver-bump finding is
  excluded as process discipline.
- `--fail-on breaking-with-consumers`: fail the merge gate only when a
  breaking change hits a *declared* consumer (requires `--consumers`).
- `POST /v1/diff` accepts an optional `consumers` array of manifest YAMLs;
  the console's PR-gate consumer panel renders the live blast radius.
- Demo manifests in `examples/consumers/`.

### Console + serve (added after the initial MVP)

- `frontend/` — the Covenant Console as a Vite + React + TypeScript app:
  the Claude Design mock (nocturne design system) mechanically converted to
  generated TSX (1-to-1 markup/styles, React escaping by construction), a
  typed `/v1` API client mirroring the serde shapes, and the mock's state
  model ported verbatim into typed `renderVals`. The built `dist/` is
  committed and embedded into the binary via rust-embed (no Node needed for
  `cargo build --features serve`). Supersedes the interim single-file
  `site/console.html` port.
- `covenant serve` (feature `serve`, OFF by default; axum): embeds the
  console and exposes the stateless data-plane API — `GET /v1/health`,
  `GET /v1/contract`, `POST /v1/validate`, `POST /v1/check`,
  `POST /v1/diff`. Six router tests (`--features serve`).
- Live wiring with honest gaps: Editor / Check reports / PR gate / Contract
  detail render real engine results when served; screens needing fleet
  state (registry, history, teams, DLQ replay, alerts) carry an in-UI
  "endpoint missing" chip.

Initial MVP: the enforcement engine and all three boundary points. The
`v0.1.0` tag lands with the first public release.

- Contract spec `covenant: 1` (YAML/JSON): models → typed fields
  (`string/integer/float/boolean/timestamp/date/uuid`) with
  `required/nullable/unique/pattern/min/max/min_length/max_length/allowed/
  format(email|uri)` constraints and an enforcement policy
  (`block|warn`, `max_violations`, `sample_violations`).
- `covenant validate` — contract linter; compile refuses error-level
  contracts (no half-enforcement mode).
- `covenant check` — CI gate over NDJSON/CSV/Parquet with exact per-rule
  counts, capped samples, owner attribution, human/JSON output, and the
  0/1/2 exit-code contract.
- `covenant gate` — transport-agnostic NDJSON stream interceptor
  (stdin→stdout, violations → DLQ envelopes with replayable records).
- `covenant diff` — breaking-change classifier (breaking/risky/info ×
  producers/consumers impact) with semver-bump enforcement and
  `--fail-on` thresholds.
- `covenant init` — starter contract.
- Library API: `CompiledContract`, row engine, Arrow `RecordBatch`
  validator (`engine::arrow::validate_batch`) with cross-batch uniqueness.
