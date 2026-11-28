// The browser reader (web/supdb.mjs over the wasm module) fed the mutated
// segments the harness dumps (`HOSTILE_DUMP=DIR hostile segment ...`), each
// opened three ways -- openMemory, openCached and openSparse over an
// in-memory cache -- and read to its end: every key of its shape by
// lookup, readConcat, count, countFixed and storedBytes, the scans, and
// the sparse dictionary. A reader must throw an Error or answer; a
// WebAssembly.RuntimeError is the module trapping (a panic under
// panic=abort, an out-of-bounds access or `unreachable`), which kills the
// instance, and is the failure counted here. For the `crcoff` cases, whose
// changes the key section's row would have caught, an opened reader's
// counts are held to the model.
//
//   node release/findings/hostile-input/wasm/drive.mjs WASM DIR [FROM TO]
//
// WASM is the module web/build.sh builds (its cargo line, into a target
// directory of your own).

import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const here = resolve(new URL(".", import.meta.url).pathname);
const web = pathToFileURL(join(here, "../../../../web/supdb.mjs")).href;
const { openMemory, openCached, openSparse } = await import(web);

const [wasmPath, dir, fromArg, toArg] = process.argv.slice(2);
const wasm = readFileSync(wasmPath);
const cases = readFileSync(join(dir, "cases.tsv"), "utf8")
  .trim()
  .split("\n")
  .map((l) => l.split("\t"));
const from = Number(fromArg ?? 0);
const to = Number(toArg ?? cases.length);
const models = [];
for (let i = 0; ; i++) {
  try {
    models.push(
      JSON.parse(readFileSync(join(dir, `shape${i}.json`), "utf8")).map(([k, n]) => [
        Uint8Array.from(Buffer.from(k, "hex")),
        n,
      ]),
    );
  } catch {
    break;
  }
}

function memCache(bytes) {
  return {
    length: bytes.length,
    async ensure() {},
    readInto(off, dst) {
      if (off < 0 || off + dst.length > bytes.length) {
        throw new Error(`read ${off}+${dst.length} past ${bytes.length}`);
      }
      dst.set(bytes.subarray(off, off + dst.length));
    },
  };
}

const tally = { cases: 0, opened: 0, traps: [], misreads: [], jsErrors: [] };
const extra = [new Uint8Array(0), new Uint8Array(200).fill(0xff), new TextEncoder().encode("absent")];

function trapped(e) {
  return e instanceof WebAssembly.RuntimeError;
}

async function attempt(name, what, f) {
  try {
    return await f();
  } catch (e) {
    if (trapped(e)) {
      tally.traps.push(`${name} ${what}: ${e.message}`);
      return "trap";
    }
    if (!(e instanceof Error) || /is not a function|undefined|null/.test(e.message)) {
      tally.jsErrors.push(`${name} ${what}: ${e}`);
    }
    return "error";
  }
}

for (let i = from; i < Math.min(to, cases.length); i++) {
  const [n, shape, name] = cases[i];
  const bytes = new Uint8Array(readFileSync(join(dir, `${n}.sup`)));
  const model = models[Number(shape)];
  const check = name.startsWith("crcoff");
  tally.cases++;
  let opened = false;

  // openMemory: the whole object in the module's memory.
  for (const [how, open] of [
    ["memory", () => openMemory(wasm, bytes)],
    ["cached", () => openCached(wasm, memCache(bytes))],
  ]) {
    const r = await attempt(name, `${how} open`, open);
    if (r === "trap" || r === "error") continue;
    opened = true;
    const rd = r;
    const st = await attempt(name, `${how} reads`, async () => {
      for (const [k, want] of model) {
        if (rd.cache) await rd.ensure([k]);
        const c = rd.count(k);
        if (check && Number(c) !== want && tally.misreads.length < 50) {
          tally.misreads.push(`${name} ${how}: count(${Buffer.from(k).toString("hex")}) = ${c}, want ${want}`);
        }
        rd.lookup(k);
        rd.readConcat(k);
        rd.countFixed(k, 8);
        rd.storedBytes(k);
      }
      for (const k of extra) {
        if (rd.cache) await rd.ensure([k]);
        rd.count(k);
        rd.lookup(k);
      }
      if (!rd.cache) {
        rd.scanCounts(new Uint8Array(0), 100000);
        rd.scanCountsFixed(new Uint8Array(0), 100000, 8);
      }
    });
    if (st !== "trap") await attempt(name, `${how} close`, () => rd.close());
  }

  // openSparse: the dictionary planned and read by range.
  const sp = await attempt(name, "sparse open", () => openSparse(wasm, memCache(bytes)));
  if (sp !== "trap" && sp !== "error") {
    opened = true;
    const st = await attempt(name, "sparse reads", async () => {
      await sp.ensureDict(new Uint8Array(0), null);
      sp.dictCounts(new Uint8Array(0), null);
      await sp.ensureDictValues(new Uint8Array(0), null);
      for (const [k] of model.slice(0, 40)) sp.dictReadConcat(k);
    });
    if (st !== "trap") await attempt(name, "sparse close", () => sp.close());
  }
  if (opened) tally.opened++;
  if ((i + 1) % 1000 === 0) {
    console.error(`${i + 1} cases: ${tally.traps.length} traps`);
  }
}

for (const t of tally.traps) console.log(`TRAP ${t}`);
for (const m of tally.misreads) console.log(`MISREAD ${m}`);
for (const j of tally.jsErrors.slice(0, 20)) console.log(`JSERROR ${j}`);
console.log(
  `wasm ${from}..${to}: ${tally.cases} cases, ${tally.opened} opened, ` +
    `${tally.traps.length} traps, ${tally.misreads.length} misreads, ${tally.jsErrors.length} driver errors`,
);
process.exit(tally.traps.length || tally.misreads.length ? 1 : 0);
