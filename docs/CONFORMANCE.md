# ODCS conformance vectors

`conformance/odcs/` holds test vectors for the Open Data Contract Standard: small cases that pin
what the standard's rules mean, written so that **any** engine can run them — not only this
one. Each vector is one contract, a handful of records, and the verdict the standard implies:

```yaml
vector: 1                                  # format revision
id: options/integer-exclusive-maximum      # its path in the suite
standard: ODCS v3.1.0                      # the revision whose text it pins
cites: "schema.md: logicalTypeOptions.exclusiveMaximum"
description: "exclusiveMaximum excludes the bound: values < exclusiveMaximum."
contract: |                                # a complete ODCS document, one schema object
  apiVersion: v3.1.0
  kind: DataContract
  id: vector
  version: 1.0.0
  schema:
    - name: t
      properties:
        - name: v
          logicalType: integer
          logicalTypeOptions:
            exclusiveMaximum: 10
records:                                   # JSON objects; a missing key is an absent value
  - { v: 9 }
  - { v: 10 }
  - { v: 11 }
expect:
  verdict: fail                            # does the dataset satisfy the contract?
  failing: [1, 2]                          # the records that break it, when that is not
                                           # the engine's choice
```

`failing` is left out where engines legitimately name different records — for a duplicate,
some report the second occurrence and some report both — and only the verdict is pinned.

## Running them

```bash
covenant conformance conformance/odcs            # exit 1 if a supported vector disagrees
covenant conformance conformance/odcs --format json
```

An engine that cannot express a vector's rules reports it **unsupported**, which is not a
failure. Covenant does so exactly when its ODCS reader refuses the contract (see `ODCS.md`).
Today: 36 vectors, 30 pass, 6 unsupported (multipleOf, exclusive bounds on numbers,
thresholds in rows and in percent, multi-column duplicates, rowCount), 0 fail.

To run the vectors through another engine, read each file, load `contract`, check `records`,
and compare the verdict and, where given, the failing records.

## What a vector may pin

Only what the standard's text settles. Where the text supports two readings, the case is left
out of the suite and listed below, so that it is decided by the standard rather than by
whichever engine shipped first. Each vector carries `cites` for where its rule is defined.

## Questions for the standard

These came up while writing the suite. Each says what Covenant does meanwhile.

1. **`mustBeBetween` bounds.** The operator table gives `∈` (bounds included); the prose says
   it is equivalent to `mustBeGreaterThan` plus `mustBeLessThan` (bounds excluded). Under the
   first, `[0, 0]` means "none"; under the second, it matches nothing.
   *Covenant refuses a `mustBeBetween` whose meaning differs between the two readings.*
2. **The default date and time format.** The standard gives one default for `date`,
   `timestamp` and `time` alike: ISO 8601 `YYYY-MM-DDTHH:mm:ss.SSSZ`. Read literally, a plain
   date such as `2024-07-10` does not match it. In the JDK formatter the standard cites, `YYYY`
   and `DD` are also the week-based year and the day of the year.
   *Covenant reads an unformatted `date` as `YYYY-MM-DD` and an unformatted `timestamp` as RFC
   3339.*
3. **Resolver style.** The JDK formatter's default resolver (`SMART`) turns `2026-02-30`
   into `2026-02-28`; `STRICT` rejects it. Which does a valid date mean?
   *Covenant rejects dates that do not exist.*
4. **Timestamps without a zone.** `timezone` and `defaultTimezone` suggest a zone-less value
   such as `2026-09-24T10:00:00` is valid and read in the default zone.
   *Covenant requires an offset (RFC 3339) and rejects it.*
5. **Is `pattern` anchored?** The standard fixes the syntax (ECMA-262) but not whether the
   pattern must match the whole value or only a part of it (JSON Schema's convention).
   *Covenant searches, unanchored: `^…$` anchors.*
6. **Nulls and `unique`.** Are nulls exempt, as in SQL?
   *Covenant exempts nulls and absent values.*
7. **Is a JSON `5.0` an integer?**
   *Covenant rejects it: an integer property accepts only values written as integers.*
8. **Nulls under `invalidValues` with `validValues`.** Is a null invalid when `validValues`
   does not list it?
   *Covenant leaves nulls to `required` and `nullValues`.*
9. **What `duplicateValues` counts.** Extra occurrences, or every row involved? For
   `mustBe: 0` it makes no difference; for a threshold it does.
   *Covenant enforces only zero tolerance, where both readings agree.*
10. **String length.** Are `minLength` and `maxLength` counted in characters (code points)
    or bytes?
    *Covenant counts Unicode scalar values.*
11. **Writing a literal `${NAME}`.** v3.2 lets any string hold variable references,
    `${NAME}` or `${NAME:-default}`, but defines no escape — so a pattern cannot say "the text
    `${NAME}`".
    *Covenant counts every `${NAME}` as a reference, after a backslash too, and refuses the
    rule that holds it: it does not resolve variables yet.*

## Refreshing

The vectors cite ODCS v3.1.0, whose text they were written against. When a new revision
changes a rule's wording, add a vector for the new reading under that revision rather than
editing the old one; a vector is a record of what a text said.
