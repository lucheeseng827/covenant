# Contributing to Covenant

Thanks for looking. Covenant is a small, deliberately narrow tool, and the
narrowness is the feature — so the most useful contributions are usually
sharper enforcement of what already exists rather than new surface.

## The one rule that shapes everything

**Everything in the contract spec must be something the runtime actually
enforces at a boundary.** If a proposed field cannot be checked on a row or a
batch, it does not belong in the schema. `covenant validate` refuses contracts
it cannot enforce, and that principle applies to the spec itself.

The corollary: a checker must be certain before it fails a record. Being wrong
in the strict direction fails clean data in production, which is worse than
missing a violation.

## Getting set up

```bash
cargo build
cargo test
cargo build --features serve   # optional localhost console
```

Tests are the specification. `cargo test` runs the full suite, including a
doc-test on the embedding example in `src/lib.rs`.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

CI runs exactly these.

## What makes a good change

- **A failing test first.** Engine changes are easiest to review as a test that
  encodes the record and the verdict, then the fix.
- **Both engines, or say why not.** There are two: the row engine
  (`src/engine/row.rs`, NDJSON and the stream gate) and the columnar engine
  (`src/engine/arrow.rs`, CSV/Parquet/Arrow). A constraint that behaves
  differently between them needs either parity or a documented reason —
  `ARCHITECTURE.md` records the one existing divergence, on null semantics.
- **Messages that name the rule, the row, and the value.** Every violation
  message follows that shape. It is the product.
- **Exit-code discipline.** 0 clean, 1 the subject violates, 2 the run failed.
  Never conflate the last two.

## What is likely to be declined

- New contract fields that cannot be enforced at a boundary (see the rule above).
- Catalog, lineage, discovery, storage, or freshness/SLA features. Covenant
  enforces contracts at a boundary; it does not describe or watch an estate.
- Anything that makes the binary phone home. There is no telemetry and there
  will not be.
- Drafting `unique` from a sample in `covenant infer`. A sample cannot prove
  uniqueness, and a wrong `unique: true` fails clean data in production. It is
  suggested as a note, never emitted as a rule.

## Reporting bugs

A contract file and the smallest record that reproduces the behaviour are worth
more than a description. Include `covenant --version` and the exact command.

## Licence

By contributing you agree that your contributions are licensed under the
Apache License 2.0, the same as the project.
