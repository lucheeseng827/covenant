#!/usr/bin/env bash
# The WASM face, held to the native command. Every case runs the native binary
# and the wasm32-wasip1 build (under Node's WASI, wasm-run.mjs beside this
# script) and compares stdout byte for byte; both sides must also exit with
# the code the case expects, so a case that fails the same way on both (its
# input moved, say) fails the run. Files a case writes, such as profiles, are
# compared too. Run from the repository root:
#
#   ops/wasm-golden.sh target/release/covenant target/wasm32-wasip1/release/covenant.wasm
set -euo pipefail

native=$1
wasm=$2
runner="$(dirname "$0")/wasm-run.mjs"
# Under the working directory, which is all the WASM side is granted; a fresh
# name per run, so nothing already there is touched.
work=$(mktemp -d .wasm-golden.XXXXXX)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/native" "$work/wasm"

# CSV takes the Arrow engine; the examples are NDJSON, which takes the row one.
cat > "$work/orders.csv" <<'CSV'
order_id,amount_cents,currency,created_at
ord_a1b2c3d4e5f6,12999,USD,2026-08-11T09:30:00Z
ORD-UPPERCASE,-50,BTC,2026-08-11T09:31:00Z
ord_a1b2c3d4e5f6,100,USD,yesterday
CSV

contract=examples/contracts/orders.yaml
failed=0

# case_ <label> <expected exit> <stdin file, or -> <args…>; `@` in an argument
# stands for the side's own output directory, for commands that write files.
case_() {
  local label=$1 expect=$2 input=$3
  shift 3
  local side code native_code wasm_code
  for side in native wasm; do
    local args=("${@//@/$work/$side}")
    local cmd=("$native")
    [ "$side" = wasm ] && cmd=(node --no-warnings "$runner" "$wasm")
    set +e
    # stderr is kept, not compared: the stream gate's dead letters carry the
    # time they were written.
    "${cmd[@]}" "${args[@]}" >"$work/$side.out" 2>"$work/$side.err" <"${input/#-//dev/null}"
    code=$?
    set -e
    if [ "$side" = native ]; then native_code=$code; else wasm_code=$code; fi
  done
  # Paths a case names differ only in the side's directory.
  sed -i "s#$work/wasm#$work/native#g" "$work/wasm.out"
  if [ "$native_code" != "$expect" ] || [ "$wasm_code" != "$expect" ]; then
    echo "WRONG   $label: expected exit $expect, native exit $native_code, wasm exit $wasm_code"
    head -5 "$work/native.err" "$work/wasm.err" || true
    failed=1
  elif cmp -s "$work/native.out" "$work/wasm.out"; then
    echo "same    $label (exit $expect)"
  else
    echo "DIFFERS $label (exit $expect)"
    diff "$work/native.out" "$work/wasm.out" | head -20 || true
    failed=1
  fi
}

case_ "validate"                    0 - validate "$contract"
case_ "check, clean NDJSON"         0 - check examples/data/orders.ndjson -c "$contract" --format json
case_ "check, dirty NDJSON"         1 - check examples/data/orders_bad.ndjson -c "$contract" --format json
case_ "check, human report"         1 - check examples/data/orders_bad.ndjson -c "$contract"
case_ "check, CSV (Arrow engine)"   1 - check "$work/orders.csv" -c "$contract" --format json
case_ "gate, stdin to stdout"       1 examples/data/orders_bad.ndjson gate -c "$contract"
case_ "gate --subprocess"           1 examples/data/orders_bad.ndjson gate --subprocess -c "$contract"
case_ "diff"                        1 - diff "$contract" examples/contracts/orders_v2.yaml --format json
case_ "export postgres"             0 - export postgres -c "$contract"
case_ "conformance vectors"         0 - conformance conformance/odcs --format json
case_ "check --profile"             0 - check examples/data/orders.ndjson -c "$contract" --profile @/profile.json

if cmp -s "$work/native/profile.json" "$work/wasm/profile.json"; then
  echo "same    profile file"
else
  echo "DIFFERS profile file"
  failed=1
fi

exit "$failed"
