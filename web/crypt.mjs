// An encrypted object, decrypted where it is fetched.
//
// `src/crypt.rs` defines the envelope: a 48-byte header and zeroes to 4096,
// then the plaintext in chunks, each encrypted on its own with a 16-byte tag
// after it, a chunk and its tag together a power of two of stored bytes. This
// file reads that envelope with WebCrypto's AES-GCM, as a wrapper around the
// fetcher every byte source here already takes:
//
//   const key = await importKey(rawKeyBytes);
//   const cache = await CachedBytes.open({
//     name,
//     fetcher: decryptingFetcher(httpRangeFetcher(url), key),
//     budgetBytes,
//   });
//
// The wrapper speaks plaintext offsets to its caller and stored offsets to
// the fetcher under it, so the cache, the module and every plan the module
// makes are unchanged: a plan's ranges are plaintext ranges, and the wrapper
// fetches the whole chunks they lie in. Nothing in the wasm module knows the
// object was encrypted, and the module carries no cipher.
//
// It costs no round trip. The header rides in the first request, which asks
// for the object from its first byte through what the caller wanted (sized
// for the chunk the writer makes by default), and every request after is one
// request, as it was. A chunk is decrypted once,
// where it is fetched, and what the cache holds after is plaintext it reads
// at its own speed.
//
// What follows from that: the cache's pages in OPFS, and an object
// downloaded whole, are plaintext in the origin's private storage. The
// envelope protects the object in the bucket, at the CDN and on the wire; a
// client that holds the key holds the data.
//
// Every byte handed up was verified first. A chunk whose tag does not match
// rejects the fetch, so an `ensure` over a changed object throws and the
// read after it never runs: no changed byte reaches the cache.
//
// ## The layout, in two places
//
// The header's layout and the nonce are written down here and in
// `src/crypt.rs`. One stored envelope, `tests/data/crypt-v1.bin`, holds both
// to one answer: the Rust tests require `encrypt_with_id` to write exactly
// those bytes, and `web/crypt.test.mjs` requires this file to read them.

const HEADER = 48;
const DATA = 4096;
const TAG = 16;
const VERSION = 1;
const MIN_CHUNK_SHIFT = 9;
const MAX_CHUNK_SHIFT = 24;
// The chunk the writer makes unless told otherwise. Only the size of the
// first request is drawn from it: an object of larger chunks reads the same
// and costs that request's shortfall as one request more.
const USUAL_CHUNK = 4096;
const MAGIC = [0x73, 0x75, 0x70, 0x64, 0x62, 0x41, 0x45, 0x00]; // "supdbAE\0"

function refused(msg) {
  const e = new Error(`supdb: ${msg}`);
  e.name = "SupdbEncrypted";
  return e;
}

/// The header the first 48 bytes of a stored object hold:
/// `{ chunkShift, chunk, per, plainLen, id, keyId, chunks, storedLen, bytes }`.
/// `chunk` is a chunk's stored bytes and `per` the plaintext in one, the
/// chunk less its tag. `bytes` is the header as stored, which every chunk's
/// tag covers; `keyId` is a BigInt, the caller's word for which key the
/// object is under.
export function parseHeader(u8) {
  if (u8.length < HEADER) throw refused("too short to be an encrypted object");
  const bytes = u8.slice(0, HEADER);
  if (MAGIC.some((b, i) => bytes[i] !== b)) {
    throw refused("not an encrypted object: the magic does not match");
  }
  const dv = new DataView(bytes.buffer);
  const version = dv.getUint32(8, true);
  if (version !== VERSION) {
    throw refused(`an encrypted object of version ${version}; this reader reads version ${VERSION}`);
  }
  const chunkShift = dv.getUint32(12, true);
  if (chunkShift < MIN_CHUNK_SHIFT || chunkShift > MAX_CHUNK_SHIFT) {
    throw refused(`an encrypted object's chunk shift is ${chunkShift}`);
  }
  // A length as a number: exact below 2^53, and the reader this feeds
  // refuses an object of 4 GiB or more on its own.
  const lenHi = dv.getUint32(20, true);
  if (lenHi >= 1 << 21) throw refused("an encrypted object too long for this reader");
  const plainLen = lenHi * 0x100000000 + dv.getUint32(16, true);
  const chunk = 2 ** chunkShift;
  const per = chunk - TAG;
  const chunks = Math.max(1, Math.ceil(plainLen / per));
  if (chunks > 0x100000000) throw refused("an encrypted object with more chunks than its nonce counts");
  return {
    chunkShift,
    chunk,
    per,
    plainLen,
    id: bytes.slice(24, 40),
    keyId: dv.getBigUint64(40, true),
    chunks,
    storedLen: DATA + plainLen + chunks * TAG,
    bytes,
  };
}

/// A raw AES key (16 or 32 bytes) as a WebCrypto key that can only decrypt
/// and cannot be read back out.
export function importKey(raw) {
  return crypto.subtle.importKey("raw", raw, { name: "AES-GCM" }, false, ["decrypt"]);
}

const storedOff = (h, i) => DATA + i * h.chunk;
const chunkPlainLen = (h, i) => Math.min(h.per, h.plainLen - i * h.per);

/// Verify and decrypt chunk `i` from its stored bytes, ciphertext then tag.
async function openChunk(h, key, i, stored) {
  const iv = new Uint8Array(12);
  iv.set(h.id.subarray(0, 8));
  new DataView(iv.buffer).setUint32(8, i, true);
  try {
    return new Uint8Array(
      await crypto.subtle.decrypt(
        { name: "AES-GCM", iv, additionalData: h.bytes, tagLength: 8 * TAG },
        key,
        stored,
      ),
    );
  } catch {
    throw refused(
      `chunk ${i} of an encrypted object failed its check: the key is wrong, or the object was changed`,
    );
  }
}

/// A fetcher of an object's plaintext, over a fetcher of its stored bytes.
///
/// Both are `(offset, length) -> { bytes, total }`. `key` is a CryptoKey for
/// AES-GCM, or a function from the parsed header to one (or to a promise of
/// one), for a caller that keeps several keys or derives one per object from
/// the header's `id`.
///
/// The first call's request starts at the object's first byte and runs
/// through what that call wants, so the header costs no request of its own
/// when the first call is at the plaintext's start, as an open's is; the
/// fetcher under it must clamp a request past the object's end, as a ranged
/// GET does and as the cache's own first fetch already relies on. A request
/// to this fetcher is clamped to the plaintext the same way, and answers
/// with the plaintext's length as `total`.
export function decryptingFetcher(fetcher, key) {
  let opened = null;
  const open = async (len) => {
    // The header's page and the whole chunks the first `len` plaintext
    // bytes lie in, at the usual chunk; with nothing wanted at the start,
    // the page and a tag, which is the least an object can be.
    const ask = DATA + (len > 0 ? Math.ceil(len / (USUAL_CHUNK - TAG)) * USUAL_CHUNK : TAG);
    const r = await fetcher(0, ask);
    const h = parseHeader(r.bytes);
    if (r.total !== h.storedLen) {
      throw refused(
        `an encrypted object of ${r.total} bytes whose header describes ${h.storedLen}: cut short, or grown`,
      );
    }
    // No tag covers the zeroes after the header, so they are looked at.
    if (r.bytes.length < DATA || r.bytes.subarray(HEADER, DATA).some((b) => b !== 0)) {
      throw refused("an encrypted object with something other than zeroes after its header");
    }
    return { h, k: typeof key === "function" ? await key(h) : key, head: r.bytes };
  };
  return async (off, len) => {
    // A failed open is not remembered: the next call asks again.
    opened ??= open(off === 0 ? len : 0).catch((e) => {
      opened = null;
      throw e;
    });
    const o = await opened;
    const { h, k } = o;
    const end = Math.min(off + len, h.plainLen);
    // Nothing asked, or nothing there: the first chunk is still verified,
    // since `total` is the header's word and a tag is what makes it true.
    const [first, last] = end > off ? [Math.floor(off / h.per), Math.floor((end - 1) / h.per)] : [0, 0];
    const at = storedOff(h, first);
    const want = storedOff(h, last) + chunkPlainLen(h, last) + TAG - at;
    let stored;
    if (o.head && at + want <= o.head.length) {
      stored = o.head.subarray(at, at + want);
    } else {
      const r = await fetcher(at, want);
      // A short answer would fail its last chunk's tag anyway; said here,
      // it is the fetcher's fault by name and not the key's.
      if (r.bytes.length < want) {
        throw refused(`asked ${want} stored bytes at ${at} of ${h.storedLen}, got ${r.bytes.length}`);
      }
      stored = r.bytes;
    }
    // The open's answer served the call that made it, and is let go.
    o.head = null;
    const parts = await Promise.all(
      Array.from({ length: last - first + 1 }, (_, n) => {
        const i = first + n;
        const lo = storedOff(h, i) - at;
        return openChunk(h, k, i, stored.subarray(lo, lo + chunkPlainLen(h, i) + TAG));
      }),
    );
    if (end <= off) return { bytes: new Uint8Array(0), total: h.plainLen };
    const out = new Uint8Array(end - off);
    let done = 0;
    for (let n = 0; n < parts.length; n++) {
      const base = (first + n) * h.per;
      const lo = Math.max(off, base) - base;
      const hi = Math.min(end, base + parts[n].length) - base;
      out.set(parts[n].subarray(lo, hi), done);
      done += hi - lo;
    }
    return { bytes: out, total: h.plainLen };
  };
}

/// A whole stored object, already in memory, as its plaintext: for
/// `openMemory`, and for an object downloaded whole into OPFS.
export async function decryptObject(stored, key) {
  const u8 = stored instanceof Uint8Array ? stored : new Uint8Array(stored);
  const whole = async (off, len) => ({
    bytes: u8.subarray(off, Math.min(off + len, u8.length)),
    total: u8.length,
  });
  // Asked for all of it: the header says how much that is.
  return (await decryptingFetcher(whole, key)(0, u8.length)).bytes;
}
