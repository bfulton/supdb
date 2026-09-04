// A caching byte source over object storage. R6.3.
//
// The whole-download path (`fetchIntoOpfs`) holds the object; this holds only
// the parts a query touches, under a byte budget, and fetches by HTTP range.
// It exists in the library rather than in every caller because "read a large
// immutable object over HTTP and keep some of it locally" is the part that
// gets written badly by everyone who writes it alone -- usually by silently
// zero-filling a miss, which for an index file is not an error but a wrong
// answer.
//
// The contract, which is the whole design (R6.2): reads never fetch. The
// caller awaits `ensure(ranges)` with the ranges the module planned --
// `supdb_open_plan` for the open, `supdb_ranges` for a query -- and the
// synchronous read path then finds every byte resident. A read outside what
// was ensured throws `SupdbCacheMiss`, loudly, because the alternative is
// the reader decoding zeroes as data.
//
// ## Pages: 64 KiB
//
// The unit of residency is a 64 KiB page, for two reasons that meet in the
// middle. From below: supdb seals blocks at 64 KiB and the planner's ranges
// are whole stored blocks, so one page usually holds one block read and a
// page boundary rarely splits one. From above: an S3 range GET carries a
// fixed per-request cost (round trip plus per-request price) that dominates
// the transfer below a megabyte or so, so what matters is not page size but
// request count -- and `ensure` coalesces runs of missing pages into one
// request each, so a large contiguous plan costs one GET regardless of page
// size. Smaller pages would multiply bookkeeping without changing what goes
// on the wire; larger ones would round a point lookup up to more over-fetch
// than the block it wants.
//
// ## Eviction: CLOCK
//
// One reference bit per page-slot, a rotating hand, evict the first slot the
// hand finds unreferenced. Chosen over exact LRU because the touch happens on
// the synchronous read path under `supdb_host_read` -- CLOCK's touch is one
// byte store into a flat array, where LRU maintains an ordered structure per
// read -- and because for this access pattern (a query touches a handful of
// pages, once) CLOCK's approximation of LRU costs nothing measurable. Pages
// named by the in-flight `ensure` are pinned so a plan cannot evict itself.
//
// ## Persistence
//
// Backed by OPFS so it survives reloads: a data file of `slots * pageSize`
// bytes -- the budget is a real file size, not an accounting fiction -- and a
// small JSON meta file mapping slots to page numbers. Reference bits are not
// persisted; they are an eviction hint, not state. Name the cache after the
// immutable object version (URL or ETag): the meta records the object length
// and refuses to resume over a mismatch. Only inside a Web Worker, like every
// sync access handle.

const PAGE = 1 << 16;
const META_VERSION = 1;

function miss(at, page) {
  const e = new Error(
    `supdb cache: byte ${at} (page ${page}) is not resident. ensure() the module's ` +
      `planned ranges before reading -- a silent zero-fill here would be corruption`,
  );
  e.name = "SupdbCacheMiss";
  return e;
}

/// Fetch one HTTP range and learn the object's total length from the answer.
///
/// The fetcher contract every source of ranges implements (this one and
/// `s3RangeFetcher`): `(offset, length) -> { bytes, total }`. `total` comes
/// from Content-Range, so no separate HEAD request is needed and the cache
/// can size itself from its very first fetch.
export function httpRangeFetcher(url, init = {}) {
  return async (off, len) => {
    const res = await fetch(url, {
      ...init,
      headers: { ...(init.headers ?? {}), range: `bytes=${off}-${off + len - 1}` },
    });
    if (res.status !== 206) {
      throw new Error(
        `supdb cache: ${url} answered ${res.status} to a range request` +
          (res.status === 200 ? " (a 200 is the whole object: the server ignores Range)" : ""),
      );
    }
    const m = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(res.headers.get("content-range") ?? "");
    if (!m || Number(m[1]) !== off) {
      throw new Error(`supdb cache: unusable content-range for ${off}+${len}`);
    }
    return { bytes: new Uint8Array(await res.arrayBuffer()), total: Number(m[3]) };
  };
}

export class CachedBytes {
  /// Open (or resume) a cache. `fetcher` is `(off, len) -> {bytes, total}`;
  /// `budgetBytes` is rounded down to whole pages, minimum one.
  static async open({ name, fetcher, budgetBytes, pageSize = PAGE }) {
    const c = new CachedBytes();
    c.ps = pageSize;
    c.slots = Math.max(1, Math.floor(budgetBytes / pageSize));
    c.budgetBytes = c.slots * pageSize;
    c.fetcher = fetcher;
    c.name = name;
    c.stats = { fetchedBytes: 0, fetchCalls: 0, pageFetches: 0, evicted: 0, reads: 0 };

    const root = await navigator.storage.getDirectory();
    const dataFile = await root.getFileHandle(`${name}.pages`, { create: true });
    const metaFile = await root.getFileHandle(`${name}.meta`, { create: true });
    c.data = await dataFile.createSyncAccessHandle();
    c.meta = await metaFile.createSyncAccessHandle();

    // slotPage[s] = page number resident in slot s, or -1. ref = CLOCK bits.
    c.slotPage = new Int32Array(c.slots).fill(-1);
    c.ref = new Uint8Array(c.slots);
    c.hand = 0;
    c.slotOf = new Map();

    if (!c.resume()) {
      // A fresh cache. The first fetch is page 0 -- the superblock lives
      // there and every open starts with it -- and its Content-Range answer
      // is where the object's length comes from.
      c.data.truncate(c.slots * c.ps);
      const r = await c.fetch(0, c.ps); // clamped inside once total is known
      c.length = r.total;
      c.install(0, r.bytes.subarray(0, Math.min(r.bytes.length, c.length)), new Set([0]));
      c.persist();
    }
    return c;
  }

  /// Try to pick up where a previous session left off. False if the meta is
  /// absent, damaged, or describes a different geometry or object.
  resume() {
    try {
      const size = this.meta.getSize();
      if (size === 0) return false;
      const buf = new Uint8Array(size);
      this.meta.read(buf, { at: 0 });
      const m = JSON.parse(new TextDecoder().decode(buf));
      if (
        m.v !== META_VERSION ||
        m.pageSize !== this.ps ||
        m.slots !== this.slots ||
        !Number.isInteger(m.length) ||
        !Array.isArray(m.slotPage) ||
        m.slotPage.length !== this.slots
      ) {
        return false;
      }
      this.length = m.length;
      for (let s = 0; s < this.slots; s++) {
        const p = m.slotPage[s];
        this.slotPage[s] = p;
        if (p >= 0) this.slotOf.set(p, s);
      }
      return true;
    } catch {
      return false;
    }
  }

  persist() {
    const doc = JSON.stringify({
      v: META_VERSION,
      pageSize: this.ps,
      slots: this.slots,
      length: this.length,
      slotPage: Array.from(this.slotPage),
    });
    const bytes = new TextEncoder().encode(doc);
    this.meta.truncate(0);
    this.meta.write(bytes, { at: 0 });
    this.meta.flush();
  }

  async fetch(off, len) {
    if (this.length !== undefined) len = Math.min(len, this.length - off);
    const r = await this.fetcher(off, len);
    this.stats.fetchCalls += 1;
    this.stats.fetchedBytes += r.bytes.length;
    return r;
  }

  get residentBytes() {
    let n = 0;
    for (const p of this.slotOf.keys()) {
      n += Math.min(this.ps, this.length - p * this.ps);
    }
    return n;
  }

  /// Make every byte of `ranges` (an array of `[offset, length]`) resident.
  /// Asynchronous, and the only place a fetch happens. Runs of missing pages
  /// coalesce into one request each.
  async ensure(ranges) {
    const needed = new Set();
    for (const [off, len] of ranges) {
      if (len === 0) continue;
      const last = Math.min(off + len - 1, this.length - 1);
      for (let p = Math.floor(off / this.ps); p <= Math.floor(last / this.ps); p++) {
        needed.add(p);
      }
    }
    if (needed.size > this.slots) {
      const e = new Error(
        `supdb cache: this plan needs ${needed.size} pages resident at once and the ` +
          `budget holds ${this.slots}. Raise the budget or split the query`,
      );
      e.name = "SupdbCacheBudget";
      throw e;
    }
    const missing = [...needed].filter((p) => !this.slotOf.has(p)).sort((a, b) => a - b);
    if (missing.length === 0) return;

    // Coalesce consecutive missing pages into one ranged request each.
    const runs = [];
    for (const p of missing) {
      const last = runs[runs.length - 1];
      if (last && p === last[0] + last[1]) last[1] += 1;
      else runs.push([p, 1]);
    }
    for (const [p0, n] of runs) {
      const off = p0 * this.ps;
      const want = Math.min(n * this.ps, this.length - off);
      const r = await this.fetch(off, want);
      if (r.bytes.length !== want) {
        throw new Error(`supdb cache: asked ${want} bytes at ${off}, got ${r.bytes.length}`);
      }
      for (let i = 0; i < n; i++) {
        this.install(p0 + i, r.bytes.subarray(i * this.ps, Math.min((i + 1) * this.ps, want)), needed);
      }
    }
    this.persist();
  }

  /// Put one page's bytes into a slot, evicting by CLOCK if none is free.
  /// Pages in `pinned` are never evicted -- they belong to the plan being
  /// installed, and a plan must not evict itself.
  install(page, bytes, pinned) {
    let s = -1;
    for (let step = 0; step < 2 * this.slots + 1; step++) {
      const cand = this.hand;
      this.hand = (this.hand + 1) % this.slots;
      const resident = this.slotPage[cand];
      if (resident === -1) {
        s = cand;
        break;
      }
      if (pinned.has(resident)) continue;
      if (this.ref[cand]) {
        this.ref[cand] = 0;
        continue;
      }
      this.slotOf.delete(resident);
      this.stats.evicted += 1;
      s = cand;
      break;
    }
    if (s === -1) {
      // Cannot happen while ensure() checks needed.size <= slots, and if it
      // ever does, failing is better than evicting a pinned page.
      const e = new Error("supdb cache: every slot is pinned");
      e.name = "SupdbCacheBudget";
      throw e;
    }
    // Always a full page on disk, zero-padded at the object's tail: reads
    // are clamped to the object length, so the padding is never served.
    const out = new Uint8Array(this.ps);
    out.set(bytes, 0);
    this.data.write(out, { at: s * this.ps });
    this.slotPage[s] = page;
    this.slotOf.set(page, s);
    this.ref[s] = 1;
  }

  /// Synchronous read into `out` (a Uint8Array sized to the read). Never
  /// fetches. Throws `SupdbCacheMiss` for any byte that is not resident.
  readInto(off, out) {
    const len = out.length;
    if (off + len > this.length) {
      throw miss(this.length, Math.floor(this.length / this.ps));
    }
    let done = 0;
    while (done < len) {
      const at = off + done;
      const page = Math.floor(at / this.ps);
      const slot = this.slotOf.get(page);
      if (slot === undefined) throw miss(at, page);
      this.ref[slot] = 1;
      const inPage = at - page * this.ps;
      const n = Math.min(len - done, this.ps - inPage);
      const got = this.data.read(out.subarray(done, done + n), { at: slot * this.ps + inPage });
      if (got !== n) {
        const e = new Error(`supdb cache: slot read returned ${got} of ${n} bytes`);
        e.name = "SupdbCacheMiss";
        throw e;
      }
      done += n;
    }
    this.stats.reads += 1;
  }

  close() {
    try {
      this.persist();
    } catch {
      /* a cache that cannot persist is still a cache */
    }
    this.data.close();
    this.meta.close();
  }
}
