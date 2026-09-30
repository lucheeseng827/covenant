// The JS API, held to the native command. Every case runs the native binary
// on real files and covenant.mjs on the same files held in memory, and
// compares stdout byte for byte, the exit code (which must also be the one
// the case expects) and every file the case writes. Run from anywhere:
//
//   node js/golden.mjs target/debug/covenant target/wasm32-wasip1/debug/covenant.wasm

import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { load } from "./covenant.mjs";

const [native, wasm] = process.argv.slice(2).map((p) => resolve(p));
if (!native || !wasm) {
  console.error("usage: node js/golden.mjs <native covenant> <covenant.wasm>");
  process.exit(2);
}
process.chdir(fileURLToPath(new URL("..", import.meta.url)));
const covenant = await load(wasm);

/** Every file under `dir`, by relative path. */
function tree(dir, out = {}) {
  for (const name of readdirSync(dir)) {
    const path = `${dir}/${name}`;
    if (statSync(path).isDirectory()) tree(path, out);
    else out[path] = readFileSync(path);
  }
  return out;
}

// A scratch directory under the working directory, for what cases write; the
// in-memory side gets the same path, so outputs that name it match as they are.
const work = mkdtempSync(".js-golden.");
process.on("exit", () => rmSync(work, { recursive: true, force: true }));
const csv = `order_id,amount_cents,currency,created_at
ord_a1b2c3d4e5f6,12999,USD,2026-08-11T09:30:00Z
ORD-UPPERCASE,-50,BTC,2026-08-11T09:31:00Z
ord_a1b2c3d4e5f6,100,USD,yesterday
`;
writeFileSync(`${work}/orders.csv`, csv);
const files = {
  ...tree("examples"),
  ...tree("conformance/odcs"),
  [`${work}/orders.csv`]: csv,
  [`${work}/`]: "",
};

const contract = "examples/contracts/orders.yaml";
const bad = "examples/data/orders_bad.ndjson";
const cases = [
  ["validate", 0, ["validate", contract]],
  ["check, clean NDJSON", 0, ["check", "examples/data/orders.ndjson", "-c", contract, "--format", "json"]],
  ["check, dirty NDJSON", 1, ["check", bad, "-c", contract, "--format", "json"]],
  ["check, human report", 1, ["check", bad, "-c", contract]],
  ["check, CSV (Arrow engine)", 1, ["check", `${work}/orders.csv`, "-c", contract, "--format", "json"]],
  ["gate, stdin to stdout", 1, ["gate", "-c", contract], { stdin: readFileSync(bad) }],
  ["gate --subprocess", 1, ["gate", "--subprocess", "-c", contract], { stdin: readFileSync(bad) }],
  ["diff", 1, ["diff", contract, "examples/contracts/orders_v2.yaml", "--format", "json"]],
  ["export postgres", 0, ["export", "postgres", "-c", contract]],
  ["conformance vectors", 0, ["conformance", "conformance/odcs", "--format", "json"]],
  [
    "check --profile",
    0,
    ["check", "examples/data/orders.ndjson", "-c", contract, "--profile", `${work}/profile.json`],
    { writes: [`${work}/profile.json`] },
  ],
  ["a missing file is a failed run", 2, ["check", "examples/data/nowhere.ndjson", "-c", contract]],
];

let failed = 0;
for (const [label, expect, args, { stdin = "", writes = [] } = {}] of cases) {
  const n = spawnSync(native, args, { input: stdin, maxBuffer: 1 << 28 });
  const j = await covenant.run(args, { files, stdin });
  const nOut = n.stdout.toString();
  const problems = [];
  if (n.status !== expect || j.code !== expect) {
    problems.push(`expected exit ${expect}, native exit ${n.status}, js exit ${j.code}`);
  }
  if (nOut !== j.stdout) problems.push(`stdout differs:\n--- native\n${nOut}\n--- js\n${j.stdout}`);
  for (const path of writes) {
    const theirs = j.files[path];
    if (!theirs || Buffer.compare(readFileSync(path), Buffer.from(theirs)) !== 0) {
      problems.push(`${path} differs`);
    }
  }
  if (problems.length) {
    failed = 1;
    console.log(`DIFFERS ${label}: ${problems.join("\n")}`);
    console.log(`native stderr: ${n.stderr.toString().slice(0, 400)}`);
    console.log(`js stderr: ${j.stderr.slice(0, 400)}`);
  } else {
    console.log(`same    ${label} (exit ${expect})`);
  }
}

// check() hands back the report the command prints for the same two files.
const yaml = readFileSync(contract, "utf8");
const viaApi = await covenant.check({
  contract: yaml,
  contractName: "orders.yaml",
  data: csv,
  dataName: "orders.csv",
});
writeFileSync(`${work}/orders.yaml`, yaml);
const viaCommand = spawnSync(native, ["check", "orders.csv", "-c", "orders.yaml", "--format", "json"], {
  cwd: work,
});
const expected = JSON.parse(viaCommand.stdout.toString())[0];
if (
  viaApi.code !== viaCommand.status ||
  JSON.stringify(viaApi.report) !== JSON.stringify(expected)
) {
  failed = 1;
  console.log(`DIFFERS check(): exit ${viaApi.code} vs ${viaCommand.status}`);
} else {
  console.log(`same    check() (exit ${viaApi.code}, ${viaApi.report.violations} violations)`);
}
const refused = await covenant.check({ contract: "not: a contract", data: csv });
if (refused.code !== 2 || refused.report !== null || !refused.stderr) {
  failed = 1;
  console.log("DIFFERS check() on a broken contract: expected exit 2, no report, a reason");
} else {
  console.log("same    check() on a broken contract (exit 2)");
}
// A Windows path is read as a file, never fetched as a URL whose scheme is the
// drive letter: here it names no file, so the read fails, not a fetch.
const windowsPath = await load("C:\\no\\such\\covenant.wasm").then(
  () => "loaded",
  (err) => err.code ?? `${err.name}: ${err.message}`,
);
if (windowsPath !== "ENOENT") {
  failed = 1;
  console.log(`DIFFERS load() on a Windows path: ${windowsPath}, expected ENOENT`);
} else {
  console.log("same    load() reads a Windows path as a file");
}

process.exit(failed);
