<img src="docs/images/covenant-logo.svg" alt="" width="72">

# Covenant — data-contract enforcement runtime

> Most data-contract tools are an honor system with a search interface.
> Covenant is the enforcement half: it *runs* the contract, at the boundary,
> fast enough to sit in the data path.

**Status:** MVP (v0.1.0) · **License:** Apache-2.0

## The problem

Data-contract tooling in 2026 stops at *describing* contracts. The YAML gets
authored, published to a catalog, linted in CI — and then nothing ever
executes the assertions where the data actually flows. Producers drift,
consumers break, and the contract is discovered to have been fiction all
along. Catalogs describe. Spec tools lint. Scan-based quality frameworks
observe after the load, from outside the path the data actually took.

Covenant is the missing piece: **one compiled contract, enforced at every
boundary the data crosses**, as a single static Rust binary with negligible
per-record latency.

## Three enforcement points

```text
              ┌─────────────────────────────────────────────┐
              │            covenant.yaml (contract)         │
              └──────────────────────┬──────────────────────┘
                             compile once
          ┌──────────────────────────┼──────────────────────────┐
          ▼                          ▼                          ▼
   covenant check             covenant gate            covenant::engine::arrow
   (CI file gate:             (stream interceptor:     (library API: validate
   NDJSON/CSV/Parquet,        stdin → clean stdout,    Arrow RecordBatches
   exit 1 on violation)       violations → DLQ)        in-process)
```

Plus the merge-time gate: `covenant diff old.yaml new.yaml` classifies every
contract change as **breaking / risky / info**, tags whether **producers** or
**consumers** feel it, and enforces the matching semver bump — exit code 1
blocks the PR.

## Blast radius: who breaks before you merge

Classification says *what* changed; consumer manifests say *who breaks*.
Consumers declare what they actually read in a small `consumer: 1` file
(kept next to their own code), and the merge gate intersects the diff with
those declarations:

```yaml
# consumers/finance_daily_rollup.yaml
consumer: 1
id: finance_daily_rollup
owner: finance-eng@acme.io
consumes:
  - contract: orders
    fields: [order_id, customer_email, currency]   # omit fields = whole model
```

```bash
covenant diff main/orders.yaml pr/orders.yaml --consumers consumers/
```

```text
impacted consumers (4 manifests, 4 consume orders):
  [breaking] finance_daily_rollup (finance-eng@acme.io) via currency, customer_email — models.orders.fields.currency, models.orders.fields.customer_email
  [risky   ] looker_revenue (analytics@acme.io) via currency — models.orders.fields.currency
  unaffected: ml_churn_features, ops_alerting
```

(That is the verbatim output for the bundled demo pair: `customer_email`
removed breaks the rollup, the widened `currency` enum flags both dashboards
as risky, and the two consumers that never read those fields are provably
untouched.)

The matching is deliberately honest: producer-side changes (a tightened
bound, a new required field) never hit readers; the semver-bump finding is
process discipline, not a shape change, and is excluded. And
`--fail-on breaking-with-consumers` turns this into the adoption-friendly
gate — block the merge only when a *declared* consumer actually breaks,
report everything else. Manifests are plain files kept next to the code that
reads them, so the whole graph is reviewable in the same pull request as the
change it constrains.

### The consumer side of the loop

A graph is only as good as its declarations, so manifests are *enforced
too* — in the consumer's own CI:

```bash
# Is my manifest still true? Declared fields exist, pins aren't stale.
covenant consumer-check consumers/ -c orders.yaml     # exit 1 when stale

# Does the data I read conform? Only MY fields are checked; dirt in
# fields I never touch (and strict-mode noise) is the producer's problem.
covenant check sample.ndjson -c orders.yaml --as-consumer consumers/me.yaml
```

`consumer-check` verifies every declared field is still enforced by the
contract, and grades the optional `verified: <semver>` pin: a **breaking**
bump past the pin is an error (re-review, then re-pin) — that's a major
bump, or a *minor* bump while the contract is still 0.x, since pre-1.0
minors are breaking by semver. Non-breaking drift is a warning. A stale manifest can never scope
a data check — `--as-consumer` refuses it outright rather than validating
fields that no longer exist. The served API exposes the same verdicts at
`POST /v1/consumers/verify`.

## Quickstart

```bash
# start from data you already have: draft a contract from a sample
covenant infer exports/orders.parquet --out orders.yaml
# ...or scaffold an empty one
covenant init orders.yaml && covenant validate orders.yaml

# CI: gate a file (exit 0 clean / 1 violated / 2 error)
covenant check exports/orders.parquet -c orders.yaml
covenant check dumps/*.ndjson -c orders.yaml --format json

# streaming: the Kafka-SMT-equivalent, transport-agnostic
kcat -C -t orders_raw -e \
  | covenant gate -c orders.yaml --dlq /var/log/orders.dlq.ndjson \
  | kcat -P -t orders_validated
# ...or topic to topic, with the `kafka` feature (see below)
covenant gate -c orders.yaml --brokers localhost:9092 \
  --from orders_raw --to orders_validated --dlq-topic orders_dlq

# merge gate: block breaking contract changes in CI
covenant diff main/orders.yaml pr/orders.yaml --fail-on breaking
```

A failing check names the rule, the row, the value, and the owning team:

```text
FAIL  examples/data/orders_bad.ndjson  [orders v1.2.0, model orders]
  rows checked: 6   violations: 9
  contract owner: data-platform@acme.io
  by rule:
    order_id                 pattern                × 1
    amount_cents             min                    × 1
    currency                 allowed                × 1
    order_id                 unique                 × 1
    ...
  samples:
    [row 1] row 1: field "amount_cents" value -50 is below min 0
    [row 1] row 1: field "currency" value "BTC" not in allowed set [EUR, GBP, USD]
```

## Starting from data you already have (`covenant infer`)

Nobody writes their first data contract from a blank page. `infer` reads a
sample of real records — NDJSON, CSV, or Parquet — and drafts one:

```bash
covenant infer exports/orders.parquet --out orders.yaml
covenant validate orders.yaml          # clean by construction
covenant check exports/orders.parquet -c orders.yaml   # and it passes
```

The draft is *honest about its evidence*, which is the whole point. A sample
proves what appeared; it can never prove what will not. So types, presence,
and nullability — the things the sample really does settle — are emitted as
rules, while every guess from the window is emitted with a `# confirm:` note
naming what it was inferred from:

```yaml
      order_id:
        type: string
        required: true
        # confirm: every sampled value matched ^ord_[0-9]{4}$ — verify it holds
        #          for values the sample never saw
        pattern: "^ord_[0-9]{4}$"
        # confirm: all 60 sampled values were distinct — consider `unique: true`
        #          if this is a key (not drafted: a sample cannot prove uniqueness)
      status:
        type: string
        required: true
        # confirm: 3 distinct values across 60 samples — a sample cannot prove
        #          the set is closed; drop this if new values are legal
        allowed: [cancelled, pending, shipped]
```

Deliberate refusals: `unique` is never drafted (a wrong one fails clean data
in production), mixed-type columns widen to `string` and say so, nested
objects are flagged rather than silently flattened, and an all-null column
admits it is a guess. The version starts at `0.1.0` and `owner:` is left as a
TODO, because a drafted contract is not yet a promise anyone should pin to.

| Flag | Default | What it changes |
|---|---|---|
| `--sample N` | 10000 | Records scanned; `0` reads everything |
| `--max-enum N` | 12 | Maximum distinct values considered for `allowed:` — the values must also repeat across the sample; `0` never drafts enums |
| `--no-ranges` | off | Skip observed `min`/`max` and length rules |
| `--id` / `--model` | file stem | Name the contract and its model |
| `--format json` | yaml | Contract plus the notes as data, for tooling |

## The contract

Covenant also reads **ODCS v3** (Open Data Contract Standard) contracts directly: point `-c`
at the `.odcs.yaml` file. Each ODCS rule is either enforced with the same meaning or refused
by name — never skipped quietly. [`docs/ODCS.md`](docs/ODCS.md) has the mapping and the
`--allow-unenforced` escape hatch. The native format:

```yaml
covenant: 1
id: orders
version: 1.2.0            # semver — `covenant diff` enforces the bump
owner: data-platform@acme.io

models:
  orders:
    strict: true          # undeclared fields are violations
    fields:
      order_id:     { type: string, required: true, unique: true,
                      pattern: "^ord_[a-z0-9]{12}$" }
      amount_cents: { type: integer, required: true, min: 0 }
      currency:     { type: string, required: true, allowed: [USD, EUR, GBP] }
      customer_email: { type: string, format: email, nullable: true }
      created_at:   { type: timestamp, required: true }

policy:
  on_violation: block     # block | warn
  max_violations: 0       # tolerated budget before a check fails
  sample_violations: 10   # examples kept per (field, rule); counts stay exact
```

Types: `string · integer · float · boolean · timestamp (RFC 3339) · date ·
uuid`. Constraints: `required · nullable · unique · pattern · min/max ·
min_length/max_length · allowed · format (email|uri)`. Everything in the spec
is something the runtime actually enforces — anything it can't check at the
boundary is deliberately not in the schema. The one advisory marker is
`deprecated: "<note>"` (or `true`): it never changes validation, but the
diff classifier reports the transition and `consumer-check` warns every
reader that still declares the field.

One documented representation caveat: columnar formats (CSV/Parquet/Arrow)
cannot distinguish "key absent" from "explicit null", so a null in an
*optional* (`required: false`, `nullable: false`) field's column is treated
as absent and passes, where the NDJSON path — which can see the difference —
rejects the explicit null. `required` fields behave identically everywhere.
See ARCHITECTURE.md's null-semantics table.

## Embedding (Arrow-native pipelines)

The Arrow engine and the CSV and Parquet readers are the `arrow` feature, which is on by default.
An embedder that only judges JSON records, such as a stream processor's plugin or a
WebAssembly module, can depend on the crate with `default-features = false` and link no Arrow
at all. In such a build `covenant check` refuses CSV and Parquet with a message that names the
feature.


```rust
use covenant::prelude::*;

let contract = CompiledContract::compile(&Contract::from_path("orders.yaml".as_ref())?)?;
let model = contract.resolve_model(None)?;
let mut collector = Collector::new(contract.policy.sample_violations);
let mut unique = UniqueTracker::new(model);  // uniqueness spans the source
let mut schema = SchemaFindings::new();      // a schema problem counts once per source
let mut rows = 0;
for batch in reader {                        // any RecordBatch source
    rows += validate_source_batch(model, &batch?, rows, Some(&mut unique), &mut schema, &mut collector);
}
```

The contract compiles once (regexes built, allowed-sets hashed); per-batch
validation is a columnar pass with per-(field, rule) sample caps so a
million-row disaster still produces a bounded, readable report. A schema
problem — a required column missing, a column of the wrong type — is a fact
about the source, so it counts once however the source is batched; for a
single batch, `validate_batch(model, &batch, 0, Some(&mut unique), &mut collector)`
checks it as a whole source.

`covenant::prelude` is the stable surface: what it exports is covered by the change policy in
[`docs/API.md`](docs/API.md), and a test pins every signature in it. The package is
`covenant-data`; the library it provides is `covenant`.

## Python (`covenant_data`)

The same engine as a Python module, for pipelines that hold their data in frames:

```python
import covenant_data

report = covenant_data.check(df, "orders.yaml")   # polars, pyarrow, DuckDB: zero-copy
assert report.passed, report                     # the report names each rule, row and value
```

Anything that exports Arrow through the PyCapsule interface is checked batch by batch, without
a copy. A list of dicts is checked exactly as `covenant check` checks an NDJSON file, and a path
as a file. `covenant_data.Contract("orders.yaml")` compiles a contract once for many checks;
`covenant_data.testing.assert_conforms(df, contract)` fails a test with the report as its
message; and the wheel installs the `covenant` command as well. On a 4-core machine, a 10M-row
polars frame with a pattern, bounds, an allowed set and a timestamp checks in about 0.9 s;
`unique` over 10M distinct strings brings it to about 6.5 s, most of it holding every key.

It builds from `python/` with maturin as one abi3 wheel for CPython 3.9 and later; it is not on
PyPI yet.

## WASM (`covenant.wasm`)

The same command builds for WebAssembly, for sandboxes, edge runtimes and any host with a WASI
runtime:

```bash
cargo build --release --target wasm32-wasip1          # target/wasm32-wasip1/release/covenant.wasm
wasmtime --dir . target/wasm32-wasip1/release/covenant.wasm check orders.ndjson -c orders.yaml
```

Every command works as the native one does, reading and writing files in the directories the
runtime grants it. CI holds it to the native build byte for byte — the example data, a CSV, the
stream gate, `diff`, `export`, the conformance vectors and a profile file — running it under
Node's built-in WASI (`.github/scripts/wasm-run.mjs`, which also runs it without a WASI
runtime installed).

From JavaScript, `js/covenant.mjs` runs the same build over an in-memory filesystem, in a
browser or in Node, with no dependencies: `await covenant.check({ contract, data })` hands back
the JSON report, and nothing touches a disk or leaves the page. `js/playground.html` is a page
on it: paste a contract and some CSV or NDJSON, and get the command's report. CI holds the JS
API to the native build the same way (`js/golden.mjs`); [`js/README.md`](js/README.md) has the
API and how to serve the playground.

## Inside a stream processor (`gate --subprocess`)

A stream processor that runs a command for its messages can run the gate itself.
`covenant gate --subprocess` answers every line with one line, flushed at once: the record on
stdout if it goes on, its dead-letter envelope on stderr if it is withheld, a blank line for a
blank one. That is the protocol of Redpanda Connect's `subprocess` processor, which replaces
the message with a stdout reply and marks it failed on a stderr reply, so the gate's verdict
routes the message:

```yaml
input:
  kafka_franz:
    seed_brokers: [localhost:9092]
    topics: [orders_raw]
    consumer_group: covenant

pipeline:
  threads: 1                    # one ordered stream into the gate; see below
  processors:
    - subprocess:
        name: covenant
        args: [gate, --subprocess, --contract, orders.yaml]

output:
  switch:
    cases:
      - check: errored()        # withheld: error() is the dead-letter envelope
        output:
          kafka_franz:
            seed_brokers: [localhost:9092]
            topic: orders_dlq
      - output:
          kafka_franz:
            seed_brokers: [localhost:9092]
            topic: orders_validated
```

One gate process serves the pipeline, so `unique` spans the stream. With several pipeline
threads, records reach it in the order the threads deliver them, which decides which of two
duplicates counts as the first; `threads: 1` keeps the input's order. Under
`on_violation: warn` a record's only reply is the record itself, and its dead letter goes to
`--dlq` when one is given. The gate is not restarted per message: it keeps its state for as
long as the processor keeps it running.

## Topic to topic (`gate --brokers`, the `kafka` feature)

Built with the `kafka` feature, the gate reads and writes Kafka topics itself. The feature
compiles librdkafka from source, so it needs a C compiler, `make` and `perl`, and it is off by
default:

```bash
cargo install covenant-data --features kafka

covenant gate -c orders.yaml --brokers localhost:9092 \
  --from orders_raw --to orders_validated --dlq-topic orders_dlq
```

A record that goes on is produced to `--to` as it arrived: key, value, headers and timestamp,
in the partition a Java producer would have put its key in. A dead letter keeps the record's
key and headers and adds three of its own, `covenant.source.topic`, `covenant.source.partition`
and `covenant.source.offset`; without `--dlq-topic`, dead letters go to `--dlq` or stderr as
usual. A tombstone (a record with no value) goes on unjudged: a delete is not a record the
contract describes.

Delivery is at-least-once. The gate commits under its consumer group (`--group`, by default
`covenant-gate.<contract id>.<topic>`) only offsets whose records the brokers have acknowledged:
every second (`--commit-interval-ms`), before a rebalance takes partitions away, and when
SIGINT or SIGTERM stops it. A second signal stops it at once. If the brokers refuse a record,
for example a dead letter larger than the DLQ topic accepts, the gate stops with exit 2 and
commits nothing from that point on. After a restart, those records are judged again. A new
group starts from the earliest offset. The gate never creates topics, so all three must exist.

Connection settings are librdkafka properties, given as `-X key=value` or in a `--kafka-config`
file, which keeps credentials off the command line. TLS and SASL (PLAIN, SCRAM, OAUTHBEARER) are
built in, and so are gzip, snappy, lz4 and zstd. `--exit-at-end` stops the gate once every
assigned partition is read to its end, for a backfill or a test, with the usual exit code.
`unique` covers what one gate process reads. Run one gate per group to hold it across the whole
topic; with several gates in a group, each one checks only its own partitions.

## Inside the broker (a Redpanda Data Transform)

[`integrations/redpanda-transform`](integrations/redpanda-transform/README.md) builds the gate as
a Redpanda Data Transform: WebAssembly that the broker runs on every record written to a topic.
The contract is compiled in at build time, and a contract the transform could not enforce
exactly fails the build. Each record is judged by the same `RecordGate`. A record that keeps
the contract goes to the first output topic unchanged; a record that breaks it becomes a dead
letter, with its key and headers, on the topic named by `COVENANT_DLQ_TOPIC`; a tombstone goes
on unjudged:

```bash
COVENANT_CONTRACT=orders.yaml COVENANT_NO_UNIQUE=1 rpk transform build
rpk transform deploy --input-topic orders_raw --output-topic orders_validated \
  --output-topic orders_dlq --var COVENANT_DLQ_TOPIC=orders_dlq
```

A transform keeps its state per partition, so it cannot hold `unique` across a topic. The build
refuses a model that declares `unique` fields until `COVENANT_NO_UNIQUE=1` says to skip them. CI
deploys the module into Redpanda and requires the same records and dead letters as
`covenant gate --no-unique` on the same input.

## On the JVM (Kafka Connect)

[`integrations/jvm`](integrations/jvm/README.md) has two parts: the gate as a Java library, and
a Kafka Connect transformation built on it. Neither ships native code. The engine's own
WebAssembly build runs on Chicory, a WebAssembly runtime written in Java, so a record gets the
same verdict and the same dead letter as in `covenant gate`:

```java
try (Gate gate = Gate.open(Path.of("orders.yaml"), null, UniqueKeys.SKIP)) {
  if (gate.judge(recordJson) != Verdict.PASS) deadLetters.send(gate.deadLetter());
}
```

To use the transformation, put its jar on a worker's `plugin.path` and set
`transforms.covenant.type=net.mancube.covenant.connect.CovenantGate` and
`transforms.covenant.contract=/etc/covenant/orders.yaml`. A blocked record then fails with its
dead letter as the error message. With `errors.tolerance=all`, a sink connector's dead letter
queue receives the record, and the dead letter arrives in its `__connect.errors.exception.message`
header. CI runs a real Connect worker and requires its sink and its dead letter queue to match
`covenant gate` on the same records.

The same library runs in a Kafka proxy. A Kroxylicious filter judges every record of a
produce request before it reaches a broker, so a batch that breaks the contract is refused to
its producer with the record's dead letter as the error. Nothing needs to change in the
producers, whatever language they are written in.

## In Arroyo SQL

[`integrations/arroyo`](integrations/arroyo/README.md) has two Rust UDFs for Arroyo that link the
engine itself. `covenant_verdict(value)` returns `'pass'`, `'block'` or `'warn'`, and
`covenant_dead_letter(value)` returns the dead letter `covenant gate` writes, or `NULL`. A
streaming SQL pipeline can route with them:

```sql
INSERT INTO orders_validated SELECT value FROM orders_raw WHERE covenant_verdict(value) <> 'block';
INSERT INTO orders_dlq SELECT covenant_dead_letter(value) AS value FROM orders_raw
  WHERE covenant_dead_letter(value) IS NOT NULL;
```

`render.py` compiles a contract into the UDF sources, after checking it with the engine. CI
registers them in a real Arroyo, which compiles them itself. It then runs this pipeline and
requires the same records and dead letters as `covenant gate`.

## Profiles and drift: monitoring from the check you already run

A check can also leave a **profile** of what it read — presence, nulls, distinct values,
ranges and distributions per field, in the same read — and `covenant drift` compares two of
them. It catches what a contract does not forbid and a consumer still notices:

```bash
covenant check exports/orders.parquet -c orders.yaml --profile today.json
covenant profile merge history/*.json -o baseline.json   # runs, partitions, days
covenant drift baseline.json today.json                  # exit 1 on drift
```

```text
DRIFT orders/orders — baseline v1.2.0, 3 runs, 12,000 rows · current v1.2.0, 1 run, 1,500 rows
  (model)         rows per run fell from 4,000 to 1,500 (−62.5%; threshold ±50%)
  amount_cents    values shifted: median 4,016 → 9,024, p95 5,984 → 11,072 (PSI 7.94 over the baseline's deciles; threshold 0.25)
  currency        "EUR" rose from 30% to 67.1% of values (Jensen–Shannon distance 0.32; threshold 0.1)
  customer_email  null rate rose from 1.9% to 21% (threshold ±5 points)
  4 findings over 6 fields
```

A profile never keeps the text of a string field: it contributes lengths and distinct counts,
and an `allowed:` field a count per value its contract lists, with anything else counted
together.
The same records in NDJSON, CSV or Parquet give the same profile, and merged profiles are
exactly the profile of all their data. [`docs/PROFILES.md`](docs/PROFILES.md) has the format,
the metrics, their accuracy and the cost.

## A report for CI, catalogs and services (`--report-json`, `--report-to`, `--openlineage`)

`check --report-json <path>` also writes the run's verdict as a document other tools can keep:
`covenant-report/v1`. It holds the verdict, the exact counts per field and rule, and where and
when the run happened. In GitHub Actions or GitLab CI it names the repository, commit, ref and
run. The terminal output and the exit code are unchanged.

```bash
covenant check orders.ndjson -c orders.yaml --report-json covenant-report.json
```

`gate` and `diff` write the same document, and so does the Python face
(`report.write_report(path)`), so every plane reports one way:

- **`gate --report-json <path>`** writes it when the stream ends: the stream's counts per field
  and rule, what became of the records (`passed`, `blocked`, `warned`), and samples whose rows
  are the dead letters' rows. The directory is checked before the first record is read. Over
  Kafka (`--brokers`) it is written when the run ends, names the topic, and counts the
  tombstones that went on unjudged.
- **`diff --report-json <path>`** writes the classified changes, the `--fail-on` verdict and,
  with `--consumers`, who each change breaks.

```json
{
  "report": 1,
  "kind": "check",
  "verdict": "fail",
  "engine": { "name": "covenant", "version": "0.1.0" },
  "run": {
    "plane": "ci",
    "started_at": "2026-09-27T17:48:41.822Z",
    "duration_ms": 41,
    "ci": { "provider": "github-actions", "repository": "acme/shop", "sha": "0123abcd",
            "ref": "refs/pull/7/merge", "run_url": "https://github.com/acme/shop/actions/runs/42" }
  },
  "budget": 0,
  "violations": 1,
  "sample_mode": "masked",
  "checks": [{
    "contract_id": "orders", "contract_version": "1.2.0", "model": "orders",
    "source": "orders.ndjson", "rows": 1500, "violations": 1,
    "per_rule": [{ "field": "currency", "rule": "allowed", "count": 1 }],
    "samples": [{ "field": "currency", "rule": "allowed", "row": 311, "type": "string", "length": 3 }]
  }]
}
```

The document is made to leave the machine, so its samples say where each violation was and
never what the value was. `--report-samples masked`, the default, keeps each sample's field,
rule and row and the value's type and length; `hashed` adds a hash of the value keyed with
`COVENANT_SAMPLE_KEY`, so two runs can say "the same bad value" without either saying what it
was; `none` keeps the counts only. The values stay in the terminal output and, for the gate, in
the dead letters.

It can leave the machine as the run ends, or later:

```bash
export COVENANT_TOKEN=…   # sent as Authorization: Bearer; never on the command line
covenant check orders.ndjson -c orders.yaml --report-to https://ingest.example.com/v1/runs
covenant push covenant-report.json --to https://ingest.example.com/v1/runs
covenant check orders.ndjson -c orders.yaml --openlineage http://localhost:5000/api/v1/lineage
```

`--report-to` (on `check`, `gate` and `diff`) POSTs the document; `push` sends documents already
written. `--openlineage` (on `check` and the Kafka gate) sends the run as OpenLineage START and
COMPLETE events whose input dataset carries each rule's result as a `dataQualityAssertions`
facet, so a lineage service such as Marquez shows the verdict next to the dataset (CI holds it
to a running Marquez: the dataset shows its passing and failing rules). A URL is
`https`, or plain `http` to this machine only, and is checked before the run starts; a run whose
document or events cannot be sent warns once and keeps its exit code. Nothing is sent unless
one of these is asked for.

[`schema/covenant-report.v1.json`](schema/covenant-report.v1.json) is the JSON Schema, and
[`docs/REPORT.md`](docs/REPORT.md) describes every field, the samples, sending and the
OpenLineage events.

## Enforcing in Postgres (`covenant export postgres`)

When the data lands in a Postgres table, the table can enforce the contract itself:

```bash
covenant export postgres -c orders.yaml --table sales.orders -o orders.sql
```

The output is a `CREATE TABLE` whose column types, `NOT NULL`, `CHECK` and `UNIQUE` constraints
reject the rows `covenant check` reports, each constraint named after its rule
(`amount_cents.min`). A rule with no exact Postgres equivalent refuses the export rather than
being approximated. CI holds the claim against a real Postgres, on the conformance vectors and
on edge values; [`docs/POSTGRES.md`](docs/POSTGRES.md) has the mapping and what "exact" covers.

## For AI agents (`covenant mcp`)

`covenant mcp` serves the checker as Model Context Protocol tools over stdio, so the agents
writing pipelines call it directly: `explain` (what a contract requires, field by field),
`check` (files, or a few inline records), `validate`, `diff` and `drift`. Register it with
your client as the command `covenant` with the argument `mcp`. A failing verdict is a
successful call; a contract that cannot be read is a tool error the agent sees.

## ODCS conformance vectors

`conformance/odcs/` pins what the Open Data Contract Standard's rules mean as small,
engine-neutral cases — a contract, a few records, the verdict the standard implies — so any
engine can be held to the same reading:

```bash
covenant conformance conformance/odcs      # exit 1 if a supported vector disagrees
```

Where the standard's text allows two readings, the case is left out of the suite and listed as
a question for the standard instead. [`docs/CONFORMANCE.md`](docs/CONFORMANCE.md) has the
format and the questions.

## The console (`covenant serve`)

An optional localhost console + JSON API, off by default so the standard
build stays a lean sync binary:

```bash
cargo build --features serve

# terminal 1 — the gate stays a plain pipe process; the extra flags write
# the file taps that light the Stream-gate and Dead-letters screens
covenant gate -c orders.yaml --dlq /var/log/orders.dlq.ndjson \
  --stats /var/run/covenant-gate-stats.json < input.ndjson > output.ndjson

# terminal 2 — console at http://127.0.0.1:8787/, reading the taps per
# request (omit the two flags for the plain console + API)
covenant serve -c orders.yaml \
  --dlq /var/log/orders.dlq.ndjson \
  --gate-stats /var/run/covenant-gate-stats.json
```

The console (`frontend/`, Vite + React + TypeScript) is a 1-to-1 port of
the Covenant Console design mock (nocturne design system), dynamically
linked to the `/v1` API through a typed client (`frontend/src/api.ts`).
Screens whose data the engine can answer are **live** (Editor lint via
`POST /v1/validate`, Check reports via `POST /v1/check`, PR gate via
`POST /v1/diff`, contract fields/policy via `GET /v1/contract`, gate
stats via `GET /v1/gate/stats`, dead letters via `GET /v1/dlq`); screens
that need fleet state (registry, history, team rollups, DLQ replay) carry
an explicit **"endpoint missing"** chip rather than rendering stale state.
The built app (`frontend/dist`, committed) is embedded
into the binary, so `cargo build --features serve` needs no Node toolchain;
without a reachable server the app runs on demo data and says so.

## Exit codes (the CI contract)

| Code | Meaning |
|------|---------|
| 0 | clean — data conforms / diff acceptable |
| 1 | the *subject* violates: data breaks the contract, the diff is breaking, (`validate`) the contract has error-level lint findings, or (`drift`) the data drifted past a threshold |
| 2 | the *run* failed (bad flags, unreadable/unparseable file; for `check`/`gate`/`diff`, a contract too broken to enforce; for `export`, a rule the engine has no exact equivalent for) |

## Repo layout

```text
src/spec.rs        contract document + linting        src/diff.rs      breaking-change classifier
src/odcs.rs        ODCS v3 reader, exact or refused   src/consumers.rs consumer manifests + blast radius
src/compile.rs     spec → hot-path validators         src/gate.rs      stream interceptor
src/engine/row.rs  JSON-record engine (NDJSON/gate)   src/profile.rs   per-field profiles
src/engine/arrow.rs columnar engine (CSV/Parquet/lib) src/sketch.rs    distinct + quantile sketches
src/sources.rs     file readers driving the engines   src/drift.rs     profile-to-profile drift
src/report.rs      violations + reports               src/mcp.rs       MCP tools over stdio
src/protocol.rs    the report document (report: 1)    schema/          its JSON Schema
src/send.rs        sending documents (HTTPS)          src/lineage.rs   OpenLineage run events
src/postgres.rs    contract → Postgres constraints    src/prelude.rs   the stable engine API
python/            the Python module (PyO3, maturin)  js/              the JS API over covenant.wasm
integrations/      the gate inside stream processors: Redpanda, the JVM (Kafka Connect, Kroxylicious), Arroyo
src/cli.rs         the `covenant` CLI                 src/kafka.rs     the gate over Kafka topics
src/conformance.rs ODCS vector runner                 conformance/odcs ODCS conformance vectors
```

See `ARCHITECTURE.md` for the engine design and the columnar null-semantics
decision, and `CHANGELOG.md` for what has shipped.

## Not yet supported

Covenant is deliberately narrow, and the boundaries are worth knowing before
you adopt it:

- **Nested fields.** Dotted paths (`payment.method`) in JSON records and Arrow
  `Struct` columns are not validated yet; only top-level fields are.
- **`decimal`.** There is no decimal contract type. Money in `integer` minor
  units (the `amount_cents` idiom) is the supported shape today.
- **Dictionary-encoded columns.** Arrow dictionary columns are not decoded; a
  contract over one is rejected as a `schema_type_mismatch` rather than silently
  skipped — the engine refuses to half-check a column it cannot iterate.
- **Uniqueness on unbounded streams.** `unique` retains every key it has seen,
  which is correct for files and bounded batches but grows without limit in a
  long-running `gate`. Keep `unique` on bounded inputs until windowed dedup
  lands.
- **Kafka only, among native transports.** Beyond stdin and stdout, the gate
  speaks Kafka itself (the `kafka` feature, above), at-least-once. For any
  other broker, pipe it or run it as a stream processor's subprocess.
- **No catalog, no lineage, no discovery UI, and no freshness or SLA
  monitoring.** Covenant enforces contracts at a boundary; it does not
  describe your estate or watch it over time.

Everything above is a known gap rather than a hidden one. `covenant validate`
refuses contracts it cannot enforce, which is the same principle applied to
the spec itself.
