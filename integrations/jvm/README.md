# Covenant on the JVM

The gate as a Java library, and as a Kafka Connect transformation built on it. Neither ships
native code. The engine runs as its own WebAssembly build, `embed/`, a Rust reactor module that
exports the gate through a small C ABI. [Chicory](https://github.com/dylibso/chicory), a
WebAssembly runtime written in Java, compiles that module to JVM bytecode when the first gate
opens. A record is therefore judged by the same code as `covenant gate`, and gets the same
dead letter.

| Module | |
|---|---|
| `embed/` | The gate as a WebAssembly reactor module (`covenant_embed.wasm`). One instance holds one gate; see `embed/src/lib.rs` for the ABI. |
| `covenant-jvm/` | `Gate`, `Verdict`, `UniqueKeys`, `Stats`: the gate for any JVM program. Java 17+. |
| `covenant-connect/` | `CovenantGate`, a Kafka Connect single message transformation, shaded into one jar. |
| `covenant-kroxylicious/` | `CovenantGateFilterFactory`, a Kroxylicious filter: the gate in a Kafka proxy, refusing records at produce time. |

## Build

```bash
rustup target add wasm32-wasip1
mvn verify            # builds embed/ with cargo, bundles the module, runs the tests
```

## The library

```java
try (Gate gate = Gate.open(Path.of("orders.yaml"), null, UniqueKeys.SKIP)) {
  Verdict verdict = gate.judge(recordJson);          // PASS, BLOCK or WARN
  if (verdict != Verdict.PASS) {
    deadLetters.send(gate.deadLetter());            // the envelope `covenant gate` writes
  }
}
```

`Gate.open` loads, lints and compiles the contract (`covenant: 1` or ODCS v3) exactly as the
command does, and throws `CovenantException` for a contract it could not enforce exactly. A
gate keeps what spans a stream: row numbers, counts (`stats()`), the violation budget
(`failed()`) and, with `UniqueKeys.TRACK`, every `unique` key it has judged. `REFUSE`, the
cautious choice, refuses a model that declares `unique` fields; `SKIP` leaves them unenforced.
A gate is not thread-safe, so give each stream its own.

On a 4-core cloud VM, a warmed-up gate judges about 55,000 records a second (18 µs each, with
one in ten dead-lettered). The first gate in a JVM takes about 2 s to compile the module; later
ones open in about 20 ms.

## Kafka Connect

Put `covenant-connect/target/covenant-connect-<version>.jar` in a directory on the worker's
`plugin.path`. The jar carries the gate and Chicory; Connect's own API, JSON converter and
logging come from the worker. Then add the transformation to a connector:

```properties
transforms=covenant
transforms.covenant.type=net.mancube.covenant.connect.CovenantGate
transforms.covenant.contract=/etc/covenant/orders.yaml
transforms.covenant.unique=skip
# a sink connector: blocked records go to a dead letter queue
errors.tolerance=all
errors.deadletterqueue.topic.name=orders_dlq
errors.deadletterqueue.context.headers.enable=true
```

| Setting | Default | |
|---|---|---|
| `contract` | (required) | The contract on the worker, `covenant: 1` or ODCS v3. |
| `model` | the only one | The model to enforce. |
| `unique` | `refuse` | `refuse` a model with `unique` fields, `skip` them, or `track` them across what this task judges. A task sees only its own partitions and starts afresh on a restart, so `track` holds `unique` within that and no further. |
| `on.block` | `fail` | `fail`: throw, with the dead letter as the message, for Connect's error handling. `drop`: drop the record and log its dead letter. |

The value is judged as JSON. A `String` or `byte[]` value (from the `StringConverter` or
`ByteArrayConverter`) is the JSON text itself. A `Map` or `Struct` (from the `JsonConverter`, or
a schema'd converter) is rendered to JSON first. The outcome depends on the verdict:

- A record that keeps the contract goes on unchanged.
- A blocked record fails with a `DataException` whose message is its dead letter. Connect then
  applies its own error handling. By default the task stops. With `errors.tolerance=all` and a
  dead letter queue, a sink connector sends the record there unchanged, and the dead letter
  arrives in the `__connect.errors.exception.message` header.
- A record under `on_violation: warn` goes on, carrying its dead letter in a
  `covenant.dead_letter` header.
- A tombstone goes on unjudged.

## Kroxylicious: refused at produce time

Kroxylicious is a Kafka proxy that runs filters on the protocol between clients and brokers.
The Covenant filter judges every record of a produce request, for the topics it is given,
before the request reaches a broker. This makes the producer boundary enforceable for any
client in any language, with no change to the producers:

```yaml
filterDefinitions:
  - name: covenant
    type: CovenantGateFilterFactory
    config:
      contract: /etc/covenant/orders.yaml
      topics: [orders]
      unique: skip              # or refuse (the default): a proxy judges each connection apart
defaultFilters:
  - covenant
```

Put `covenant-kroxylicious/target/covenant-kroxylicious-<version>.jar` on the proxy's
classpath, for example with `KROXYLICIOUS_CLASSPATH`. The contract is read, linted and compiled
when the proxy starts, and a contract that cannot be enforced exactly stops the proxy from
starting.

A partition's batch whose records all keep the contract goes on to the broker. A batch holding a
record that breaks the contract is refused whole, as a broker refuses a batch that fails its own
validation. The response carries `INVALID_RECORD` with one record error per broken record, and
each error message is that record's dead letter. A Java producer fails the broken record with an
`InvalidRecordException` whose message is the dead letter, and each clean neighbour with the
client's own "part of a batch which had one or more invalid records" error. Other partitions
of the same request are unaffected, and so are ungated topics and tombstones. The proxy judges
each connection apart, so `unique` can be refused or skipped but not tracked.

## Tested

- `mvn verify` runs the library's and the transformation's tests. Add
  `-Dcovenant.bin=<covenant binary>` to also hold the library to the command: on the demo
  orders, with `unique` skipped and tracked, the records a `Gate` passes and the dead letters
  it writes must equal what `covenant gate` does (timestamps aside).
- `e2e_connect.py` runs a real Connect worker (`connect-standalone`, from a Kafka distribution
  named by `KAFKA_HOME`) with a FileStreamSink behind the transformation and Connect's dead
  letter queue turned on. The records the sink writes must be exactly the ones
  `covenant gate --no-unique` passes. The queue must hold exactly the ones it blocks,
  unchanged, each with the gate's own dead letter as its exception message.

```bash
KAFKA_HOME=/opt/kafka KAFKA_BOOTSTRAP=127.0.0.1:9092 python3 e2e_connect.py \
  <covenant binary> covenant-connect/target/covenant-connect-0.1.0-SNAPSHOT.jar \
  ../../examples/contracts/orders.yaml
```

- `e2e_kroxylicious.py` starts a Kroxylicious proxy (named by `KROXYLICIOUS_HOME`) in front
  of a broker and produces through it with the console producer. The demo orders, one record
  per batch, must leave the broker holding exactly what `covenant gate --no-unique` passes,
  with each record it blocks refused to the producer with the gate's dead letter. A request
  over two partitions must keep its clean partition and refuse the other whole.

CI runs all three against Apache Kafka 4.1.0 and Kroxylicious 0.23.0.
