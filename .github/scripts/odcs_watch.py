#!/usr/bin/env python3
"""Keep Covenant's ODCS reader from falling behind the standard.

Synced from the source repo as .github/scripts/odcs_watch.py and run weekly by
.github/workflows/odcs-watch.yml. Edit it at ops/odcs_watch.py in the source
tree. It needs a checkout of the standard with its full history, and a built
covenant binary:

    python3 odcs_watch.py --covenant target/debug/covenant --standard path/to/odcs

1. A revision is published when the standard adds its JSON schema,
   schema/odcs-json-schema-vX.Y.Z.json. The commit that added the file dates
   the revision.
2. For each published revision from v3 on, the binary is asked whether it
   reads a contract of that revision without refusing its apiVersion.
3. A revision it does not read is due 60 days after publication. With
   --open-issues, an issue is opened for its review, once. Past the due date,
   the watch fails.
4. The binary reads each of the standard's published examples. The watch
   fails if an example of a revision Covenant reads does not read at all.

The report goes to stdout, and to $GITHUB_STEP_SUMMARY when that is set.
"""

import argparse
import datetime as dt
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SCHEMA_FILE = re.compile(r"^odcs-json-schema-(v(\d+)\.(\d+)\.(\d+))\.json$")
API_VERSION = re.compile(r"^apiVersion:\s*[\"']?([^\"'\s#]+)", re.M)

PROBE = """\
apiVersion: {version}
kind: DataContract
id: odcs-watch-probe
version: 1.0.0
schema:
  - name: probe
    properties:
      - name: id
        logicalType: string
"""

ISSUE = """\
The Open Data Contract Standard published {version} on {published}. Covenant reads \
contracts of the revisions it has been reviewed against and refuses the others by \
`apiVersion`, so contracts that declare {version} are refused until this review is done. \
It is due by **{due}**, {days} days after publication.

- [ ] Read the revision's changelog entry and the pages it changed.
- [ ] For each change that promises something about values: map it exactly in \
`src/odcs.rs`, or refuse it with a reason.
- [ ] Add a conformance vector under `conformance/odcs/` for each newly enforced rule.
- [ ] Refresh `tests/fixtures/odcs/bitol/` from the standard's examples (see its `SOURCE.md`).
- [ ] Add {version} to `VERSIONS` in `src/odcs.rs` and record the review under \
"Revisions" in `docs/ODCS.md`.

Opened by the ODCS watch workflow.
"""


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=True
    ).stdout


def published(standard: Path) -> dict:
    """Every published revision from v3 on, with the date its schema was added."""
    if git(standard, "rev-parse", "--is-shallow-repository").strip() == "true":
        sys.exit(
            f"{standard}: a shallow checkout cannot date revisions; fetch its full history"
        )
    revisions = {}
    for path in (standard / "schema").glob("odcs-json-schema-v*.json"):
        m = SCHEMA_FILE.match(path.name)
        if not m or int(m.group(2)) < 3:
            continue
        added = git(
            standard, "log", "--diff-filter=A", "--format=%cs", "--", f"schema/{path.name}"
        ).split()
        if not added:
            sys.exit(f"{path}: not committed, so it cannot be dated")
        # Newest first: a file removed and added again dates from its first add.
        revisions[m.group(1)] = dt.date.fromisoformat(added[-1])
    return dict(sorted(revisions.items(), key=lambda kv: key(kv[0])))


def key(version: str) -> tuple:
    return tuple(int(p) for p in version.lstrip("v").split("."))


def reads(covenant: str, version: str, tmp: Path) -> bool:
    """Whether the binary reads a contract of `version` without refusing its apiVersion."""
    probe = tmp / f"probe-{version}.odcs.yaml"
    probe.write_text(PROBE.format(version=version))
    run = subprocess.run(
        [covenant, "validate", "--format", "json", str(probe)], capture_output=True, text=True
    )
    if run.returncode == 2:
        return False
    return not any(f["path"] == "apiVersion" for f in json.loads(run.stdout))


def examples(covenant: str, standard: Path) -> list:
    """(path, apiVersion, error or None) for every published example contract."""
    out = []
    root = standard / "docs" / "examples"
    for path in sorted(root.rglob("*.odcs.yaml")):
        m = API_VERSION.search(path.read_text(errors="replace"))
        rel = path.relative_to(root)
        run = subprocess.run(
            [covenant, "validate", "--allow-unenforced", "--format", "json", str(rel)],
            capture_output=True,
            text=True,
            cwd=root,
        )
        error = None
        if run.returncode == 2:
            lines = run.stderr.strip().splitlines()
            error = lines[-1] if lines else "exit 2"
        out.append((rel, m.group(1) if m else "?", error))
    return out


def open_issue(version: str, published_on: dt.date, due: dt.date, days: int) -> str:
    title = f"ODCS {version}: review and support"
    found = subprocess.run(
        ["gh", "issue", "list", "--state", "all", "--search", f'"{title}" in:title',
         "--json", "title,url"],
        capture_output=True, text=True, check=True,
    )
    for issue in json.loads(found.stdout):
        if issue["title"] == title:
            return issue["url"]
    body = ISSUE.format(version=version, published=published_on, due=due, days=days)
    made = subprocess.run(
        ["gh", "issue", "create", "--title", title, "--body", body],
        capture_output=True, text=True, check=True,
    )
    return made.stdout.strip()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--covenant", required=True, help="the covenant binary")
    ap.add_argument("--standard", required=True, type=Path,
                    help="a full-history checkout of the standard's repository")
    ap.add_argument("--days", type=int, default=60, help="days from publication to support")
    ap.add_argument("--today", type=dt.date.fromisoformat,
                    default=dt.datetime.now(dt.timezone.utc).date())
    ap.add_argument("--open-issues", action="store_true",
                    help="open a review issue for each revision not read yet (needs gh)")
    args = ap.parse_args()
    # Examples run from their own directory, so their paths read short.
    covenant = shutil.which(args.covenant)
    if covenant is None:
        sys.exit(f"{args.covenant}: not found, or not executable")
    covenant = str(Path(covenant).resolve())

    revisions = published(args.standard)
    failed = False
    lines = [f"## ODCS watch — {args.today}", "",
             "| Revision | Published | Covenant | Due |", "|---|---|---|---|"]
    supported = set()
    with tempfile.TemporaryDirectory() as tmp:
        for version, published_on in revisions.items():
            if reads(covenant, version, Path(tmp)):
                supported.add(version)
                lines.append(f"| {version} | {published_on} | reads | — |")
                continue
            due = published_on + dt.timedelta(days=args.days)
            left = (due - args.today).days
            status = f"{due} ({left} days left)" if left >= 0 else f"{due} (**{-left} days late**)"
            if args.open_issues:
                status += f" · {open_issue(version, published_on, due, args.days)}"
            lines.append(f"| {version} | {published_on} | **does not read** | {status} |")
            if left < 0:
                failed = True
                print(f"::error::ODCS {version} was due {due} and Covenant still refuses it")
            else:
                print(f"::warning::ODCS {version} is not read yet; due {due}")

    broken = []
    ran = examples(covenant, args.standard)
    for path, version, error in ran:
        if error is None:
            continue
        broken.append(f"- `{path}` ({version}): {error}")
        if version in supported:
            failed = True
            print(f"::error::{path} ({version}, a revision Covenant reads) does not read: {error}")
    lines += ["", f"Published examples: {len(ran) - len(broken)} of {len(ran)} read."]
    if broken:
        lines += ["", "Examples that do not read:", *broken]

    report = "\n".join(lines) + "\n"
    print(report)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write(report)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
