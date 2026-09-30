#!/usr/bin/env python3
"""The transform deployed into a real Redpanda and held to `covenant gate`.

Deploys the module this directory builds and sends records through its input
topic: the demo orders, clean and dirty, each with a key and a header, and a
tombstone. Then it checks what arrives:

- the records that keep the contract, on the first output topic, byte for
  byte with their keys and headers, and the tombstone, still a tombstone;
- a dead letter for each record that breaks it, on the DLQ topic, with the
  record's key and headers.

It also runs `covenant gate --no-unique` over the same records. The records
the gate passes must be exactly the ones the transform passes, and the dead
letters must be the gate's own, apart from their timestamps.

    python3 e2e.py <covenant binary> <transform .wasm> <contract the .wasm embeds>

rpk must be on the PATH (or named by RPK), pointed at a cluster with data
transforms enabled: RPK_BROKERS and RPK_ADMIN, by default 127.0.0.1:9092 and
127.0.0.1:9644. Only the standard library is used.
"""

import json
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile
import time

HERE = pathlib.Path(__file__).resolve().parent
DATA = HERE.parent.parent / "examples" / "data"
RPK = shlex.split(os.environ.get("RPK", "rpk")) + [
    "-X", "brokers=" + os.environ.get("RPK_BROKERS", "127.0.0.1:9092"),
    "-X", "admin.hosts=" + os.environ.get("RPK_ADMIN", "127.0.0.1:9644"),
]


def rpk(*args, stdin=None):
    done = subprocess.run(RPK + list(args), input=stdin, capture_output=True, text=True, timeout=120)
    if done.returncode != 0:
        sys.exit(f"rpk {' '.join(args)} failed:\n{done.stdout}{done.stderr}")
    return done.stdout


def high_watermark(topic):
    lines = rpk("topic", "describe", topic, "-p").splitlines()
    return int(lines[1].split()[-1])


def consume(topic, count):
    """`count` records from the start of `topic`, as rpk's JSON renders them."""
    if count == 0:
        return []
    out = rpk("topic", "consume", topic, "-o", "start", "-n", str(count), "-f", "json")
    decoder, records, at = json.JSONDecoder(), [], 0
    while at < len(out):
        if out[at].isspace():
            at += 1
            continue
        record, at = decoder.raw_decode(out, at)
        records.append(record)
    return records


def without_ts(letter):
    letter = json.loads(letter)
    letter.pop("ts")
    return letter


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    covenant, wasm, contract = sys.argv[1:]
    lines = []
    for name in ("orders.ndjson", "orders_bad.ndjson"):
        lines += [line for line in (DATA / name).read_text().splitlines() if line.strip()]
    keys = [f"k{i}" for i in range(len(lines))]

    # What the command decides, record by record.
    with tempfile.TemporaryDirectory() as tmp:
        letters_file = pathlib.Path(tmp) / "dlq.ndjson"
        gate = subprocess.run(
            [covenant, "gate", "-c", contract, "--no-unique", "-q", "--dlq", str(letters_file)],
            input="".join(line + "\n" for line in lines),
            capture_output=True,
            text=True,
            check=False,
            timeout=300,
        )
        if gate.returncode not in (0, 1):
            sys.exit(f"covenant gate failed:\n{gate.stderr}")
        passed = gate.stdout.splitlines()
        letters = [without_ts(line) for line in letters_file.read_text().splitlines()]
    passed_keys = [keys[lines.index(line)] for line in passed]
    letter_keys = [keys[letter["row"]] for letter in letters]

    suffix = f"{int(time.time())}-{os.getpid()}"
    raw, clean, dlq = (f"covenant-e2e-{t}-{suffix}" for t in ("raw", "clean", "dlq"))
    name = f"covenant-e2e-{suffix}"
    rpk("topic", "create", raw, clean, dlq, "-p", "1")
    failures = []
    try:
        rpk("transform", "deploy", "--file", wasm, "--name", name, "--input-topic", raw,
            "--output-topic", clean, "--output-topic", dlq, "--var", f"COVENANT_DLQ_TOPIC={dlq}")
        deadline = time.time() + 60
        while f"{name} " not in rpk("transform", "list") or " 1 / 1 " not in rpk("transform", "list"):
            if time.time() > deadline:
                sys.exit(f"the transform never ran:\n{rpk('transform', 'list')}")
            time.sleep(1)

        records = "".join(f"{key} {line}\n" for key, line in zip(keys, lines))
        rpk("topic", "produce", raw, "-f", "%k %v\n", "-H", "trace:t-1", stdin=records)
        rpk("topic", "produce", raw, "-f", "%k %v\n", "-Z", stdin="gone \n")

        want_clean, want_dlq = len(passed) + 1, len(letters)
        deadline = time.time() + 60
        while (high_watermark(clean), high_watermark(dlq)) != (want_clean, want_dlq):
            if time.time() > deadline:
                sys.exit(
                    f"expected {want_clean} records on {clean} and {want_dlq} on {dlq}, "
                    f"found {high_watermark(clean)} and {high_watermark(dlq)}"
                )
            time.sleep(0.5)

        forwarded = consume(clean, want_clean)
        tombstone = forwarded.pop()
        if tombstone.get("key") != "gone" or "value" in tombstone:
            failures.append(f"the tombstone arrived as {tombstone}")
        if [r["value"] for r in forwarded] != passed:
            failures.append("the records that went on differ from the ones `covenant gate` passes")
        if [r.get("key") for r in forwarded] != passed_keys:
            failures.append(f"keys on {clean}: {[r.get('key') for r in forwarded]} != {passed_keys}")
        for r in forwarded + consume(dlq, want_dlq):
            if r.get("headers") != [{"key": "trace", "value": "t-1"}]:
                failures.append(f"headers lost: {r}")
        dead = consume(dlq, want_dlq)
        if [without_ts(r["value"]) for r in dead] != letters:
            failures.append("the dead letters differ from the ones `covenant gate` writes")
        if [r.get("key") for r in dead] != letter_keys:
            failures.append(f"keys on {dlq}: {[r.get('key') for r in dead]} != {letter_keys}")
    finally:
        # Each step runs even if the one before it times out; a timeout is reported with the
        # test's own failures.
        for step in (
            ["transform", "delete", name, "--no-confirm"],
            ["topic", "delete", raw, clean, dlq],
        ):
            try:
                subprocess.run(RPK + step, capture_output=True, timeout=120)
            except subprocess.TimeoutExpired:
                failures.append(f"cleanup timed out: rpk {' '.join(step)}")

    if failures:
        sys.exit("FAILED\n  " + "\n  ".join(failures))
    print(
        f"ok  {len(lines)} records and a tombstone through the transform: {len(passed)} passed "
        f"and {len(letters)} dead-lettered, as `covenant gate` decides; the tombstone passed on"
    )


if __name__ == "__main__":
    main()
