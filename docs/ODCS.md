# ODCS contracts

Covenant reads contracts written in the **Open Data Contract Standard (ODCS) v3** — v3.0.0
through v3.2.0, each reviewed as described under [Revisions](#revisions) — as well as its own
`covenant: 1` format. An ODCS file is recognised by its
`apiVersion` and `kind: DataContract`; there is nothing to configure:

```bash
covenant validate orders.odcs.yaml
covenant check exports/orders.parquet -c orders.odcs.yaml
kcat -C -t orders_raw -e | covenant gate -c orders.odcs.yaml --dlq orders.dlq.ndjson
covenant diff main/orders.odcs.yaml pr/orders.odcs.yaml --fail-on breaking
```

## Exact or refused

Every ODCS rule either becomes a rule Covenant enforces with the same meaning, or Covenant
refuses the contract and names the rule. It never skips a rule quietly: a PASS against a
contract whose rules were silently dropped would certify data nobody checked.

```text
$ covenant check orders.ndjson -c orders.odcs.yaml
covenant: error: orders.odcs.yaml: 1 contract rule(s) cannot be enforced yet:
  - schema.orders.quality[0] (library rowCount): rowCount is a dataset-level rule: not enforced yet
  Rerun with --allow-unenforced to check everything else; the report is then marked partial.
```

That run exits 2: the contract could not be enforced, which is not the same as the data
passing or failing. With `--allow-unenforced`, Covenant checks everything else and says so
everywhere a verdict appears:

- `check` prints `PASS (partial)` or `FAIL (partial)`, followed by the skipped rules;
- `diff` prints `(partial)` after its summary and lists the rules it did not compare, because
  a change inside one of them is invisible to it;
- JSON reports from `check` and `diff`, and every `POST /v1/check` report from
  `covenant serve`, carry an `unenforced` list (`path`, `rule`, `reason`). A report without
  one covers the whole contract;
- `covenant validate` reports those rules as warnings instead of errors (without the flag, and
  in `POST /v1/validate`, they are errors);
- every run that enforces the contract — `check`, `gate`, `diff`, `consumer-check`, `serve` —
  prints a warning on stderr naming how many rules were skipped.

Descriptive and operational metadata — descriptions, tags, physical types, servers, team
members, SLAs, roles, support channels, custom properties — promises nothing about values, so
it is not read and never causes a refusal.

## The mapping

### The contract

| ODCS | Covenant |
|---|---|
| `id`, `version` | The contract id and version. `version` must be semver: `covenant diff` enforces the bump |
| `name` | The contract name |
| `team.name` | The owner, named in failing reports |
| `description.purpose` | The description |
| `schema[]` | One model per schema object, keyed by its `name`. ODCS schemas are open: columns the contract doesn't declare are not violations |

### Properties

| ODCS | Enforced as |
|---|---|
| `physicalName`, else `name` | The column or key Covenant looks for in the data |
| `logicalType: string` · `integer` · `number` · `boolean` | `string` · `integer` · `float` · `boolean` |
| `logicalType: date` | `date` — `YYYY-MM-DD` strings, or native date columns |
| `logicalType: timestamp` | `timestamp` — RFC 3339 strings, or native timestamp columns |
| `logicalType: time` · `object` · `array` · `map` · `vector` | Not enforced yet |
| No `logicalType` | Not enforced, if the property promises anything |
| `required: true` | Present and not null |
| `required: false` (the default) | May be null or absent |
| `unique: true` | Unique |
| `primaryKey: true` on one property | Unique and not null. `validate` notes it when the property also says `required: false` |
| `primaryKey: true` on several properties | Each part not null; uniqueness of the combination is not enforced yet |
| `enum` (v3.2 objects, or plain values) | The allowed set. A `null` entry changes nothing: whether a value may be null is `required`'s (or a primary key's) business |
| `deprecated: true` | The advisory marker: reported by `diff` and `consumer-check`, no effect on checks |
| `semanticType: dimension` | Checked like any column |
| `semanticType: measure` | Not a column: an aggregate its `transformLogic` computes over the records. Not checked (`validate` notes it), and a rule on it is refused |
| `relationships` | Not enforced: a reference to another dataset can't be verified at one boundary |

### `logicalTypeOptions`

| Type | Option | Enforced as |
|---|---|---|
| string | `minLength`, `maxLength`, `pattern` | Length bounds; the regex, unanchored as in ODCS |
| string | `format: email`, `format: uri` | The format check |
| string | `format: uuid` | The `uuid` type (not yet together with `pattern` or length options) |
| string | Any other `format` | Not enforced |
| integer | `minimum`, `maximum` | Inclusive bounds |
| integer | `exclusiveMinimum`, `exclusiveMaximum` | Exact inclusive bounds: `> 5` is `>= 6`. v3.0's form, `true` on a `minimum` or `maximum`, maps the same way |
| number | `minimum`, `maximum` | Inclusive bounds |
| number | `exclusiveMinimum`, `exclusiveMaximum` | Not enforced yet |
| integer, number | `multipleOf`, `format` | Not enforced |
| date | `format: yyyy-MM-dd` | The `date` type itself |
| date, timestamp | Any other `format` | The property is not checked: values in that format would fail the date or RFC 3339 check |
| date, timestamp | `minimum`, `maximum`, exclusive bounds, `timezone`, `defaultTimezone` | Not enforced yet |

### Quality rules

A `library` rule maps onto a per-value rule only when it tolerates no offending rows:
`mustBe: 0`, `mustBeLessOrEqualTo: 0`, or — since a count of rows is
a whole number — `mustBeLessThan: 1` in rows.

`mustBeBetween` is enforced only where the standard's two definitions of it agree: the
operator table gives it the symbol `∈` (bounds included), while the prose equates it with
`mustBeGreaterThan` plus `mustBeLessThan` (bounds excluded). `[0, 0]` means "no offending
rows" under the first and matches nothing under the second, so Covenant refuses it and
suggests `mustBe: 0`.

| Rule | Enforced as |
|---|---|
| `nullValues` | Present and not null |
| `duplicateValues` on a property, or at schema level over one property | Unique |
| `invalidValues` with `validValues` | The allowed set (intersected with `enum`, if both are given) |
| `invalidValues` with `pattern` | The pattern |
| `missingValues` listing `null` and/or `""` | Not null / at least one character |
| Any threshold, e.g. `mustBeLessThan: 5` with `unit: percent` | Not enforced yet |
| `rowCount`; `duplicateValues` across several properties | Not enforced yet |
| `sql`, `custom`, `text` | Not enforced: SQL needs a SQL engine, a custom rule belongs to the engine it names, and text is prose |

## Revisions

Covenant reads the ODCS revisions it has been reviewed against, listed below. A review goes
through everything a revision changed and settles, for each change that promises something
about values, how it maps: exactly, or refused. A contract of a v3 revision not listed here is
refused, naming its `apiVersion`, because a rule that revision introduced would otherwise go
unchecked; with `--allow-unenforced` it is read as the newest listed revision and the result is
marked partial. v2 contracts are not read.

| Revision | What it changed that promises something about values |
|---|---|
| v3.0.0 | The v3 structure — `schema[].properties` with logical types, `logicalTypeOptions` and `quality` — mapped as above |
| v3.0.1 | Nothing: descriptive fields only |
| v3.0.2 | `physicalName` on properties; a default date format (question 2 in `CONFORMANCE.md`) |
| v3.1.0 | Exclusive bounds as values; `timestamp` and `time`, with `timezone` and `defaultTimezone`; `relationships`; the `library` quality metrics |
| v3.2.0 | `enum`, `semanticType`, variables, and the `map` and `vector` types — below |

### v3.2.0

| Change | Covenant |
|---|---|
| `enum` | The allowed set |
| `semanticType` | `column` and `dimension` are checked as columns. A `measure` is computed over the records, not carried by them: it is not checked as a column, and a rule on one is refused |
| Variables: `${NAME}`, `${NAME:-default}` | Not resolved yet. A column name, pattern or allowed value holding one is refused, never matched as literal text; the standard defines no escape, so `\${NAME}` counts too. A `version` holding one is not semver, so the contract is refused as invalid. Names — `id`, `name`, `team.name`, a schema object's `name` — are reported as written. Anywhere else, a variable sits in metadata Covenant does not read |
| `map` and `vector` logical types | Not enforced yet |
| `deprecated` | The advisory marker |
| `servers[].encoding` | Servers are not read: Covenant reads text as UTF-8, whatever a server declares |
| `context`, `synonyms`, relationship `id`, `vendor` on custom properties, custom properties on SLAs, new server types | Promise nothing about values: not read |

When a new revision is published, the `ODCS watch` workflow opens an issue for its review, and
fails once the revision has gone unsupported for 60 days.

## Coverage

The tests load 40 of the ODCS project's published example contracts
(`tests/fixtures/odcs/bitol/`, whose `SOURCE.md` lists what was left out or changed). Six of
them define no schema (they illustrate servers, SLAs, roles and similar sections), so there is
nothing to enforce. Of the 34 that do, **15 are enforced in full** and 16 partially. In the
other 3 there is no property Covenant can check yet: two list no properties, and one types its
columns only physically.
The most common gaps, in order: nested and untyped properties, composite primary keys, and
relationships.
