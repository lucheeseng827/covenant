# Profiles and drift

`covenant check` answers "does this data keep its promises?". A profile, written by the same
check, is a record of what the data looked like: presence, nulls, distinct values, ranges and
distributions, per contract field. `covenant drift` compares two profiles and answers "is this
still the data we used to get?" — the changes a contract does not forbid, and a consumer still
notices.

```bash
# The check you already run, plus a profile of what it read. Same read: no second pass.
covenant check exports/orders.parquet -c orders.yaml --profile today.json

# Fold recent runs, partitions or days into a baseline.
covenant profile merge history/*.json -o baseline.json

# Compare. Exit 0 = stable, 1 = drifted, 2 = the run failed.
covenant drift baseline.json today.json

# Read one.
covenant profile show today.json
```

```text
DRIFT orders/orders — baseline v1.2.0, 3 runs, 12,000 rows · current v1.2.0, 1 run, 1,500 rows
  (model)         rows per run fell from 4,000 to 1,500 (−62.5%; threshold ±50%)
  amount_cents    values shifted: median 4,016 → 9,024, p95 5,984 → 11,072 (PSI 7.94 over the baseline's deciles; threshold 0.25)
  currency        "EUR" rose from 30% to 67.1% of values (Jensen–Shannon distance 0.32; threshold 0.1)
  customer_email  null rate rose from 1.9% to 21% (threshold ±5 points)
  4 findings over 6 fields
```

Every row of that data passed `check`: the amounts stayed within bounds, the currencies stayed
in the allowed set, and the email field may be null.

## What a profile keeps

For every field of the checked model:

| | |
|---|---|
| All fields | rows seen, rows that carried the field, nulls, values not of the contract type |
| `string` | length (min, max, mean), empty strings, distinct values — **never a value** |
| `integer`, `float` | min, max, mean, quantiles, distinct values; NaN and infinities counted apart |
| `timestamp`, `date` | the earliest and latest value, distinct values |
| `uuid` | distinct values |
| `boolean`, and `allowed:` fields | an exact count per value, and a count of values outside the set |

Profiles are JSON, about 30 KB for a six-field model, and written atomically.

**Same data, same profile.** Values are sketched in their contract type's domain, not their
serialization, so NDJSON, CSV and Parquet renderings of the same records give the same
sketches, bit for bit. A Parquet `FLOAT` 0.1 profiles as 0.1, not 0.100000001. The one
difference is inherent: a columnar file cannot say a value is absent rather than null, so there
every record carries the column and absent values count as nulls.

**Merging is exact.** Counts add; extremes combine; distinct counts are HyperLogLog registers,
merged by maximum; quantiles are a histogram, merged by adding. A merged profile is exactly
the profile of all the data, whatever order it arrived in — the only thing merging changes is
float rounding in the running sum behind a mean. Profiles merge only within one contract model;
a newer contract version wins, and a field whose type changed refuses to merge.

## Accuracy

- **Distinct values**: HyperLogLog with 4,096 registers and Ertl's improved estimator, about
  1.6 % standard error, with no bias at small counts. Hashes are fixed forever, so profiles
  from different machines and releases merge.
- **Quantiles**: a histogram with 64 logarithmic buckets per power of two, in the manner of
  DDSketch: every quantile is within 1/128 (0.78 %) of the true value. Past 4,096 buckets per
  sign (about 19 decades of range), the smallest magnitudes fold together.

## What drift compares

| Metric | Compared | Default threshold |
|---|---|---|
| Volume | rows per run (so a baseline of 7 merged days compares with one day) | ±50 % |
| Null, missing and invalid rates | the change, in points | ±5 points |
| Numeric distribution | population stability index over the baseline's deciles | 0.25 |
| Categorical distribution (`boolean`, `allowed:`) | Jensen–Shannon distance between value shares, naming the value that moved most | 0.1 |
| Distinct values | a falling share for fields that were (nearly) unique — values started repeating; the count for low-cardinality fields | ±10 % |

Each threshold is a flag: `--rate`, `--volume`, `--psi`, `--js`, `--distinct`. Nothing is
learned or trained; a finding is always one number against one threshold.

A distribution or distinct comparison needs 100 values on each side; a field below that is
listed as not compared, with the reason, rather than judged on noise. Fields that exist on one
side only, or changed type, are listed the same way. A low-cardinality distinct count also
needs ten values per distinct value on each side, since a smaller sample simply shows fewer
values; below that, and for fields that are neither (nearly) unique nor low-cardinality, the
distinct count is not compared and no finding is made.

## Cost

Profiling is off unless asked for. On a 1M-row, seven-field benchmark (a unique string id with a
pattern, a bounded integer, a float, a boolean, an `allowed:` string, a free string and a
timestamp), it adds, counted in CPU instructions against the check alone: 8.6 % for NDJSON,
16.5 % for CSV and 17.6 % for Parquet — 60 to 90 instructions per value, depending on the
type. For NDJSON the row engine hands each value it looks up to the profiler, so profiling
costs no second lookup.

## For programs

`covenant profile show --format json` gives rates, estimates and quantiles per field;
`covenant drift --format json` gives every finding with its metric, both sides' values, the
score and the threshold. The library exposes the same pieces: `profile::Profiler` fed from
`sources::check_path_profiled` (or record by record, or batch by batch), `Profile::merge`, and
`drift::drift`.
