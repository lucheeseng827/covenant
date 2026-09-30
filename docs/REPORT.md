# The report: `covenant-report/v1`

`check`, `gate` and `diff` write the run's verdict with `--report-json <path>`, and the Python
face's `Report` writes the same document. It is a JSON document that other tools can keep: a
CI artifact, a catalog, a service that records runs. Every tool reads the same document, so a
verdict means the same thing wherever it is read. The terminal output and the exit codes do not
change. `--report-to <url>` also sends it, `covenant push` sends documents already written
([Sending it](#sending-it)), and `--openlineage <url>` sends the run to a lineage service as
OpenLineage events ([OpenLineage](#openlineage)).

```bash
covenant check orders.ndjson -c orders.yaml --report-json covenant-report.json
covenant check orders.ndjson -c orders.yaml --report-json r.json --report-samples none
covenant gate -c orders.yaml --dlq dlq.ndjson --report-json gate-report.json < in.ndjson > out.ndjson
covenant gate -c orders.yaml --brokers b1:9092 --from orders.raw --to orders.clean --report-json gate-report.json
covenant diff orders.yaml orders_v2.yaml --consumers consumers/ --report-json diff-report.json
```

```python
report = covenant_data.check(df, "orders.yaml")
report.write_report("covenant-report.json")      # report.report_document() gives the dict
```

The JSON Schema is [`schema/covenant-report.v1.json`](../schema/covenant-report.v1.json). The
golden files in [`tests/golden/report/`](../tests/golden/report/) are real documents, one per
kind, pinned by the tests, and every document the tests write is validated against the schema.

## Every document

| Field | What it holds |
|---|---|
| `report` | The revision key: `1`. |
| `kind` | What ran: `check` (data checked against a contract, by the command or the Python face), `gate` (a stream held to a contract record by record) or `diff` (two versions of a contract compared). |
| `verdict` | `pass` (exit 0) or `fail` (exit 1). For `check` and `gate`, `fail` is over the violation budget, and `warn` is over it under `on_violation: warn`: reported, not failed (exit 0). For `diff`, `fail` is a change at or above `--fail-on`. |
| `engine` | `name` (`covenant`) and `version`. |
| `run.plane` | What produced it: a command is `ci` when a CI provider was detected and `cli` otherwise; the stream gate is `gate`; the Python face is `python`. The gate and the Python face keep their plane in CI, and name the CI run in `run.ci`. |
| `run.started_at` | When the run started: RFC 3339, UTC, milliseconds. For the Python face, when the check started. |
| `run.duration_ms` | How long it took. |
| `run.ci` | In CI only: `provider` (`github-actions`, `gitlab-ci`, or `unknown` for a CI that sets only `CI=true`), and as far as the provider says: `repository`, `sha`, `ref` (always the full ref: `refs/heads/…`, `refs/tags/…`, `refs/pull/…`) and `run_url`. |

## `check` and `gate`: data held to a contract

| Field | What it holds |
|---|---|
| `budget` | The run's violation budget. |
| `violations` | Violations across every source of the run; the verdict compares this with the budget. |
| `sample_mode` | `masked`, `hashed` or `none`: what the samples below may say ([Samples](#samples)). |
| `sample_key` | `hashed` only: the id of the key the hashes were made with. Hashes compare only under one key. |
| `checks[]` | One entry per source, in the order the run was given them. A gate has one: its stream, named `stdin`, or the Kafka gate's topic, named `kafka://<first bootstrap server>/<topic>`. |
| `checks[].contract_id`, `contract_version`, `owner`, `model` | The contract and model enforced. |
| `checks[].source` | The source as the run named it. |
| `checks[].as_consumer` | Present when only this consumer's declared fields were checked, with strict mode off (`--as-consumer`): a scoped verdict, not a full one. |
| `checks[].rows`, `violations` | Rows read and violations found in this source. |
| `checks[].per_rule[]` | The exact count per `field` and `rule`. `field` is empty for record-level rules. |
| `checks[].samples[]` | Where sampled violations were: `field` (absent for record-level findings), `rule` and `row` (0-based; absent for schema-level findings), and the value without the value: its `type` and `length`, and under `hashed` its `hash` ([Samples](#samples)). Capped per field and rule by the contract's `sample_violations`. A gate's `row` is the row of its dead letter, so a sample joins the dead letter that holds the record. |
| `checks[].unenforced[]` | Present when the run skipped contract rules (`--allow-unenforced`): each one's `path`, `rule` and `reason`. The verdict covers the rest. |
| `gate` | `gate` only. What became of the records: `passed` (went on, including those passed under `on_violation: warn`), `blocked` (withheld and dead-lettered) and `warned` (broke the contract and went on under `warn`). `passed` and `blocked` add up to the rows. The Kafka gate adds `tombstones`: records with no value, the delete markers of a compacted topic, which went on unjudged and are not among the rows. |

Rule names are the stable identifiers the terminal output and dead-letter envelopes use
(`required_missing`, `pattern`, `allowed`, `unique`, …). New rules arrive in minor releases,
so read a name you do not know as a violation of a rule you do not know.

## Samples

A sample says where a violation was and what kind of value broke the rule, never the value.

| `--report-samples` | Python `samples=` | A sample holds |
|---|---|---|
| `masked` (the default) | `"masked"` | `field`, `rule`, `row`, and the value's `type` and `length` |
| `hashed` | `"hashed"` | the same, and a keyed `hash` of the value |
| `none` | `"none"` | nothing: there are no samples, only the counts |

`type` is the value's type as its source held it: `null`, `boolean`, `integer`, `number`,
`string`, `array` or `object`, and `timestamp`, `date` or `binary` for an Arrow source's
(CSV, Parquet) native temporal and binary columns. `length` is a string's characters, an
array's items or an object's keys, and is absent for other types. A finding without a value (a
missing field or column, a line that is not JSON) has neither.

`hash` lets two documents say "the same bad value" without either saying what it was: under
one key, the same value has the same hash in every run and every format (NDJSON, CSV,
Parquet). It is the first 16 bytes, in hex, of the HMAC-SHA256, under the key, of the SHA-256
of the value's canonical form: the type's name, a zero byte, then the value (a string's own
text; an integer in decimal; a number as JSON writes it, `-0` as `0`; `true` or `false`;
`null`; an array or object as compact JSON). It is keyed because a plain hash of a value with
few candidates, a currency or a status, is the value to anyone who hashes the candidates.

The key comes from `COVENANT_SAMPLE_KEY`, at least 16 bytes (`openssl rand -hex 32` makes
one), and never from the command line. A run asked for hashed samples without a key stops
before it starts (exit 2); in Python, the call raises `CovenantError`. The key is never
written: the document's `sample_key` is its id, the first 8 bytes in hex of the SHA-256 of
`covenant-report/v1 sample key`, a zero byte and the key, and hashes compare only between
documents with the same `sample_key`. Keep the key as you would a password: whoever holds it
can test a guessed value against a hash.

A value's `type` and `length` say something about it, and for a field with few possible
values (a three-letter currency) they narrow it down. `none` keeps the counts only.

## The Kafka gate

`gate --brokers … --report-json` writes the document when the run ends: on SIGINT or SIGTERM,
or with `--exit-at-end` once every assigned partition is read to its end (a second signal
stops the gate at once, and nothing is written). As for the stream
gate, the document's directory is checked before the first record. The source is the topic,
named `kafka://<first bootstrap server>/<topic>` (a listener prefix such as `SASL_SSL://`
dropped), `rows` counts the records judged, and `gate.tombstones` counts those with no value,
which go on unjudged. A sample's `row` is its dead letter's `row`.

## `diff`: a contract change

| Field | What it holds |
|---|---|
| `diff.contract_id` | The proposed contract's id. |
| `diff.old_version`, `new_version` | The two versions compared. |
| `diff.fail_on` | The threshold the verdict was decided by: `never`, `any`, `risky`, `breaking` or `breaking-with-consumers`. |
| `diff.max_severity` | The worst change's severity (`info`, `risky` or `breaking`); absent when nothing changed. |
| `diff.changes[]` | Each change's `severity`, `impact` (`producers`, `consumers` or `both`), `path` in the contract and `message`. |
| `diff.consumer_impact` | With `--consumers`: the manifests read, the consumers of this contract, each `impacted` consumer (worst severity, the declared fields and the change paths that hit it) and the `unaffected` ones. |
| `diff.unenforced[]` | Rules on either side that were not compared (`--allow-unenforced`). |

A diff compares contracts, which are metadata, so nothing in it is masked: its messages quote
the contracts, never data.

## Sending it

```bash
export COVENANT_TOKEN=…     # sent as Authorization: Bearer
covenant check orders.ndjson -c orders.yaml --report-to https://ingest.example.com/v1/runs
covenant push covenant-report.json gate-report.json --to https://ingest.example.com/v1/runs
```

`--report-to <url>`, on `check`, `gate` and `diff`, POSTs the run's document as
`application/json`, byte for byte what `--report-json` writes. The run's exit code stands
whatever becomes of it: a document that cannot be sent is one warning on stderr. `covenant push
<file>… --to <url>` sends documents already written, from a machine that may send when the
run's may not. It sends what it can, and exits 2 if a file was not delivered or is not a
`covenant-report/v1` document (which it does not send).

- **The token** comes from `COVENANT_TOKEN`, when it is set, and goes in an
  `Authorization: Bearer` header. It is never read from the command line and never printed.
- **The URL** is checked before the run starts (exit 2): `https` anywhere; plain `http` only
  to this machine (`localhost`, `127.0.0.1`, `::1`), since the token and the document would
  otherwise cross the network in the clear; and no credentials in it, since a URL is argv.
- **The server's certificate** is checked against the Mozilla root certificates compiled into
  the binary, not the system's trust store: an endpoint behind a private CA is refused.
- A proxy the environment names (`HTTPS_PROXY`, `ALL_PROXY`, with `NO_PROXY`) is used off this
  machine and never for it. A redirect is not followed, since it could lead where the rule
  above would not: the document counts as not delivered. A connection gets 10 seconds, and the
  whole request 30.
- The stream gate sends when its stream ends; `gate --subprocess` does not send, since its
  stderr is the reply channel.
- Sending is the `send` feature, on by default. A WASI build has no sockets, and cannot send.

## OpenLineage

```bash
export OPENLINEAGE_API_KEY=…     # sent as Authorization: Bearer, when it is set
covenant check orders.ndjson -c orders.yaml --openlineage http://localhost:5000/api/v1/lineage
covenant gate -c orders.yaml --brokers b1:9092 --from orders.raw --to orders.clean \
  --openlineage https://lineage.example.com/api/v1/lineage
```

`--openlineage <url>` sends the run to an OpenLineage endpoint (Marquez's `/api/v1/lineage`,
or any service that takes the standard's HTTP events), so a lineage service shows the verdict
next to the dataset it was about. It is on `check` and the Kafka gate: a stream on stdin has
no name to show a verdict next to, so the stream gate refuses it (exit 2), and a diff is about
a contract, not a dataset. The URL is held to the rule above, the key comes from
`OPENLINEAGE_API_KEY`, and events that cannot be sent are one warning.

A run is two OpenLineage 2-0-2 `RunEvent`s, sent when it ends: START, timed at the run's
start, then COMPLETE, timed at its end.

| Field | What it holds |
|---|---|
| `run.runId` | A UUIDv7: the run's start in milliseconds, then bits of the SHA-256 of its report document. The same run sent twice is one run. |
| `job` | Namespace `covenant`; name `<kind>.<contract id>.<model>`, such as `check.orders.orders`. |
| `inputs[]` | Each source, named as OpenLineage names datasets: a file is namespace `file` and its absolute path; a topic is namespace `kafka://<host:port>` and its name. |
| `inputs[].inputFacets.dataQualityAssertions` | COMPLETE only; the standard's facet, revision 1-0-2. One assertion per rule the model declares, in contract order, by `column`: per field, `required_missing` when it is required, `null_not_allowed` when it is not nullable, `type_mismatch`, and each constraint it declares; then, for a strict model, `unexpected_field` for the whole dataset. Then any other rule the run reported, failed. `success` says whether the rule held; `severity` is `error`, or `warn` under `on_violation: warn`. |
| `inputs[].inputFacets.dataQualityMetrics` | COMPLETE only; revision 1-0-3: `rowCount`, the rows read. |

The events say less than the document: no values, no samples and no counts per rule. The
tests hold them to the published OpenLineage schemas, copied in `tests/openlineage/`.

**In a catalog.** CI also holds them to a running Marquez 0.51.1, the OpenLineage reference
implementation (the `marquez` job, [`tests/marquez_test.rs`](../tests/marquez_test.rs)). The run
is the job `check.<contract id>.<model>` in the namespace `covenant`. The dataset it read shows
its quality as the assertions passing and failing, and lists each failing rule by column. A
later run's verdict replaces an earlier one. A topic the Kafka gate read shows the same way,
under the namespace `kafka://<host:port>`. Covenant sends no schema facet: it reads a dataset,
and the dataset's schema is for the job that writes it to publish.

## What never leaves the machine

The document is made to be kept and sent elsewhere, so it holds what the run found, not the
data it read:

- **No values.** A sample says where a violation was and what kind of value it was: the
  field, the rule, the row, the value's type and length, and under `hashed` a keyed hash. It
  never carries the value, nor the message, because messages quote the value. The schema
  refuses a sample with a `value` or a `message`. The values stay in the terminal output,
  in `--format json` and the Python `Report`, and, for the gate, in the dead letters: none of
  them is meant to leave the machine.
- **`--report-samples none`** (`samples="none"` in Python) drops the samples too, and keeps the
  counts only.
- **No network call unless one is asked for.** `--report-to`, `push` and `--openlineage` are
  the only ones, and each goes only to the URL it is given.
- **The environment variables read** are the CI provider's own, and only when a document is
  made: `GITHUB_ACTIONS`, `GITHUB_REPOSITORY`, `GITHUB_SHA`, `GITHUB_REF`,
  `GITHUB_SERVER_URL` and `GITHUB_RUN_ID`; `GITLAB_CI`, `CI_PROJECT_PATH`, `CI_COMMIT_SHA`,
  `CI_COMMIT_TAG`, `CI_COMMIT_BRANCH` and `CI_JOB_URL`; and `CI`. The rest, each only when
  its feature is used: `COVENANT_SAMPLE_KEY` for hashed samples, `COVENANT_TOKEN` for
  sending, `OPENLINEAGE_API_KEY` for `--openlineage`, and the proxy variables for a URL off
  this machine.

A report that cannot be written (a missing directory, no permission) fails the run with exit
2, like any other output the run was asked for and could not produce; in Python,
`write_report` raises `CovenantError`. The gate writes its document when its stream ends, and
checks the document's directory before it reads the first record, since a stream can run for
hours. A run that fails before it reaches a verdict (exit 2: a contract that does not load, a
source that cannot be read) writes no report, because there is no verdict to report.

## Compatibility

The document is a format with its own revision key, versioned apart from the code
([`API.md`](API.md)). Within revision 1, fields, kinds and planes are added and never renamed or
removed: read it ignoring fields you do not know, skip a kind you do not know, and read a plane
you do not know as just that. The schema does not forbid unknown fields, for the same reason. A
change that would break a reader is revision 2, and a release that writes it says so.

The OpenLineage events follow OpenLineage's versions, not this one: each event's `schemaURL`
and each facet's `_schemaURL` name the revision it is written to.
