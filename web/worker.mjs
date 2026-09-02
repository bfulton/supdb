// The worker logshed's reader runs in.
//
// It exists because `FileSystemSyncAccessHandle` does not exist on the main
// thread, and that handle is the whole of R2.2(a): it is what makes a browser
// byte fetch synchronous, which is what lets `flatindex::lookup` go on
// returning a borrow instead of a promise.
//
// The shape is: one asynchronous step at startup (download the object into
// OPFS -- or, for the cached source, fetch only what the open plans), then
// every query after that is synchronous inside the worker, except that the
// cached source's point reads want an `ensure` first: the module plans the
// ranges (R6.2), the ensure awaits fetching them, and the read itself is
// synchronous as ever. The await lives here, never inside wasm.

import { openSyncHandle, openMemory, openCached, openSparse, fetchIntoOpfs } from "./supdb.mjs";
import { CachedBytes, httpRangeFetcher } from "./cache.mjs";

let reader = null;
let handle = null;
let cache = null;

async function open({ wasmUrl, indexUrl, name, source, budgetBytes, pageSize, probeBytes, directory }) {
  const wasmBytes = await (await fetch(wasmUrl)).arrayBuffer();
  if (source === "memory") {
    const bytes = new Uint8Array(await (await fetch(indexUrl)).arrayBuffer());
    reader = await openMemory(wasmBytes, bytes);
    return { source: "memory", keys: reader.keys };
  }
  if (source === "cached") {
    cache = await CachedBytes.open({
      name,
      fetcher: httpRangeFetcher(indexUrl),
      budgetBytes,
    });
    reader = await openCached(wasmBytes, cache);
    return {
      source: "cached",
      keys: reader.keys,
      length: cache.length,
      openFetchedBytes: cache.stats.fetchedBytes,
    };
  }
  if (source === "sparse") {
    // R6.3: the index itself by range. Same cache, a different open.
    // A smaller page than the point-read cache's: the index is where the
    // sparse reader's bytes go, and w5-dict found the 64 KiB page rather
    // than the bytes to be its cost at logshed's dictionary sizes.
    cache = await CachedBytes.open({
      name,
      fetcher: httpRangeFetcher(indexUrl),
      budgetBytes,
      ...(pageSize ? { pageSize } : {}),
    });
    reader = await openSparse(wasmBytes, cache, {
      ...(probeBytes ? { probe: probeBytes } : {}),
      ...(directory ? { directory: true } : {}),
    });
    return {
      source: "sparse",
      keys: reader.keys,
      length: cache.length,
      openFetchedBytes: cache.stats.fetchedBytes,
    };
  }
  handle = await fetchIntoOpfs(indexUrl, name);
  reader = await openSyncHandle(wasmBytes, handle);
  return { source: "opfs", keys: reader.keys, size: handle.getSize() };
}

// FNV-1a 32, the same hash `logshed segment` records, so a multi-kilobyte
// lookup is checked byte-for-byte without shipping the bytes in the fixture.
function fnv32(values) {
  let h = 0x811c9dc5 >>> 0;
  let n = 0;
  for (const v of values) {
    n += 1;
    for (const b of v) {
      h = (h ^ b) >>> 0;
      h = Math.imul(h, 0x01000193) >>> 0;
    }
  }
  return { count: n, hash: h >>> 0 };
}

const plain = (rows) => rows.map(({ key, count }) => ({ key, count }));

const ops = {
  open,
  keys: () => reader.keys,
  indexBytes: () => reader.indexBytes,
  generation: () => reader.generation,
  lookup: ({ key }) =>
    reader.lookup(key).map((v) => Array.from(v)),
  lookupHash: ({ key }) => fnv32(reader.lookup(key)),
  count: ({ key }) => reader.count(key),
  countFixed: ({ key, width }) => reader.countFixed(key, width),
  storedBytes: ({ key }) => reader.storedBytes(key),
  // Rows cross the worker boundary as text and count: `keyBytes` is the
  // key and is what a caller passes back to a lookup (web/test/node.mjs
  // proves the text form of a byte key does not), but the browser suite
  // compares rows against the native fixture's text rows.
  scanCounts: ({ from, limit }) => plain(reader.scanCounts(from, limit)),
  scanCountsFixed: ({ from, limit, width }) =>
    plain(reader.scanCountsFixed(from, limit, width)),
  // R6.2: the plan, and the plan-then-fetch that makes reads miss-proof.
  planRanges: ({ keys }) => reader.planRanges(keys),
  ensure: async ({ keys }) => {
    await reader.ensure(keys);
    return true;
  },
  // R6.3: the dictionary by range, for the sparse source.
  dictCounts: ({ lo, hi }) => plain(reader.dictCounts(lo, hi ?? null)),
  ensureDict: async ({ lo, hi }) => {
    await reader.ensureDict(lo, hi ?? null);
    return true;
  },
  ensureDictValues: async ({ lo, hi }) => {
    await reader.ensureDictValues(lo, hi ?? null);
    return true;
  },
  dictReadHash: ({ key }) => {
    const r = reader.dictReadConcat(key);
    return { count: r.count, hash: fnv32([r.bytes]).hash };
  },
  cacheStats: () =>
    cache && {
      ...cache.stats,
      length: cache.length,
      budgetBytes: cache.budgetBytes,
      residentBytes: cache.residentBytes,
    },
  close: () => {
    if (reader) reader.close();
    if (handle) handle.close();
    if (cache) cache.close();
    reader = null;
    handle = null;
    cache = null;
    return true;
  },
};

self.onmessage = async (e) => {
  const { id, op, args } = e.data;
  try {
    const fn = ops[op];
    if (!fn) throw new Error(`supdb worker: no operation ${op}`);
    self.postMessage({ id, ok: await fn(args ?? {}) });
  } catch (err) {
    self.postMessage({ id, error: String(err && err.stack ? err.stack : err) });
  }
};
