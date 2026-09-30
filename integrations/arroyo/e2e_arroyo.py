#!/usr/bin/env python3
"""The UDFs in a real Arroyo, held to `covenant gate`.

Renders the two UDFs with the demo contract (against the engine in this
checkout), starts an Arroyo cluster, registers them through its API so that
Arroyo's own compiler builds them with the published UDF plugin, and runs a
pipeline over the demo orders:

    INSERT INTO validated    SELECT value FROM orders WHERE covenant_verdict(value) <> 'block';
    INSERT INTO dead_letters SELECT covenant_dead_letter(value) AS value FROM orders
                             WHERE covenant_dead_letter(value) IS NOT NULL;

The records `validated` receives must be exactly the ones `covenant gate
--no-unique` passes. The dead letters must be the gate's own, apart from
their timestamps and row numbers: a UDF judges each record on its own, so
its rows are 0.

    python3 e2e_arroyo.py <covenant binary> <arroyo binary>

Needs cargo on the PATH (Arroyo compiles UDFs with it) and Arroyo's default
ports (5114-5118) free. Only the standard library is used.
"""

import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
ENGINE = HERE.parent.parent
CONTRACT = ENGINE / "examples" / "contracts" / "orders.yaml"
DATA = ENGINE / "examples" / "data"
API = "http://127.0.0.1:5115/api/v1"


def call(method, path, body=None, timeout=120):
    request = urllib.request.Request(
        API + path, method=method, headers={"content-type": "application/json"},
        data=None if body is None else json.dumps(body).encode(),
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()
    except (urllib.error.URLError, ConnectionError):
        return None, None


def normalized(letter):
    """A dead letter without its timestamp, its row numbers set to 0."""
    head, _, tail = letter.partition('"ts":"')
    letter = head + tail.partition('",')[2]
    return re.sub(r"\brow (\d+):", "row 0:", re.sub(r'"row":\d+', '"row":0', letter))


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    covenant, arroyo = sys.argv[1], sys.argv[2]
    work = pathlib.Path(tempfile.mkdtemp(prefix="covenant-arroyo-"))
    lines = []
    for name in ("orders.ndjson", "orders_bad.ndjson"):
        lines += [line for line in (DATA / name).read_text().splitlines() if line.strip()]
    (work / "in.ndjson").write_text("".join(line + "\n" for line in lines))

    with open(work / "in.ndjson") as stdin:
        gate = subprocess.run(
            [covenant, "gate", "-c", str(CONTRACT), "--no-unique", "-q", "--dlq", str(work / "cli.dlq")],
            stdin=stdin, capture_output=True, text=True, timeout=300,
        )
    if gate.returncode not in (0, 1):
        sys.exit(f"covenant gate failed:\n{gate.stderr}")
    passed = gate.stdout.splitlines()
    letters = [normalized(line) for line in (work / "cli.dlq").read_text().splitlines()]

    subprocess.run(
        [sys.executable, str(HERE / "render.py"), str(CONTRACT), "--covenant", covenant,
         "--no-unique", "--engine", str(ENGINE), "--out", str(work / "udf")],
        check=True, capture_output=True, timeout=300,
    )

    env = {
        **os.environ,
        "ARROYO__CHECKPOINT_URL": str(work / "checkpoints"),
        "ARROYO__COMPILER__ARTIFACT_URL": str(work / "artifacts"),
        "ARROYO__COMPILER__BUILD_DIR": str(work / "build"),
        "ARROYO__COMPILER__INSTALL_CLANG": "false",
        "ARROYO__COMPILER__INSTALL_RUSTC": "false",
        "ARROYO__DATABASE__SQLITE__PATH": str(work / "arroyo.sqlite"),
    }
    log = open(work / "cluster.log", "w")
    cluster = subprocess.Popen([arroyo, "cluster"], env=env, stdout=log, stderr=subprocess.STDOUT,
                               start_new_session=True, cwd=work)
    failures = []
    try:
        deadline = time.time() + 120
        while call("GET", "/ping")[0] != 200:
            if cluster.poll() is not None or time.time() > deadline:
                sys.exit(f"Arroyo did not start:\n{(work / 'cluster.log').read_text()[-3000:]}")
            time.sleep(1)

        for name in ("covenant_verdict", "covenant_dead_letter"):
            source = (work / "udf" / f"{name}.rs").read_text()
            status, udf = call("POST", "/udfs", {"prefix": "", "definition": source, "description": name},
                               timeout=1800)
            if status != 200:
                sys.exit(f"Arroyo refused {name}: {udf}\n{(work / 'cluster.log').read_text()[-3000:]}")

        query = f"""
            CREATE TABLE orders (value TEXT) WITH (connector = 'single_file',
                path = '{work / "in.ndjson"}', format = 'raw_string', type = 'source');
            CREATE TABLE validated (value TEXT) WITH (connector = 'single_file',
                path = '{work / "out.ndjson"}', format = 'raw_string', type = 'sink');
            CREATE TABLE dead_letters (value TEXT) WITH (connector = 'single_file',
                path = '{work / "dlq.ndjson"}', format = 'raw_string', type = 'sink');
            INSERT INTO validated SELECT value FROM orders WHERE covenant_verdict(value) <> 'block';
            INSERT INTO dead_letters SELECT covenant_dead_letter(value) AS value FROM orders
                WHERE covenant_dead_letter(value) IS NOT NULL;
        """
        status, pipeline = call("POST", "/pipelines", {"name": "covenant-e2e", "query": query, "parallelism": 1})
        if status != 200:
            sys.exit(f"Arroyo refused the pipeline: {pipeline}")
        deadline = time.time() + 600
        while True:
            status, jobs = call("GET", f"/pipelines/{pipeline['id']}/jobs")
            state = jobs["data"][0]["state"] if status == 200 and jobs.get("data") else None
            if state in ("Finished", "Failed", "Stopped"):
                break
            if time.time() > deadline:
                sys.exit(f"the pipeline did not finish (state {state})")
            time.sleep(2)
        if state != "Finished":
            sys.exit(f"the pipeline ended {state}: {jobs['data'][0].get('failureMessage')}")

        validated = (work / "out.ndjson").read_text().splitlines()
        dead = [normalized(line) for line in (work / "dlq.ndjson").read_text().splitlines()]
        if validated != passed:
            failures.append(f"validated holds {len(validated)} records, not the {len(passed)} `covenant gate` passes")
        if dead != letters:
            failures.append("the dead letters differ from the ones `covenant gate` writes")
    finally:
        if cluster.poll() is None:
            os.killpg(cluster.pid, signal.SIGTERM)
            try:
                cluster.wait(timeout=60)
            except subprocess.TimeoutExpired:
                os.killpg(cluster.pid, signal.SIGKILL)

    if failures:
        sys.exit("FAILED\n  " + "\n  ".join(failures) + f"\n(work dir, kept: {work})")
    shutil.rmtree(work, ignore_errors=True)
    print(
        f"ok  in Arroyo, covenant_verdict passed the {len(passed)} records `covenant gate` passes and "
        f"covenant_dead_letter wrote the {len(letters)} dead letters it writes"
    )


if __name__ == "__main__":
    main()
