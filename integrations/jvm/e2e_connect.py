#!/usr/bin/env python3
"""The Kafka Connect transformation in a real Connect worker, held to `covenant gate`.

Starts a standalone Connect worker whose FileStreamSink reads a topic through
the CovenantGate transformation, with Connect's own dead letter queue turned
on, and sends the demo orders through it. The records the sink writes must be
exactly the ones `covenant gate --no-unique` passes. Each blocked record must
reach Connect's DLQ unchanged, and its `__connect.errors.exception.message`
header must be the dead letter `covenant gate` writes for it, apart from the
timestamp.

    KAFKA_HOME=<a Kafka distribution> python3 e2e_connect.py \\
        <covenant binary> <covenant-connect jar> <contract>

The broker is KAFKA_BOOTSTRAP (default 127.0.0.1:9092); it must let the test
create topics. Only the standard library and the distribution's own tools are
used.
"""

import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time

HERE = pathlib.Path(__file__).resolve().parent
DATA = HERE.parent.parent / "examples" / "data"
KAFKA = pathlib.Path(os.environ.get("KAFKA_HOME", "")).resolve()
BROKERS = os.environ.get("KAFKA_BOOTSTRAP", "127.0.0.1:9092")
# Separators the DLQ's headers and values never contain: record, header,
# value. The console consumer joins a header's name and value with ":",
# which no header name here contains.
RS, HS, VS = "\x1c", "\x1e", "\x1d"


def tool(name, *args, stdin=None):
    done = subprocess.run(
        [str(KAFKA / "bin" / name), "--bootstrap-server", BROKERS, *args],
        input=stdin, capture_output=True, text=True, timeout=180,
    )
    if done.returncode != 0:
        sys.exit(f"{name} {' '.join(args)} failed:\n{done.stderr}")
    return done.stdout


def stop(process):
    """SIGTERM a process group; SIGKILL it if it is still running a minute later."""
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=60)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


def end_offset(topic):
    out = tool("kafka-get-offsets.sh", "--topic", topic)
    return sum(int(line.rsplit(":", 1)[1]) for line in out.split() if line.startswith(topic + ":"))


def without_ts(letter):
    head, _, tail = letter.partition('"ts":"')
    return head + tail.partition('",')[2]


def main():
    if len(sys.argv) != 4 or not (KAFKA / "bin" / "connect-standalone.sh").exists():
        sys.exit(__doc__)
    covenant, jar, contract = sys.argv[1], sys.argv[2], str(pathlib.Path(sys.argv[3]).resolve())
    lines = []
    for name in ("orders.ndjson", "orders_bad.ndjson"):
        lines += [line for line in (DATA / name).read_text().splitlines() if line.strip()]

    work = pathlib.Path(tempfile.mkdtemp(prefix="covenant-connect-"))
    worker = None
    suffix = f"{int(time.time())}-{os.getpid()}"
    raw, dlq = f"covenant-connect-raw-{suffix}", f"covenant-connect-dlq-{suffix}"
    failures = []
    try:
        # What the command decides.
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
        blocked = [line for line in lines if line not in passed]

        for topic in (raw, dlq):
            tool("kafka-topics.sh", "--create", "--topic", topic, "--partitions", "1",
                 "--replication-factor", "1")
        tool("kafka-console-producer.sh", "--topic", raw, stdin="".join(line + "\n" for line in lines))

        plugins = work / "plugins"
        plugins.mkdir()
        shutil.copy(jar, plugins)
        for file_connector in (KAFKA / "libs").glob("connect-file-*.jar"):
            shutil.copy(file_connector, plugins)
        (work / "worker.properties").write_text(
            f"bootstrap.servers={BROKERS}\n"
            "key.converter=org.apache.kafka.connect.storage.StringConverter\n"
            "value.converter=org.apache.kafka.connect.storage.StringConverter\n"
            f"offset.storage.file.filename={work / 'offsets'}\n"
            f"plugin.path={plugins}\n"
            "listeners=http://127.0.0.1:0\n"
        )
        (work / "sink.properties").write_text(
            f"name=covenant-e2e-{suffix}\n"
            "connector.class=org.apache.kafka.connect.file.FileStreamSinkConnector\n"
            "tasks.max=1\n"
            f"topics={raw}\n"
            f"file={work / 'out.ndjson'}\n"
            "transforms=covenant\n"
            "transforms.covenant.type=net.mancube.covenant.connect.CovenantGate\n"
            f"transforms.covenant.contract={contract}\n"
            "transforms.covenant.unique=skip\n"
            "errors.tolerance=all\n"
            f"errors.deadletterqueue.topic.name={dlq}\n"
            "errors.deadletterqueue.topic.replication.factor=1\n"
            "errors.deadletterqueue.context.headers.enable=true\n"
        )
        log = open(work / "worker.log", "w")
        worker = subprocess.Popen(
            [str(KAFKA / "bin" / "connect-standalone.sh"), str(work / "worker.properties"),
             str(work / "sink.properties")],
            stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
        )

        out = work / "out.ndjson"
        deadline = time.time() + 120
        while not (out.exists() and len(out.read_text().splitlines()) >= len(passed)
                   and end_offset(dlq) >= len(letters)):
            if worker.poll() is not None or time.time() > deadline:
                sys.exit(f"the worker did not deliver:\n{(work / 'worker.log').read_text()[-3000:]}")
            time.sleep(1)

        written = out.read_text().splitlines()
        if written != passed:
            failures.append(f"the sink wrote {len(written)} records, not the {len(passed)} `covenant gate` passes")
        dumped = tool(
            "kafka-console-consumer.sh", "--topic", dlq, "--from-beginning",
            "--max-messages", str(len(letters)), "--timeout-ms", "30000",
            "--property", "print.headers=true", "--property", "print.value=true",
            "--property", f"headers.separator={HS}", "--property", f"key.separator={VS}",
            "--property", f"line.separator={RS}",
        )
        records = [r for r in dumped.split(RS) if r.strip()]
        values, messages = [], []
        for record in records:
            headers, _, value = record.partition(VS)
            values.append(value)
            fields = dict(h.split(":", 1) for h in headers.split(HS) if ":" in h)
            messages.append(without_ts(fields.get("__connect.errors.exception.message", "")))
        if values != blocked:
            failures.append("the DLQ does not hold exactly the records `covenant gate` blocks, unchanged")
        if messages != letters:
            first = next((i for i, (a, b) in enumerate(zip(messages, letters)) if a != b), None)
            detail = f" (first difference, record {first}: {messages[first]!r} vs {letters[first]!r})" if first is not None else f" ({len(messages)} vs {len(letters)})"
            failures.append("the DLQ's exception messages are not the dead letters `covenant gate` writes" + detail)
    finally:
        if worker is not None and worker.poll() is None:
            stop(worker)
        # Each topic is deleted even if the other's delete times out; a timeout is reported
        # with the test's own failures.
        for topic in (raw, dlq):
            try:
                subprocess.run(
                    [str(KAFKA / "bin" / "kafka-topics.sh"), "--bootstrap-server", BROKERS,
                     "--delete", "--topic", topic],
                    capture_output=True, timeout=120,
                )
            except subprocess.TimeoutExpired:
                failures.append(f"cleanup timed out deleting {topic}")
        shutil.rmtree(work, ignore_errors=True)

    if failures:
        sys.exit("FAILED\n  " + "\n  ".join(failures))
    print(
        f"ok  a Connect worker's sink wrote the {len(passed)} records `covenant gate` passes, and "
        f"its dead letter queue holds the {len(blocked)} it blocks, each with the gate's own "
        "dead letter as the exception message"
    )


if __name__ == "__main__":
    main()
