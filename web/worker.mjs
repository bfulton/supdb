// The worker logshed's reader runs in.
//
// It exists because `FileSystemSyncAccessHandle` does not exist on the main
// thread, and that handle is the whole of R2.2(a): it is what makes a browser
// byte fetch synchronous, which is what lets `flatindex::lookup` go on
// returning a borrow instead of a promise.
//
// The shape is: one asynchronous step at startup (download the object into
// OPFS), then every query after that is synchronous inside the worker and
// asynchronous only in the sense that it is on another thread.

import { openSyncHandle, openMemory, fetchIntoOpfs } from "./supdb.mjs";

let reader = null;
let handle = null;

async function open({ wasmUrl, indexUrl, name, source }) {
  const wasmBytes = await (await fetch(wasmUrl)).arrayBuffer();
  if (source === "memory") {
    const bytes = new Uint8Array(await (await fetch(indexUrl)).arrayBuffer());
    reader = await openMemory(wasmBytes, bytes);
    return { source: "memory", keys: reader.keys };
  }
  handle = await fetchIntoOpfs(indexUrl, name);
  reader = await openSyncHandle(wasmBytes, handle);
  return { source: "opfs", keys: reader.keys, size: handle.getSize() };
}

const ops = {
  open,
  keys: () => reader.keys,
  indexBytes: () => reader.indexBytes,
  generation: () => reader.generation,
  lookup: ({ key }) =>
    reader.lookup(key).map((v) => Array.from(v)),
  count: ({ key }) => reader.count(key),
  countFixed: ({ key, width }) => reader.countFixed(key, width),
  storedBytes: ({ key }) => reader.storedBytes(key),
  scanCounts: ({ from, limit }) => reader.scanCounts(from, limit),
  scanCountsFixed: ({ from, limit, width }) =>
    reader.scanCountsFixed(from, limit, width),
  close: () => {
    if (reader) reader.close();
    if (handle) handle.close();
    reader = null;
    handle = null;
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
