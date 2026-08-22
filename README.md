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

```rust
use covenant::{compile::CompiledContract, spec::Contract};
use covenant::engine::{arrow::validate_batch, UniqueTracker};
use covenant::report::Collector;

let contract = CompiledContract::compile(&Contract::from_path(std::path::Path::new("orders.yaml"))?)?;
let model = contract.resolve_model(None)?;
let mut collector = Collector::new(contract.policy.sample_violations);
let mut unique = UniqueTracker::new(model);
let mut rows = 0;
for batch in reader {                       // any RecordBatch source
    rows += validate_batch(model, &batch?, rows, Some(&mut unique), &mut collector);
}
```

The contract compiles once (regexes built, allowed-sets hashed); per-batch
validation is a columnar pass with per-(field, rule) sample caps so a
million-row disaster still produces a bounded, readable report.

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
| 1 | the *subject* violates: data breaks the contract, the diff is breaking, or (`validate`) the contract has error-level lint findings |
| 2 | the *run* failed (bad flags, unreadable/unparseable file; for `check`/`gate`/`diff`, a contract too broken to enforce) |

## Repo layout

```text
src/spec.rs        contract document + linting        src/diff.rs      breaking-change classifier
src/compile.rs     spec → hot-path validators         src/consumers.rs consumer manifests + blast radius
src/engine/row.rs  JSON-record engine (NDJSON/gate)   src/gate.rs      stream interceptor
src/engine/arrow.rs columnar engine (CSV/Parquet/lib) src/cli.rs       the `covenant` CLI
src/sources.rs     file readers driving the engines   src/report.rs    violations + reports
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
- **No native Kafka client.** The gate is a pipe; use `kcat` or any other
  transport that can pipe.
- **No catalog, no lineage, no discovery UI, and no freshness or SLA
  monitoring.** Covenant enforces contracts at a boundary; it does not
  describe your estate or watch it over time.

Everything above is a known gap rather than a hidden one. `covenant validate`
refuses contracts it cannot enforce, which is the same principle applied to
the spec itself.
