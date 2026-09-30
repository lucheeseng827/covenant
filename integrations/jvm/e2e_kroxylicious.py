#!/usr/bin/env python3
"""The Kroxylicious filter in a real proxy, held to `covenant gate`.

Starts a Kroxylicious proxy in front of a broker with the CovenantGate filter
gating two topics, and produces through it with the Kafka console producer
(an idempotent Java producer).

1. The demo orders, one record per batch. The broker must end up with exactly
   the records `covenant gate --no-unique` passes. Each record it blocks must
   be refused to the producer with an InvalidRecordException whose message is
   the dead letter `covenant gate` writes for it, apart from the timestamp.
2. One request over two partitions: one partition's batch keeps the contract,
   the other's holds a record that breaks it. The first must reach the broker.
   The second must be refused whole, as a broker refuses a batch that fails
   its own validation: the broken record with its dead letter, its clean
   neighbour as part of a refused batch.

    KAFKA_HOME=<Kafka distribution> KROXYLICIOUS_HOME=<Kroxylicious distribution> \\
        python3 e2e_kroxylicious.py <covenant binary> <covenant-kroxylicious jar> <contract>

The broker is KAFKA_BOOTSTRAP (default 127.0.0.1:9092); the proxy listens on
127.0.0.1:29192 and the ports just above it. Only the standard library and the
distributions' own tools are used.
"""

import os
import pathlib
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

HERE = pathlib.Path(__file__).resolve().parent
DATA = HERE.parent.parent / "examples" / "data"
KAFKA = pathlib.Path(os.environ.get("KAFKA_HOME", "")).resolve()
PROXY_HOME = pathlib.Path(os.environ.get("KROXYLICIOUS_HOME", "")).resolve()
BROKERS = os.environ.get("KAFKA_BOOTSTRAP", "127.0.0.1:9092")
PROXY = "127.0.0.1:29192"
INVALID = "org.apache.kafka.common.InvalidRecordException: "
PART_OF_BATCH = "part of a batch which had one more more invalid records"


def tool(name, *args, bootstrap=BROKERS, stdin=None, check=True):
    done = subprocess.run(
        [str(KAFKA / "bin" / name), "--bootstrap-server", bootstrap, *args],
        input=stdin, capture_output=True, text=True, timeout=180,
    )
    if check and done.returncode != 0:
        sys.exit(f"{name} {' '.join(args)} failed:\n{done.stderr}")
    return done


def stop(process):
    """SIGTERM a process group; SIGKILL it if it is still running a minute later."""
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=60)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


def consume(topic):
    """(partition, key, value) for every record in `topic`, read from the broker itself."""
    out = tool(
        "kafka-console-consumer.sh", "--topic", topic, "--from-beginning", "--timeout-ms", "10000",
        "--property", "print.partition=true", "--property", "print.key=true", check=False,
    ).stdout
    records = []
    for line in out.splitlines():
        partition, key, value = line.split("\t", 2)
        records.append((int(partition.removeprefix("Partition:")), key, value))
    return records


def without_ts(letter):
    head, _, tail = letter.partition('"ts":"')
    return head + tail.partition('",')[2]


def produce(topic, lines, *properties):
    """Produce through the proxy; the producer's error lines."""
    done = tool(
        "kafka-console-producer.sh", "--topic", topic, *properties,
        bootstrap=PROXY, stdin="".join(line + "\n" for line in lines),
    )
    return done.stderr.splitlines()


def main():
    if len(sys.argv) != 4 or not (PROXY_HOME / "bin" / "kroxylicious-start.sh").exists():
        sys.exit(__doc__)
    covenant, jar, contract = sys.argv[1], sys.argv[2], str(pathlib.Path(sys.argv[3]).resolve())
    lines = []
    for name in ("orders.ndjson", "orders_bad.ndjson"):
        lines += [line for line in (DATA / name).read_text().splitlines() if line.strip()]

    work = pathlib.Path(tempfile.mkdtemp(prefix="covenant-kroxylicious-"))
    suffix = f"{int(time.time())}-{os.getpid()}"
    one, two = f"covenant-krox-one-{suffix}", f"covenant-krox-two-{suffix}"
    proxy = None
    failures = []
    try:
        (work / "in.ndjson").write_text("".join(line + "\n" for line in lines))
        with open(work / "in.ndjson") as stdin:
            gate = subprocess.run(
                [covenant, "gate", "-c", contract, "--no-unique", "-q", "--dlq", str(work / "cli.dlq")],
                stdin=stdin, capture_output=True, text=True, timeout=300,
            )
        if gate.returncode not in (0, 1):
            sys.exit(f"covenant gate failed:\n{gate.stderr}")
        passed = gate.stdout.splitlines()
        letters = [without_ts(line) for line in (work / "cli.dlq").read_text().splitlines()]

        tool("kafka-topics.sh", "--create", "--topic", one, "--partitions", "1", "--replication-factor", "1")
        tool("kafka-topics.sh", "--create", "--topic", two, "--partitions", "2", "--replication-factor", "1")
        (work / "proxy.yaml").write_text(
            "virtualClusters:\n"
            "  - name: covenant\n"
            "    targetCluster:\n"
            f"      bootstrapServers: {BROKERS}\n"
            "    gateways:\n"
            "      - name: default\n"
            "        portIdentifiesNode:\n"
            f"          bootstrapAddress: {PROXY}\n"
            "filterDefinitions:\n"
            "  - name: covenant\n"
            "    type: CovenantGateFilterFactory\n"
            "    config:\n"
            f"      contract: {contract}\n"
            f"      topics: [{one}, {two}]\n"
            "      unique: skip\n"
            "defaultFilters:\n"
            "  - covenant\n"
        )
        log = open(work / "proxy.log", "w")
        proxy = subprocess.Popen(
            [str(PROXY_HOME / "bin" / "kroxylicious-start.sh"), "--config", str(work / "proxy.yaml")],
            stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
            env={**os.environ, "KROXYLICIOUS_CLASSPATH": str(pathlib.Path(jar).resolve())},
        )
        deadline = time.time() + 90
        while "Kroxylicious is started" not in (work / "proxy.log").read_text():
            if proxy.poll() is not None or time.time() > deadline:
                sys.exit(f"the proxy did not start:\n{(work / 'proxy.log').read_text()[-3000:]}")
            time.sleep(1)
        host, port = PROXY.split(":")
        socket.create_connection((host, int(port)), timeout=10).close()

        # 1. One record per batch.
        errors = produce(one, lines, "--producer-property", "batch.size=1")
        refused = [without_ts(e[len(INVALID):]) for e in errors if e.startswith(INVALID)]
        landed = [value for _, _, value in consume(one)]
        if landed != passed:
            failures.append(f"the broker holds {len(landed)} records, not the {len(passed)} `covenant gate` passes")
        if refused != letters:
            failures.append(
                f"the producer was refused {len(refused)} records, not with the {len(letters)} dead "
                "letters `covenant gate` writes"
            )

        # 2. One request, two partitions ("d" and "f" hash to partition 1, "a" and "b" to 0).
        clean = [line for line in passed][:3]
        mixed = [f"d|{clean[0]}", f"a|{clean[1]}", 'b|{"order_id":"bad"}', f"f|{clean[2]}"]
        errors = produce(
            two, mixed, "--property", "parse.key=true", "--property", "key.separator=|",
            "--producer-property", "linger.ms=3000", "--producer-property", "batch.size=65536",
        )
        landed = consume(two)
        if sorted((p, k) for p, k, _ in landed) != [(1, "d"), (1, "f")]:
            failures.append(f"of the mixed request, the broker holds {[(p, k) for p, k, _ in landed]}, not d and f")
        if sum(e.startswith(INVALID) for e in errors) != 1 or sum(PART_OF_BATCH in e for e in errors) != 1:
            failures.append("the refused batch did not fail as one broken record and one neighbour")
    finally:
        if proxy is not None and proxy.poll() is None:
            stop(proxy)
        # Each topic is deleted even if the other's delete times out; a timeout is reported
        # with the test's own failures.
        for topic in (one, two):
            try:
                tool("kafka-topics.sh", "--delete", "--topic", topic, check=False)
            except subprocess.TimeoutExpired:
                failures.append(f"cleanup timed out deleting {topic}")

    if failures:
        sys.exit("FAILED\n  " + "\n  ".join(failures) + f"\n(proxy log, kept: {work / 'proxy.log'})")
    shutil.rmtree(work, ignore_errors=True)
    print(
        f"ok  through the proxy, the broker got the {len(passed)} records `covenant gate` passes and "
        f"the producer was refused the {len(letters)} it blocks, each with its dead letter; a "
        "request over two partitions kept the clean one and refused the other whole"
    )


if __name__ == "__main__":
    main()
