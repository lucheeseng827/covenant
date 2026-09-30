#!/usr/bin/env python3
"""Render Covenant's Arroyo UDFs with a contract compiled in.

Arroyo builds a Rust UDF from one source file, so the contract goes into the
source itself. This checks the contract with the engine first, `covenant
validate` for the contract and `covenant gate` for the model, and then
writes covenant_verdict.rs and covenant_dead_letter.rs, ready to register
with Arroyo.

    python3 render.py <contract> --covenant <binary> [--model NAME] [--no-unique]
                      [--engine <path to the covenant-data crate>] [--out DIR]

A UDF judges each record on its own, so it cannot hold `unique`. Without
--no-unique, a model that declares `unique` fields makes the UDF fail on its
first call rather than skip them quietly. --engine builds against a local
checkout of the engine instead of the published one.
"""

import argparse
import pathlib
import re
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
UDFS = ("covenant_verdict", "covenant_dead_letter")
# The row engine alone: without the `arrow` feature the engine links no Arrow,
# whose chrono needs clash with the arrow the UDF plugin is built on.
GIT = 'covenant = { package = "covenant-data", git = "https://github.com/lucheeseng827/covenant", default-features = false }'


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("contract", type=pathlib.Path)
    parser.add_argument("--covenant", required=True, help="the covenant binary, to check the contract")
    parser.add_argument("--model", default="", help="the model, when the contract has more than one")
    parser.add_argument("--no-unique", action="store_true", help="skip the model's unique fields")
    parser.add_argument("--engine", type=pathlib.Path, help="a local checkout of the engine crate")
    parser.add_argument("--out", type=pathlib.Path, default=pathlib.Path("."))
    args = parser.parse_args()

    text = args.contract.read_text()
    if '"####' in text:
        sys.exit('render.py: the contract contains "####, which ends the string it is embedded in')
    checked = subprocess.run([args.covenant, "validate", str(args.contract)], capture_output=True, text=True)
    if checked.returncode != 0:
        sys.exit(f"render.py: the contract does not validate:\n{checked.stdout}{checked.stderr}")
    gate = [args.covenant, "gate", "-c", str(args.contract), "--no-unique", "-q"]
    if args.model:
        gate += ["-m", args.model]
    resolved = subprocess.run(gate, stdin=subprocess.DEVNULL, capture_output=True, text=True)
    if resolved.returncode != 0:
        sys.exit(f"render.py: the model does not resolve:\n{resolved.stderr}")

    dependency = GIT
    if args.engine:
        manifest = (args.engine / "Cargo.toml").read_text()
        package = re.search(r'^\[package\]\s*\nname = "([^"]+)"', manifest, re.M).group(1)
        dependency = f'covenant = {{ package = "{package}", path = "{args.engine.resolve()}", default-features = false }}'

    args.out.mkdir(parents=True, exist_ok=True)
    for name in UDFS:
        source = (HERE / "udf" / f"{name}.rs").read_text()
        source = (
            source.replace(GIT, dependency)
            .replace("__COVENANT_CONTRACT__", text)
            .replace("__COVENANT_ORIGIN__", args.contract.name.replace('"', ""))
            .replace("__COVENANT_MODEL__", args.model.replace('"', ""))
            .replace("__COVENANT_SKIP_UNIQUE__", "true" if args.no_unique else "false")
        )
        (args.out / f"{name}.rs").write_text(source)
        print(f"wrote {args.out / f'{name}.rs'}")


if __name__ == "__main__":
    main()
