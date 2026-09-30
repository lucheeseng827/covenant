# Enforcing a contract in Postgres

`covenant check` gates data on its way somewhere. When that somewhere is a Postgres table, the
table can enforce the contract itself: `covenant export postgres` compiles a contract model
into a `CREATE TABLE` whose column types, `NOT NULL`, `CHECK` and `UNIQUE` constraints reject
the rows `covenant check` reports.

```bash
covenant export postgres -c orders.yaml                       # SQL on stdout
covenant export postgres -c orders.yaml --table sales.orders -o orders.sql
```

```sql
CREATE TABLE "orders" (
  "order_id" text NOT NULL,
  "amount_cents" bigint NOT NULL,
  "currency" text NOT NULL,
  ...
  CONSTRAINT "order_id.pattern" CHECK (("order_id" COLLATE "C") ~ '^ord_[a-z0-9]{12}$'),
  CONSTRAINT "amount_cents.min" CHECK ("amount_cents" >= 0),
  CONSTRAINT "currency.allowed" CHECK ("currency" IN ('USD', 'EUR', 'GBP')),
  CONSTRAINT "orders.order_id.unique" UNIQUE ("order_id")
);
```

Each constraint is one contract rule, named `<column>.<rule>`, so a rejected insert says which
rule it broke: `new row for relation "orders" violates check constraint "amount_cents.min"`.
The script starts with a guard that stops it on a database that is not UTF8 (lengths and
patterns count characters) or that reads backslashes in strings as escapes.

## Exact or refused

As with ODCS, a rule without an exact Postgres equivalent is not approximated: the export
refuses (exit 2) and names it. `--allow-unenforced` exports everything else and lists what the
table leaves out in a comment at the top of the script.

| Contract | Postgres |
|---|---|
| `string` · `integer` · `float` · `boolean` | `text` · `bigint` · `double precision` · `boolean` |
| `date` · `timestamp` | `date` · `timestamptz` |
| `uuid` | `text` with a check for the 8-4-4-4-12 shape: Covenant's uuid is text, compared as text, and Postgres's `uuid` type accepts other shapes and ignores case |
| `required: true` and not `nullable` | `NOT NULL` |
| `required` alone | Nothing: every row has every column |
| `min`, `max` on integers | `>=`, `<=` against the whole-number bound, as the engines compare |
| `min`, `max` on floats | `>=`, `<=`, letting NaN through as the engines do |
| `min_length`, `max_length` | `char_length`, which counts what Covenant counts |
| `pattern` | `~`, translated (below), or refused |
| `format: email`, `format: uri` | The same checks as regular expressions, and no whitespace (Unicode's) |
| `allowed` | `IN (…)` — refused on timestamps |
| `unique` | `UNIQUE` — refused on timestamps |

Timestamps are the refusals: Covenant compares a timestamp's text, and Postgres compares the
instant it names, so `2026-01-01T00:00:00Z` and `2026-01-01T00:00:00+00:00` are two values to
one and one to the other.

**Patterns** are translated only from the part of the syntax that means the same in Covenant's
regex engine and in Postgres's: literals, escaped punctuation, `\n` `\t` `\r`, `.`, anchors,
classes of literals and ranges, groups, alternation and repetition (up to 255, Postgres's
limit). `.` becomes `[^\n]`, because Postgres's `.` also matches a newline. `\d`, `\w`, `\s` and
`\b` are refused: they are Unicode-wide in Covenant and depend on the locale in Postgres, so
spell the characters out — `[0-9]` for a digit.

## What "exact" means here

Insert rows one at a time, with values of the column types: a row is rejected exactly when the
engines report it, given the rows the table already holds. Two things follow from that
sentence.

- **Values of the column types.** How the text `5` becomes a `bigint`, or `2026-09-24` a `date`,
  is up to whatever loads the table: a CSV `COPY` parses text that Covenant's JSON check would
  reject as a string. The constraints start where the Arrow engine does, from typed values.
- **The rows the table holds.** A rejected row is not in the table, so it cannot make a later
  row a duplicate. `covenant check` reads a dataset as it is, so it reports that later row
  too.

A table also has no absent values, so it reads nulls as the Arrow engine does
(`ARCHITECTURE.md`): a required, nullable field's missing key is a null it allows, and a null
in an optional field that forbids nulls reads as absent.

A contract's violation budget (`policy.max_violations`) has no equivalent: the table rejects
every violating row, and the script says so.

## How it is verified

`tests/postgres_test.rs` creates the exported tables in a real Postgres and inserts, row by
row:

- every record of the [ODCS conformance vectors](CONFORMANCE.md) Covenant supports;
- edge values: integers at the edge of float precision, `-0.0`, `NaN` and the infinities,
  combining characters and emoji against lengths, newlines against `.`, Unicode whitespace in
  emails and URIs, uuid shapes and case, and duplicates of rejected rows;
- every pattern of a table of translations, against inputs that probe newlines, ranges and
  Unicode;

and compares each verdict with the engines'. The CI job `postgres` runs it on every change.
