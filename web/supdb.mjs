// The browser library: a supdb reader over a byte source, in JavaScript.
//
// There is no binding generator here. The wasm module exports eleven C
// functions that pass integers and byte ranges, and this file is the whole
// glue for them -- about two hundred lines against the descriptor sections,
// shims and reflection a generator would add to something the size budget in
// R3.3 is explicitly about. `web/build.sh` measures what ships.
//
// Three byte sources, one API:
//
//   openMemory(bytes)   the object is already an ArrayBuffer in this realm
//   openSyncHandle(h)   the object is a file in OPFS and `h` is a
//                       FileSystemSyncAccessHandle over it
//   openCached(cache)   the object stays in object storage and `cache` is a
//                       CachedBytes (cache.mjs) holding only the parts
//                       queries touch, under a byte budget
//
// The second and third are what logshed uses and the reason nothing here is
// async past `load`/`ensure`. `FileSystemSyncAccessHandle.read(buf, {at})` is
// a synchronous random read, so the Rust side never has to await, and
// `flatindex` can go on handing back borrows into the index. Both are only
// available inside a Web Worker, which is where `worker.mjs` runs them.
//
// The third source works because a lookup is already a plan (R6.2): the
// module names the byte ranges a query will touch *before* reading any of
// them (`supdb_open_plan` for the open, `supdb_ranges` for a set of keys),
// JavaScript awaits fetching those ranges into the cache, and the read then
// runs synchronously and cannot miss. The await lives out here; nothing
// inside the module ever suspends, so no Asyncify, no JSPI, no size cost.

const NO_HANDLE = -1;

/// One instantiated module, bound to one byte source.
///
/// Bound, because the host import is fixed when the module is instantiated;
/// an index is one instance. They are small -- see the size record -- and a
/// browser holds one per open day index.
class Module {
  constructor(instance, source) {
    this.instance = instance;
    this.source = source;
    this.enc = new TextEncoder();
    this.dec = new TextDecoder();
  }

  // Never cache this. Growing the wasm memory detaches the old ArrayBuffer,
  // and a stale view reads zeroes or throws.
  get mem() {
    return new Uint8Array(this.instance.exports.memory.buffer);
  }

  get view() {
    return new DataView(this.instance.exports.memory.buffer);
  }

  lastError() {
    const e = this.instance.exports;
    const len = e.supdb_error_len();
    if (len === 0) return "supdb: unspecified failure";
    const ptr = e.supdb_error_ptr();
    return this.dec.decode(this.mem.subarray(ptr, ptr + len));
  }

  // Copy a key into wasm memory. Keys are short and there are a handful per
  // query, so a scratch allocation per call is not what this costs.
  withKey(key, f) {
    const bytes = typeof key === "string" ? this.enc.encode(key) : key;
    const e = this.instance.exports;
    const ptr = e.supdb_alloc(bytes.length);
    try {
      this.mem.set(bytes, ptr);
      return f(ptr, bytes.length);
    } finally {
      e.supdb_free(ptr, bytes.length);
    }
  }

  // Decode a range frame from the out buffer: u32 n, then n pairs of
  // (u32 off, u32 len). Absolute file offsets -- nothing here assumes the
  // caller holds any other part of the file.
  ranges() {
    const base = this.instance.exports.supdb_out_ptr();
    const dv = this.view;
    const n = dv.getUint32(base, true);
    const out = new Array(n);
    for (let i = 0; i < n; i++) {
      out[i] = [dv.getUint32(base + 4 + i * 8, true), dv.getUint32(base + 8 + i * 8, true)];
    }
    return out;
  }
}

/// The reader logshed calls. R4.
export class SupdbReader {
  constructor(mod, handle) {
    this.mod = mod;
    this.handle = handle;
  }

  get exports() {
    return this.mod.instance.exports;
  }

  // Every wasm return crosses this. A wasm u32 arrives in JavaScript as a
  // *signed* i32 and a u64 as a signed BigInt, so a failure sentinel of
  // u32::MAX arrives as -1 and a comparison against 4294967295 can never
  // match -- which meant every error check in this file was dead and a
  // failed call was indistinguishable from an empty answer. The convention
  // is: normalize to unsigned at the boundary, compare unsigned, return the
  // unsigned value. `web/test/node.mjs` carries the zeroed-object repro.
  check(v, sentinel) {
    const u = typeof v === "bigint" ? BigInt.asUintN(64, v) : v >>> 0;
    if (u === sentinel) throw new Error(this.mod.lastError());
    return u;
  }

  /// R4.5
  get keys() {
    return this.check(this.exports.supdb_keys(this.handle), 0xffffffff);
  }

  /// R4.5
  get indexBytes() {
    return this.check(this.exports.supdb_index_bytes(this.handle), 0xffffffff);
  }

  get generation() {
    return this.check(this.exports.supdb_generation(this.handle), 0xffffffff);
  }

  /// R4.2 -- every value of a key, in append order.
  lookup(key) {
    const m = this.mod;
    const n = m.withKey(key, (p, l) =>
      this.check(this.exports.supdb_lookup(this.handle, p, l), 0xffffffff),
    );
    if (n === 0) return [];
    const base = this.exports.supdb_out_ptr();
    const dv = m.view;
    const count = dv.getUint32(base, true);
    const out = new Array(count);
    let at = base + 4;
    for (let i = 0; i < count; i++) {
      const len = dv.getUint32(at, true);
      at += 4;
      // Sliced, not subarrayed: a view into wasm memory is invalidated by the
      // next call that grows it, and handing one out is a bug that only shows
      // up under load.
      out[i] = m.mem.slice(at, at + len);
      at += len;
    }
    return out;
  }

  /// Every value of a key concatenated into one buffer, with the count.
  ///
  /// `lookup` frames one view per record, which for a common trigram is
  /// hundreds of thousands of allocations; this crosses the boundary once
  /// and copies once. Fixed-width values are then one typed array:
  /// `new Uint32Array(readConcat(key).bytes.buffer)` for logshed's postings.
  readConcat(key) {
    const m = this.mod;
    const n = m.withKey(key, (p, l) =>
      this.check(this.exports.supdb_read_concat(this.handle, p, l), 0xffffffff),
    );
    if (n === 0) return { count: 0, bytes: new Uint8Array(0) };
    const base = this.exports.supdb_out_ptr();
    const count = m.view.getUint32(base, true);
    // Sliced, not subarrayed, for the same reason as `lookup`.
    return { count, bytes: m.mem.slice(base + 4, base + n) };
  }

  /// R4.3 -- how many values, without decoding any of them.
  ///
  /// O(values) but not O(bytes): it walks the length prefixes and skips the
  /// payload, and -- the part that matters here -- nothing crosses the wasm
  /// boundary per value. See `countFixed` for the O(extents) form.
  count(key) {
    const v = this.mod.withKey(key, (p, l) =>
      this.check(this.exports.supdb_count(this.handle, p, l), 0xffffffffffffffffn),
    );
    return Number(v);
  }

  /// R4.3, the fast form: the count for a fixed-width posting list, derived
  /// from the extent list with no block touched at all.
  ///
  /// `null` when the key's values are not all `width` bytes, in which case
  /// fall back to `count`. logshed's postings are four-byte line ordinals.
  countFixed(key, width) {
    const v = this.mod.withKey(key, (p, l) =>
      this.exports.supdb_count_fixed(this.handle, p, l, width),
    );
    // Signed on arrival, like every i64 return -- normalize before comparing.
    if (BigInt.asUintN(64, v) === 0xffffffffffffffffn) return null;
    return Number(v);
  }

  /// Stored bytes under a key: payload plus one length prefix per value.
  /// O(extents). This is the input to "is the index cheaper than a scan".
  storedBytes(key) {
    const v = this.mod.withKey(key, (p, l) =>
      this.check(this.exports.supdb_stored_bytes(this.handle, p, l), 0xffffffffffffffffn),
    );
    return Number(v);
  }

  /// R4.4 -- the dictionary in key order from `from`, with each key's count.
  /// What a "top paths" or "countries" panel is made of.
  ///
  /// This form costs a walk over every posting in the range, which for a day
  /// index is the whole file. Prefer `scanCountsFixed` for a posting list.
  scanCounts(from, limit) {
    return this.scanFrame((p, l) =>
      this.exports.supdb_scan_counts(this.handle, p, l, limit),
      from,
    );
  }

  /// The same, counted in O(extents) for a fixed-width posting list.
  ///
  /// Bounded by the dictionary rather than by the traffic, and no block is
  /// touched. A key whose values are not all `width` bytes comes back with
  /// `count: null`; fall back to `count(key)` for that one key.
  scanCountsFixed(from, limit, width) {
    return this.scanFrame(
      (p, l) => this.exports.supdb_scan_counts_fixed(this.handle, p, l, limit, width),
      from,
    );
  }

  /// R6.2 -- the byte ranges a read of these keys will touch, before any of
  /// them is read. Sorted, merged, absolute file offsets. What `ensure`
  /// hands to the cache; exposed for callers that batch their own fetching.
  ///
  /// Covers data reads only: the superblock, key index and block table are
  /// fetched whole at open (planned by `supdb_open_plan`) and are resident
  /// after. That split costs ~nothing while key cardinality is bounded --
  /// a logshed segment is ~100 keys of index over megabytes of postings --
  /// and it is the premise that expires if keys become unbounded, e.g. a
  /// trigram or free-text index. Do not index free text over this source
  /// without revisiting it; the ranges stay absolute so that day changes
  /// this file, not the ABI.
  planRanges(keys) {
    const m = this.mod;
    const packed = keys.map((k) => (typeof k === "string" ? m.enc.encode(k) : k));
    let total = 4;
    for (const k of packed) total += 4 + k.length;
    const buf = new Uint8Array(total);
    const dv = new DataView(buf.buffer);
    dv.setUint32(0, packed.length, true);
    let at = 4;
    for (const k of packed) {
      dv.setUint32(at, k.length, true);
      at += 4;
      buf.set(k, at);
      at += k.length;
    }
    m.withKey(buf, (p, l) =>
      this.check(this.exports.supdb_ranges(this.handle, p, l), 0xffffffff),
    );
    return m.ranges();
  }

  /// Plan-then-fetch for a reader opened over a cache: after this resolves,
  /// `lookup`/`count` for these keys run synchronously with no miss. The
  /// only awaits in a cached reader's life are `openCached` and this. A
  /// no-op on whole-object sources, which hold everything already.
  async ensure(keys) {
    if (this.cache) await this.cache.ensure(this.planRanges(keys));
  }

  scanFrame(call, from) {
    const m = this.mod;
    m.withKey(from, (p, l) => this.check(call(p, l), 0xffffffff));
    const base = this.exports.supdb_out_ptr();
    const dv = m.view;
    const count = dv.getUint32(base, true);
    const out = new Array(count);
    let at = base + 4;
    for (let i = 0; i < count; i++) {
      const klen = dv.getUint32(at, true);
      const lo = dv.getUint32(at + 4, true);
      const hi = dv.getUint32(at + 8, true);
      at += 12;
      // Both halves set is the sentinel for "this key's values are not all
      // the width you asked about", which only `scanCountsFixed` produces.
      // `getUint32` already answers unsigned, so no `>>> 0` is needed here --
      // this is the one sentinel in the file that never had the i32 problem,
      // because it is read out of the frame rather than returned by a call.
      const missing = lo === 0xffffffff && hi === 0xffffffff;
      out[i] = {
        key: m.dec.decode(m.mem.subarray(at, at + klen)),
        count: missing ? null : hi * 0x100000000 + lo,
      };
      at += klen;
    }
    return out;
  }

  close() {
    if (this.handle !== NO_HANDLE) {
      this.exports.supdb_close(this.handle);
      this.handle = NO_HANDLE;
    }
  }
}

async function instantiate(wasm, host) {
  const imports = { env: host };
  const src =
    wasm instanceof Response
      ? await WebAssembly.instantiateStreaming(wasm, imports)
      : await WebAssembly.instantiate(
          wasm instanceof ArrayBuffer ? wasm : wasm.buffer,
          imports,
        );
  return src.instance;
}

/// Open over an object already in this realm's memory.
///
/// The bytes are copied into the module's linear memory, so the caller's
/// buffer is theirs again afterwards. Fine up to the download budget this
/// library is designed for; `openSyncHandle` is the one that does not hold a
/// second copy.
export async function openMemory(wasm, bytes) {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  // The host imports must exist even on this path -- the module declares
  // them unconditionally -- and must never be called on it.
  const unreachable = () => {
    throw new Error("supdb: this reader was opened over memory, not a host");
  };
  const instance = await instantiate(wasm, {
    supdb_host_len: unreachable,
    supdb_host_read: unreachable,
  });
  const mod = new Module(instance, "memory");
  const ptr = instance.exports.supdb_alloc(u8.length);
  new Uint8Array(instance.exports.memory.buffer).set(u8, ptr);
  // `>>> 0`, because the u32 handle arrives as a signed i32 and u32::MAX as
  // -1. This exact comparison was dead for as long as it compared raw, and a
  // reader over an object that failed to open answered [] for every key.
  const h = instance.exports.supdb_open_mem(ptr, u8.length) >>> 0;
  if (h === 0xffffffff) throw new Error(mod.lastError());
  return new SupdbReader(mod, h);
}

/// Open over an OPFS file, read synchronously. R2.2(a).
///
/// `handle` is a `FileSystemSyncAccessHandle`, which only exists inside a
/// Web Worker. This is the path the library is designed around: the object is
/// downloaded once, asynchronously, and every read after that is synchronous,
/// so the reader API needs no async shape and the index borrow survives.
export async function openSyncHandle(wasm, handle) {
  const size = handle.getSize();
  let mem = null;
  const host = {
    supdb_host_len: () => size,
    supdb_host_read: (off, ptr, len) => {
      try {
        // Read into a detached buffer first. `read` wants a view it can write
        // into, and a view of wasm memory is invalidated whenever the memory
        // grows -- which it does, during open, between this call being set up
        // and being made.
        const tmp = new Uint8Array(len);
        const got = handle.read(tmp, { at: off });
        if (got !== len) return 1;
        new Uint8Array(mem.buffer).set(tmp, ptr);
        return 0;
      } catch {
        return 1;
      }
    },
  };
  const instance = await instantiate(wasm, host);
  mem = instance.exports.memory;
  const mod = new Module(instance, "opfs");
  const h = instance.exports.supdb_open_host() >>> 0;
  if (h === 0xffffffff) throw new Error(mod.lastError());
  return new SupdbReader(mod, h);
}

/// Open over a `CachedBytes` (cache.mjs): the object stays in object
/// storage, and only the parts queries touch become resident. R6.
///
/// The open itself is planned, not faulted: fetch the superblock probe, ask
/// the module (`supdb_open_plan`) which ranges the open will read -- the key
/// index, the block table, and the redo log's emptiness word -- fetch those,
/// then open. No format knowledge lives in JavaScript; a hand-copied
/// superblock constant has drifted out from under this library once already,
/// and the plan call is how it does not happen again.
///
/// After open the index and block table are resident inside the module, so
/// planning is free and `countFixed`/`scanCountsFixed` answer with no fetch
/// at all. Point reads want `await reader.ensure([...keys])` first; the
/// cache throws `SupdbCacheMiss` rather than serve a byte it does not hold.
export async function openCached(wasm, cache) {
  let mem = null;
  const host = {
    supdb_host_len: () => cache.length,
    supdb_host_read: (off, ptr, len) => {
      try {
        // Via a detached buffer, same reason as openSyncHandle: the wasm
        // memory can grow between planning this call and making it.
        const tmp = new Uint8Array(len);
        cache.readInto(off, tmp);
        new Uint8Array(mem.buffer).set(tmp, ptr);
        return 0;
      } catch (e) {
        // Surfaced on the JS side too: the module reports "host refused",
        // and this names which byte was not resident.
        cache.lastReadError = e;
        return 1;
      }
    },
  };
  const instance = await instantiate(wasm, host);
  mem = instance.exports.memory;
  const mod = new Module(instance, "cached");
  const e = instance.exports;

  const probe = e.supdb_open_probe();
  await cache.ensure([[0, probe]]);
  const head = new Uint8Array(Math.min(probe, cache.length));
  cache.readInto(0, head);
  const framed = mod.withKey(head, (p, l) => e.supdb_open_plan(p, l, cache.length)) >>> 0;
  if (framed === 0xffffffff) throw new Error(mod.lastError());
  await cache.ensure(mod.ranges());

  const h = e.supdb_open_host() >>> 0;
  if (h === 0xffffffff) {
    const why = cache.lastReadError ? ` (${cache.lastReadError})` : "";
    throw new Error(mod.lastError() + why);
  }
  const reader = new SupdbReader(mod, h);
  reader.cache = cache;
  return reader;
}

/// Download an object into OPFS once and hand back a synchronous handle.
///
/// This is the asynchronous half, and the only asynchronous half. It is a
/// worker-only call: `createSyncAccessHandle` does not exist on the main
/// thread.
export async function fetchIntoOpfs(url, name) {
  const root = await navigator.storage.getDirectory();
  const file = await root.getFileHandle(name, { create: true });
  const res = await fetch(url);
  if (!res.ok) throw new Error(`supdb: ${url} answered ${res.status}`);
  const bytes = new Uint8Array(await res.arrayBuffer());
  const handle = await file.createSyncAccessHandle();
  handle.truncate(0);
  handle.write(bytes, { at: 0 });
  handle.flush();
  return handle;
}
