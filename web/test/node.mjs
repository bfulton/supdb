// The unit half of the web test: the same supdb.mjs and the same wasm module
// the browser opens, run in Node, which can instantiate wasm but has no OPFS.
// What lives here is what needs no browser -- above all, the error paths.
//
// It exists because every check in supdb.mjs was once dead. A wasm u32 return
// arrives in JavaScript as a *signed* i32 (a u64 as a signed BigInt), so a
// failure sentinel of u32::MAX arrived as -1 and a comparison against
// 4294967295 never matched. The consequence was not a missing message: a
// reader over an object that failed to open answered [] for every key, and a
// lookup whose block failed its checksum came back empty -- an under-return,
// the one thing this index may never do. Nothing in the browser suite could
// have seen it, because the browser suite only walks the happy path.
//
// Run after `logshed fixture` has written web/test/out and `web/build.sh`
// (or `cargo build --profile wasm ...`) has written web/supdb.wasm:
//
//   node web/test/node.mjs
//
// `web/test/run.sh` does all of that in order.

import { readFile } from "node:fs/promises";
import { openMemory } from "../supdb.mjs";

const here = new URL(".", import.meta.url);
const log = [];
let failures = 0;

function ok(name) {
  log.push(`ok   ${name}`);
}
function fail(name, detail) {
  log.push(`FAIL ${name}: ${detail}`);
  failures += 1;
}
function eq(name, got, want) {
  const g = JSON.stringify(got);
  const w = JSON.stringify(want);
  if (g === w) ok(name);
  else fail(name, `got ${g}, want ${w}`);
}

// The failure the signedness bug produced was not a wrong error but a wrong
// *answer*, so "it throws" is the whole assertion -- and the message is
// checked too, because a throw from the wrong layer (a TypeError out of the
// glue, say) would pass a bare "it threw" and still hide the real check.
async function throws(name, fn, contains) {
  try {
    await fn();
    fail(name, "expected a throw and got an answer");
  } catch (e) {
    const msg = String(e);
    if (contains && !msg.includes(contains)) {
      fail(name, `threw, but with: ${msg}`);
    } else {
      ok(name);
    }
  }
}

// A Buffer's .buffer is Node's shared pool, not the file -- slice out the
// real bytes before handing them to WebAssembly.instantiate.
async function fileBytes(url) {
  const b = await readFile(url);
  return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
}

async function main() {
  const wasm = await fileBytes(new URL("../supdb.wasm", here));

  // The requirements document's minimal repro, verbatim: four kilobytes of
  // zeroes is not a supdb store by any reading of the format, and before the
  // sentinel fix `openMemory` handed back a reader whose `keys` was -1 and
  // whose every lookup answered [].
  await throws(
    "a zeroed object refuses to open",
    () => openMemory(wasm, new Uint8Array(4096)),
    "checkpoint",
  );
  await throws(
    "a too-short object refuses to open",
    () => openMemory(wasm, new Uint8Array(16)),
    "too short",
  );

  // The happy path against the native reader's answers, so the sentinel
  // normalization is shown not to have bent a single correct value.
  const expected = JSON.parse(
    await readFile(new URL("./out/expected.json", here), "utf8"),
  );
  const day = new Uint8Array(await fileBytes(new URL("./out/day.supdb", here)));
  const reader = await openMemory(wasm, day);
  eq("keys", reader.keys, expected.keys);
  eq("indexBytes", reader.indexBytes, expected.index_bytes);
  for (const c of expected.lookups) {
    eq(
      `lookup ${c.key}`,
      reader.lookup(c.key).map((v) => Array.from(v)),
      c.values,
    );
  }
  for (const c of expected.counts) {
    eq(`count ${c.key}`, reader.count(c.key), c.count);
    eq(
      `countFixed ${c.key}`,
      reader.countFixed(c.key, expected.posting_bytes),
      c.count,
    );
    eq(`storedBytes ${c.key}`, reader.storedBytes(c.key), c.stored_bytes);
  }

  // A closed reader's handle is not a handle, and every arm of the ABI must
  // say so: the u32 sentinel (keys, lookup) and the u64 sentinel (count,
  // storedBytes) each had their own dead comparison.
  reader.close();
  await throws(
    "keys on a closed reader throws (the u32 sentinel)",
    () => reader.keys,
    "no open reader",
  );
  await throws(
    "lookup on a closed reader throws",
    () => reader.lookup("type=pageview"),
    "no open reader",
  );
  await throws(
    "count on a closed reader throws (the u64 sentinel)",
    () => reader.count("type=pageview"),
    "no open reader",
  );
  await throws(
    "storedBytes on a closed reader throws",
    () => reader.storedBytes("type=pageview"),
    "no open reader",
  );

  // The deeper half: one byte corrupted *inside* a block. The store still
  // opens -- header, key index and block table are untouched -- so the only
  // place the damage can surface is the read, and it must surface as an
  // error. The coordinates come from the fixture generator, because only the
  // native side knows which byte belongs to which key's extent; the same
  // shape is pinned natively in tests/blob.rs.
  const dam = day.slice();
  dam[expected.corrupt.at] ^= 0xff;
  const damaged = await openMemory(wasm, dam);
  eq(
    "a store with one corrupt block byte still opens",
    damaged.keys,
    expected.keys,
  );
  await throws(
    `lookup ${expected.corrupt.key} fails its checksum rather than answering empty`,
    () => damaged.lookup(expected.corrupt.key),
    "checksum",
  );
  // The count no longer walks the block: since format v5 it is read out of
  // the extent record, so damage inside the block cannot reach it and it
  // must still answer rather than fail.
  eq(
    "the count of the damaged key answers from the index",
    damaged.count(expected.corrupt.key) > 0,
    true,
  );
  // Repeated, because the first version of Blob::verify marked a chunk
  // verified before comparing it: the error fired once and the next read
  // served the corrupt bytes as already-verified.
  await throws(
    "and it fails again on the next read, not just the first",
    () => damaged.lookup(expected.corrupt.key),
    "checksum",
  );
  // A key in a different block still answers exactly: the damage is one
  // block's, not the file's.
  const intact = expected.counts.find(
    (c) => c.key === expected.corrupt.intact_key,
  );
  eq(
    `the intact key ${intact.key} still answers over the damage`,
    damaged.count(intact.key),
    intact.count,
  );
  damaged.close();
}

main()
  .then(() => {
    for (const l of log) console.log(l);
    if (failures > 0) {
      console.error(`\n${failures} node assertion(s) failed`);
      process.exit(1);
    }
    console.log(`\nOK: ${log.length} node assertions passed`);
  })
  .catch((e) => {
    for (const l of log) console.log(l);
    console.error(String(e.stack ?? e));
    process.exit(1);
  });
