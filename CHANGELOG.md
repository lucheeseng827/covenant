# Changelog

## Unreleased (targeting 0.1.0)

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
