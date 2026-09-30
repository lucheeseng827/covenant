# covenant (`mancube/covenant`)

**A data-contract enforcement runtime in one static binary.** Most data-contract tooling stops at describing: the YAML gets authored, cataloged and linted, and then nothing ever executes the assertions where the data actually flows. Covenant is the half that runs them — one compiled contract, enforced at every boundary the data crosses, cheap enough to sit in the path.

- **Image:** `mancube/covenant` — static musl binary on **distroless/static**, runs as **nonroot** (uid `65532`), no shell, no package manager. ~8 MB.
- **Arch:** `linux/amd64`, `linux/arm64` · **Binary inside:** `/usr/local/bin/covenant` (entrypoint)
- **Build:** pure Rust, zero Python/C/C++. Arrow, Parquet and the regex/semver stack are all pure-Rust, so the whole matrix builds without a C toolchain.
- **No network access on any code path.** No telemetry, no update check, no phone-home. There is no flag to turn off.
- **Source / full docs:** [github.com/lucheeseng827/covenant](https://github.com/lucheeseng827/covenant) · Apache-2.0

## Tags

| Tag | Notes |
|---|---|
| `latest` | newest stable release — moves on every release |
| `0.2.0` | the `covenant-report/v1` document from every plane, OpenLineage, stream processors, the Python, WASM and JS faces; signed, with build provenance |
| `0.1.0` | first release — contract spec + linter, CI file gate over NDJSON/CSV/Parquet, stream gate with dead-letter envelopes, breaking-change diff with consumer blast radius, `covenant infer` |
| `*-rc.*`, `*-alpha.*` | pre-release builds, never tagged `latest` — do not use in production |

**Pin a version in production: `mancube/covenant:0.2.0`.**

`latest` is a moving target by definition. Covenant is usually wired into a CI gate or a producer's data path, and both are places where a silent change of behaviour is expensive — a new release that classifies one more change as breaking will start failing pipelines that passed yesterday. Pin the exact tag, upgrade deliberately, and read the changelog for the release you are moving to.

For a stronger guarantee than a tag, pin the digest:

```bash
docker pull mancube/covenant:0.2.0
docker inspect --format='{{index .RepoDigests 0}}' mancube/covenant:0.2.0
# mancube/covenant@sha256:…  — use that in CI
```

A tag can be repointed; a digest cannot.

Releases after `0.1.0` are signed (keyless, Sigstore) and carry build provenance. The two
commands that check an image before you run it are in
[SECURITY.md](https://github.com/lucheeseng827/covenant/blob/main/SECURITY.md#releases).

## Quick start

The binary is the entrypoint, so the `docker` args are just `covenant` subcommands. Everything is local file I/O, so mount the directory holding your contract and data:

```bash
docker run --rm mancube/covenant:0.2.0 --version
```

**1. Draft a contract from data you already have.** Nobody writes their first contract from a blank page:

```bash
docker run --rm -v "$PWD:/w" mancube/covenant:0.2.0 \
  infer /w/exports/orders.parquet --out /w/orders.yaml
```

The draft is honest about its evidence: types, presence and nullability become rules, while every guess from the sample carries a `# confirm:` note naming what it was inferred from. `unique` is never drafted — a sample cannot prove it.

**2. Lint the contract.** `validate` refuses a contract it could not enforce, so a bad contract fails here rather than half-enforcing later:

```bash
docker run --rm -v "$PWD:/w" mancube/covenant:0.2.0 validate /w/orders.yaml
```

**3. Gate a file in CI.** This is the whole integration — exit 1 fails the job:

```bash
docker run --rm -v "$PWD:/w" mancube/covenant:0.2.0 \
  check /w/exports/orders.parquet -c /w/orders.yaml
```

A failure names the rule, the row and the value, and the team that owns the contract:

```text
FAIL  /w/exports/orders.ndjson  [orders v1.2.0, model orders]
  rows checked: 6   violations: 9
  contract owner: data-platform@acme.io
  by rule:
    order_id                 pattern                × 1
    amount_cents             min                    × 1
    currency                 allowed                × 1
  samples:
    [row 1] row 1: field "amount_cents" value -50 is below min 0
    [row 1] row 1: field "currency" value "BTC" not in allowed set [EUR, GBP, USD]
```

**4. Block the pull request that breaks the contract.** `diff` runs on two YAML files — no data path, no credentials — which makes it the cheapest thing to adopt first:

```bash
docker run --rm -v "$PWD:/w" mancube/covenant:0.2.0 \
  diff /w/main/orders.yaml /w/pr/orders.yaml --fail-on breaking
```

It classifies every change as **breaking**, **risky** or **info**, says whether producers or consumers feel it, and enforces the matching semver bump.

**5. Put it in the data path.** The stream gate is a plain pipe process: records in on stdin, clean records out on stdout, violations diverted to a dead-letter file with the rule that caught them. Transport-agnostic — anything that can pipe can gate:

```bash
kcat -C -t orders_raw -e \
  | docker run -i --rm -v "$PWD:/w" mancube/covenant:0.2.0 \
      gate -c /w/orders.yaml --dlq /w/orders.dlq.ndjson \
  | kcat -P -t orders_validated
```

Use `-i` (not `-t`) so stdin stays a pipe.

## Exit codes — the CI contract

| Code | Meaning |
|------|---------|
| `0` | clean — the data conforms, or the diff is acceptable |
| `1` | the **subject** violates: data breaks the contract, the diff is breaking, or the contract has error-level lint findings |
| `2` | the **run** failed: bad flags, an unreadable file, or a contract too broken to enforce |

Never conflate 1 and 2. A pipeline that treats "the run crashed" as "the data was fine" is the failure this tool exists to prevent.

## Running as nonroot

The image runs as uid `65532` with no shell. Mounted paths must be readable by that uid, and any directory it writes to (a `--dlq` target, an `--out` path) must be writable by it:

```bash
docker run --rm -u "$(id -u):$(id -g)" -v "$PWD:/w" mancube/covenant:0.2.0 \
  check /w/orders.ndjson -c /w/orders.yaml
```

There is no shell in the image, so `docker exec … sh` will not work — that is deliberate.

## What it does not do

Covenant enforces contracts at a boundary. It is not a catalog (no lineage, no discovery UI), not a registry, and not a freshness or SLA monitor. Nested field paths, a `decimal` type, dictionary-encoded columns and windowed uniqueness for unbounded streams are not implemented yet. The repository's README lists the gaps in full.

## License

Apache-2.0. The image ships `LICENSE` and `NOTICE` at `/usr/share/doc/covenant/`.
