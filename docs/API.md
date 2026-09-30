# The engine API and its change policy

Covenant is also a library, for embedding contract enforcement inside another tool: a catalog
that stores contracts but does not run them, a stream gateway, a lakehouse writer. This page
says what an embedder can build on, and what "stable" promises.

```toml
[dependencies]
covenant-data = "0.2"     # the package; the library is imported as `covenant`
```

```rust
use covenant::prelude::*;
```

## The stable surface

Everything `covenant::prelude` exports, and nothing else:

| Area | Items |
|---|---|
| Contracts | `Contract::{load, load_path, parse, from_path, lint}`, `LoadedContract` (`contract`, `format`, `unenforced`, `notes`, `into_enforceable`, `lint`), `SourceFormat`, `UnenforcedRule`, `LintFinding`, `LintLevel` |
| Compiling | `CompiledContract::{compile, resolve_model}`, `CompiledModel` |
| Checking | `validate_record`, `validate_batch`, `validate_source_batch`, `SchemaFindings` (`new`), `UniqueTracker::new`, `Collector::{new, into_report}`, `check_path`, `check_path_profiled`, `DataFormat` (`infer`), `CheckReport` (`passed`, `render_human`), `Violation`, `Rule`, `RuleCount`, `ReportHeader` |
| Streams | `gate`, `GateOutcome` (`stats`, `failed`), `GateStats`, `RecordGate::{new, judge, violations, dead_letter, stats, failed, into_stats}`, `Verdict`, `DlqEnvelope` |
| Contract changes | `diff`, `DiffReport` (`max_severity`, `render_human`), `Change`, `Severity`, `Impact` |
| Profiles and drift | `Profiler::{new, observe_record, observe_batch, finish}`, `Profile::{from_path, parse, merge, write, to_json, summary}`, `ProfileSummary`, `drift`, `DriftThresholds`, `DriftReport` (`drifted`, `render_human`), `Finding`, `Metric`, `NotCompared` |
| Errors | `CovenantError`, `Result` |

`validate_batch`, `validate_source_batch`, `SchemaFindings` and `Profiler::observe_batch` belong
to the `arrow` feature, which is on by default; a build with `default-features = false` has the
rest.

`tests/api_surface.rs` names every one of these with its full signature, and the fields
embedders read, so a change that would break an embedder fails to compile. It can still be
made, but only on purpose: by editing that file in the same change, with the version bump
below.

Internal, whatever its visibility: the `cli`, `serve`, `mcp` and `kafka` modules, the ODCS
reader's and the sketches' internals, the `postgres` dialect until it settles, and any item or
field not listed above. Internal items can change in any release.

## The policy

**Versions** follow semver, on the `covenant-data` crate.

**Before 1.0 (now)**, a minor release may break the stable surface. Each break is listed under
**Breaking** in `CHANGELOG.md`, with the migration.

**From 1.0**, the stable surface only grows within a major version:

- new functions, methods and types;
- new fields on report types — they are built by the library and read by you, not built by
  you;
- new variants on the enums marked `#[non_exhaustive]` (`Rule`, `Metric`, `SourceFormat`,
  `CovenantError`), so match them with a wildcard arm. `Severity`, `Impact`, `LintLevel`
  and `Verdict` are closed scales and do not grow.

A removal or signature change needs a major release. It is deprecated in a minor release
at least 90 days before.

**Formats** have their own revision keys and are versioned apart from the code: contracts
(`covenant: 1`), consumer manifests (`consumer: 1`), profiles (`profile: 1`), conformance
vectors (`vector: 1`), run reports (`report: 1`, [`REPORT.md`](REPORT.md)). A release keeps reading every revision earlier releases wrote; a change
to a format is a new revision, never a silent change to an old one.

**The command line** is held to the same standard: its exit codes (0 clean, 1 violated, 2 the
run failed) and the fields of its JSON output are added to, never renamed or removed, within
a major version.

**The minimum supported Rust** is `rust-version` in `Cargo.toml`, built by CI with exactly that
toolchain. It rises only in a minor release, and never to a Rust release less than six months
old.

**The Python face**, `covenant_data` (built from `python/`), wraps this surface: `Contract`,
`Report`, `check`, `CovenantError` and `testing.assert_conforms`. It carries the same version
numbers and is held to the same policy; its tests compare its reports with the command's.
**The WASM face** is the command itself, built for `wasm32-wasip1`: the same arguments,
output and exit codes, held byte for byte to the native build in CI. `js/covenant.mjs` runs it
from JavaScript over an in-memory filesystem (`load`, `run`, `check`); that module is new and
not yet held to this policy.
