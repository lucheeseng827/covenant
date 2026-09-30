// Run covenant.wasm (the wasm32-wasip1 build of the `covenant` command) under
// Node's built-in WASI, with the current directory as the guest's root:
//
//   node ops/wasm-run.mjs path/to/covenant.wasm check data.ndjson -c contract.yaml
//
// stdin, stdout and stderr are the process's own, and the exit code is the
// command's, so it drops in wherever the native binary runs.
import { readFile } from "node:fs/promises";
import process from "node:process";
import v8 from "node:v8";
import { WASI } from "node:wasi";

// Node 22's WASI can start a garbage collection inside a V8 fast API call (its
// allocator reports external memory from `path_open`), and V8 then crashes
// walking the stack: a segfault, at a point that moves with the module's size.
// The plain call path is always safe. Set before the module is compiled.
v8.setFlagsFromString("--no-turbo-fast-api-calls");

const [wasmPath, ...args] = process.argv.slice(2);
const wasi = new WASI({
  version: "preview1",
  args: ["covenant", ...args],
  env: {},
  preopens: { "/": process.cwd() },
  returnOnExit: true,
});
const module = await WebAssembly.compile(await readFile(wasmPath));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exitCode = wasi.start(instance);
