// The browser library: a supdb reader over a byte source, in JavaScript.
//
// There is no binding generator here. The wasm module exports eleven C
// functions that pass integers and byte ranges, and this file is the whole
// glue for them -- about two hundred lines against the descriptor sections,
// shims and reflection a generator would add to something the size budget in
// R3.3 is explicitly about. `web/build.sh` measures what ships.
//
// Two byte sources, one API:
//
//   openMemory(bytes)   the object is already an ArrayBuffer in this realm
//   openSyncHandle(h)   the object is a file in OPFS and `h` is a
//                       FileSystemSyncAccessHandle over it
//
// The second is the one logshed uses and the reason nothing here is async
// past `load`. `FileSystemSyncAccessHandle.read(buf, {at})` is a synchronous
// random read, so the Rust side never has to await, and `flatindex` can go on
// handing back borrows into the index. It is only available inside a Web
// Worker, which is where `worker.mjs` runs it.

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

  check(v, sentinel) {
    if (v === sentinel) throw new Error(this.mod.lastError());
    return v;
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

  /// R4.3 -- how many values, without decoding any of them.
  ///
  /// O(values) but not O(bytes): it walks the length prefixes and skips the
  /// payload, and -- the part that matters here -- nothing crosses the wasm
  /// boundary per value. See `countFixed` for the O(extents) form.
  count(key) {
    const v = this.mod.withKey(key, (p, l) =>
      this.exports.supdb_count(this.handle, p, l),
    );
    if (v === 0xffffffffffffffffn) throw new Error(this.mod.lastError());
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
    if (v === 0xffffffffffffffffn) return null;
    return Number(v);
  }

  /// Stored bytes under a key: payload plus one length prefix per value.
  /// O(extents). This is the input to "is the index cheaper than a scan".
  storedBytes(key) {
    const v = this.mod.withKey(key, (p, l) =>
      this.exports.supdb_stored_bytes(this.handle, p, l),
    );
    if (v === 0xffffffffffffffffn) throw new Error(this.mod.lastError());
    return Number(v);
  }

  /// R4.4 -- the dictionary in key order from `from`, with each key's count.
  /// What a "top paths" or "countries" panel is made of.
  scanCounts(from, limit) {
    const m = this.mod;
    m.withKey(from, (p, l) =>
      this.check(
        this.exports.supdb_scan_counts(this.handle, p, l, limit),
        0xffffffff,
      ),
    );
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
      out[i] = {
        key: m.dec.decode(m.mem.subarray(at, at + klen)),
        count: hi * 0x100000000 + lo,
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
  const h = instance.exports.supdb_open_mem(ptr, u8.length);
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
  const h = instance.exports.supdb_open_host();
  if (h === 0xffffffff) throw new Error(mod.lastError());
  return new SupdbReader(mod, h);
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
