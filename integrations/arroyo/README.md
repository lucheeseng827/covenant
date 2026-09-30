# Covenant in Arroyo SQL

Two Rust UDFs for [Arroyo](https://github.com/ArroyoSystems/arroyo), each linking the engine
itself, so a record in a streaming SQL pipeline is judged exactly as `covenant gate` judges
it:

| UDF | Returns |
|---|---|
| `covenant_verdict(record TEXT) -> TEXT` | `'pass'`, `'block'` or `'warn'` |
| `covenant_dead_letter(record TEXT) -> TEXT` | the dead letter `covenant gate` writes, as JSON, or `NULL` when the record keeps the contract |

```sql
INSERT INTO orders_validated
SELECT value FROM orders_raw WHERE covenant_verdict(value) <> 'block';

INSERT INTO orders_dlq
SELECT covenant_dead_letter(value) AS value FROM orders_raw
WHERE covenant_dead_letter(value) IS NOT NULL;
```

A `NULL` record gives `NULL`. Each call judges one record on its own, so a dead letter's `row`
is 0, and `unique` cannot be held. A model that declares `unique` fields makes the UDF fail on
its first call unless it was rendered with `--no-unique`, so the fields are never skipped
without anyone knowing.

## Render, then register

Arroyo builds a Rust UDF from a single source file, so the contract is compiled into the source.
`render.py` first checks the contract with the engine: `covenant validate` for the contract and
`covenant gate` for the model. It then writes the two files:

```bash
python3 render.py orders.yaml --covenant $(which covenant) --no-unique --out udfs/
```

Register each file in Arroyo, either in the console's UDF editor or through the API
(`POST /api/v1/udfs`). Arroyo compiles it with its own compiler service against the published
`arroyo-udf-plugin`. The files depend on the engine as `covenant-data` from its repository,
without its `arrow` feature. That feature's Arrow line would clash with the older Arrow the UDF
plugin is built on, and the row engine is all a UDF needs. `--engine <path>` builds against a
local checkout of the engine instead.

## Tested

`e2e_arroyo.py` runs the whole path against a real Arroyo, the release binary, with nothing
mocked:

1. Render the UDFs with the demo contract.
2. Start `arroyo cluster` and register both UDFs through the API, so that Arroyo's compiler
   builds them.
3. Run the pipeline above over the demo orders, with file sources and sinks.

The records `validated` receives must be exactly the ones `covenant gate --no-unique` passes.
The dead letters must be the command's own, apart from timestamps and row numbers.

```bash
python3 e2e_arroyo.py <covenant binary> <arroyo binary>
```

CI runs it against Arroyo 0.15.0.
