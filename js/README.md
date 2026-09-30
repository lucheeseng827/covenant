# covenant.mjs — the command as a JavaScript API

`covenant.mjs` runs `covenant.wasm`, the `covenant` command built for `wasm32-wasip1`, over an
in-memory filesystem. The files a call passes are all the command can read; what it writes
comes back in the result; nothing touches a disk or leaves the page. It is one dependency-free
ES module for browsers and Node 20+, and it runs the same build CI holds byte for byte to the
native binary, so a verdict here is the command's verdict.

```js
import { load } from "./covenant.mjs";

const covenant = await load(new URL("./covenant.wasm", import.meta.url)); // compiled once

// covenant check, with the JSON report as an object.
const { code, report, stderr } = await covenant.check({
  contract: contractText,          // covenant: 1 or ODCS v3
  data: csvText,
  dataName: "orders.csv",          // the extension picks the reader: .csv, .ndjson, .jsonl, .json
});
// code: 0 passed, 1 violated, 2 the run failed (report is null; stderr says why)

// Any command line, over any files: a path ending in "/" is an empty directory.
const run = await covenant.run(["check", "orders.csv", "-c", "orders.yaml", "--profile", "out/p.json"], {
  files: { "orders.yaml": contractText, "orders.csv": csvText, "out/": "" },
});
run.code;                  // the exit code
run.stdout, run.stderr;    // text
run.files["out/p.json"];   // every file after the run, as bytes
```

`load` takes a URL (in Node, also a path), a `Response`, the bytes, or a compiled
`WebAssembly.Module`. Each call runs a fresh instance, so calls share nothing. `stdin` (text or
bytes) feeds `covenant gate`.

## Build and test

```bash
cargo build --release --target wasm32-wasip1 --bin covenant
cargo build --bin covenant
node js/golden.mjs target/debug/covenant target/wasm32-wasip1/release/covenant.wasm
```

`golden.mjs` runs every case of the WASM golden test through this module, over files held in
memory, against the native binary on the same files on disk: stdout byte for byte, the exit
code each case expects, and the files a case writes (a profile). It also holds `check()` to the
command's JSON report.

## The playground

`playground.html` is a page on this module: paste a contract and some CSV or NDJSON, get the
command's report. The engine runs in the page, so nothing is uploaded. Module scripts need
HTTP, so serve the directory with the build beside it:

```bash
cp target/wasm32-wasip1/release/covenant.wasm js/
python3 -m http.server -d js 8000     # then open http://localhost:8000/playground.html
```

## Scope

It runs the command's WASI build, the one the golden test covers, rather than a second build
for the browser, so there is one engine to keep right. What that build cannot do, this cannot
either: no network, no `serve`, no threads. The filesystem implements what the command uses
(reading, writing, listing and renaming files) and nothing else.
