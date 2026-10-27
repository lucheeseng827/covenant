#!/usr/bin/env bash
# The README's quickstart, as a check of one `covenant` however it was
# installed: it reports the version expected, lints the example contract,
# passes the clean example data and fails the dirty data with exit 1. Run from
# the repository root, since it reads examples/:
#
#   quickstart.sh <version> <the command that runs covenant…>
#
#   quickstart.sh 0.2.0 covenant
#   quickstart.sh 0.2.0 target/x86_64-unknown-linux-musl/release/covenant
#   quickstart.sh 0.2.0 docker run --rm -u "$(id -u):$(id -g)" -v "$PWD:/w" -w /w mancube/covenant:0.2.0
#
# It also drafts a contract with `infer --out`, because a check that only reads proves
# nothing about writing: the image runs as uid 65532, and its documented quick start
# could not write into the directory it was given until the docs ran it as the owner.
set -uo pipefail

fail() {
    echo "::error title=quickstart::$*"
    exit 1
}

[ $# -ge 2 ] || fail "usage: quickstart.sh <version> <command…>"
version=$1
shift
contract=examples/contracts/orders.yaml

said=$("$@" --version) || fail "'$* --version' failed"
[ "$said" = "covenant $version" ] || fail "'$* --version' says '$said', not 'covenant $version'"
"$@" validate "$contract" || fail "the example contract does not lint"
"$@" check examples/data/orders.ndjson -c "$contract" || fail "the clean example data does not pass"
"$@" check examples/data/orders_bad.ndjson -c "$contract" >/dev/null
code=$?
[ "$code" -eq 1 ] || fail "the dirty example data exits $code, not 1"
draft=.quickstart-draft.yaml
rm -f "$draft"
"$@" infer examples/data/orders.ndjson --out "$draft" >/dev/null 2>&1 \
    || fail "'infer --out' could not write a draft contract into the working directory"
[ -s "$draft" ] || fail "'infer --out' exited 0 but wrote no draft"
rm -f "$draft"
echo "quickstart passed: $said"
