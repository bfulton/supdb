// The browser reader held to the cases `tests/hostile.rs` builds: the
// pre-tag hostile-input audit's segments, each opened three ways
// (openMemory, openCached, openSparse over an in-memory cache) and read
// to its end. A case marked `refuse` must be refused at the open by every
// way, with an Error; a case marked `hold` may open or refuse, and an
// opened reader answers every key's count as written or throws. A
// WebAssembly.RuntimeError is the module trapping (a panic under
// panic=abort), which kills the instance, and fails every case.
//
//   SUPDB_HOSTILE_DUMP=DIR cargo test --profile checked --test hostile
//   node web/hostile.test.mjs WASM DIR
//
// WASM is the module web/build.sh builds.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { openCached, openMemory, openSparse } from "./supdb.mjs";

const [wasmPath, dir] = process.argv.slice(2);
if (!wasmPath || !dir) {
  console.error("usage: node web/hostile.test.mjs WASM DIR");
  process.exit(2);
}
const wasm = readFileSync(wasmPath);
const cases = readFileSync(join(dir, "cases.tsv"), "utf8")
  .trim()
  .split("\n")
  .map((l) => l.split("\t"));
const models = new Map();
function model(shape) {
  if (!models.has(shape)) {
    const rows = JSON.parse(readFileSync(join(dir, `shape${shape}.json`), "utf8"));
    models.set(
      shape,
      rows.map(([k, n]) => [Uint8Array.from(Buffer.from(k, "hex")), n]),
    );
  }
  return models.get(shape);
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

const fails = [];
let opened = 0;
let refused = 0;

// `f`'s outcome: its value, or "error" for an Error thrown. A trap is a
// failure of the case, whatever it was marked.
async function attempt(name, what, f) {
  try {
    return { ok: await f() };
  } catch (e) {
    if (e instanceof WebAssembly.RuntimeError) {
      fails.push(`TRAP ${name} ${what}: ${e.message}`);
      return { trap: true };
    }
    if (!(e instanceof Error) || /is not a function/.test(e.message)) {
      fails.push(`DRIVER ${name} ${what}: ${e}`);
    }
    return { error: e };
  }
}

for (const [file, shape, verdict, name] of cases) {
  const bytes = new Uint8Array(readFileSync(join(dir, file)));
  const m = model(shape);
  const pristine = name.startsWith("pristine");
  for (const [how, open] of [
    ["memory", () => openMemory(wasm, bytes)],
    ["cached", () => openCached(wasm, memCache(bytes))],
    ["sparse", () => openSparse(wasm, memCache(bytes))],
  ]) {
    const r = await attempt(name, `${how} open`, open);
    if (r.trap) continue;
    if (r.error) {
      refused++;
      if (pristine) fails.push(`REFUSED ${name} ${how}: ${r.error.message}`);
      continue;
    }
    opened++;
    if (verdict === "refuse") fails.push(`OPENED ${name} ${how}`);
    const rd = r.ok;
    const st = await attempt(name, `${how} reads`, async () => {
      if (how === "sparse") {
        await rd.ensureDict(new Uint8Array(0), null);
        rd.dictCounts(new Uint8Array(0), null);
        return;
      }
      for (const [k, want] of m) {
        if (rd.cache) await rd.ensure([k]);
        const got = await attempt(name, `${how} count`, () => rd.count(k));
        if (got.trap) return;
        if (got.ok !== undefined && Number(got.ok) !== want) {
          fails.push(
            `MISREAD ${name} ${how}: count(${Buffer.from(k).toString("hex")}) = ${got.ok}, want ${want}`,
          );
          return;
        }
      }
    });
    if (!st.trap) await attempt(name, `${how} close`, () => rd.close());
  }
}

for (const f of fails.slice(0, 40)) console.log(f);
console.log(
  `hostile: ${cases.length} cases, ${opened} opened, ${refused} refused, ${fails.length} failures`,
);
process.exit(fails.length ? 1 : 0);
