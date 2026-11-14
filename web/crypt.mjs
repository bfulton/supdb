// An encrypted object, decrypted where it is fetched.
//
// `src/crypt.rs` defines the envelope: a 48-byte header, then the plaintext
// in chunks, each encrypted on its own with a 16-byte tag after it. This file
// reads that envelope with WebCrypto's AES-GCM, as a wrapper around the
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
// What follows from decrypting at the fetch: the cache's pages in OPFS, and
// an object downloaded whole, are plaintext in the origin's private storage.
// The envelope protects the object in the bucket, at the CDN and on the
// wire; a client that holds the key holds the data.
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
const TAG = 16;
const VERSION = 1;
const MIN_CHUNK_SHIFT = 9;
const MAX_CHUNK_SHIFT = 24;
const MAGIC = [0x73, 0x75, 0x70, 0x64, 0x62, 0x41, 0x45, 0x00]; // "supdbAE\0"

function refused(msg) {
  const e = new Error(`supdb: ${msg}`);
  e.name = "SupdbEncrypted";
  return e;
}

/// The header the first 48 bytes of a stored object hold:
/// `{ chunkShift, chunk, plainLen, id, keyId, chunks, storedLen, bytes }`.
/// `bytes` is the header as stored, which every chunk's tag covers; `keyId`
/// is a BigInt, the caller's word for which key the object is under.
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
  const chunks = Math.max(1, Math.ceil(plainLen / chunk));
  if (chunks > 0x100000000) throw refused("an encrypted object with more chunks than its nonce counts");
  return {
    chunkShift,
    chunk,
    plainLen,
    id: bytes.slice(24, 40),
    keyId: dv.getBigUint64(40, true),
    chunks,
    storedLen: HEADER + plainLen + chunks * TAG,
    bytes,
  };
}

/// A raw AES key (16 or 32 bytes) as a WebCrypto key that can only decrypt
/// and cannot be read back out.
export function importKey(raw) {
  return crypto.subtle.importKey("raw", raw, { name: "AES-GCM" }, false, ["decrypt"]);
}

const storedOff = (h, i) => HEADER + i * (h.chunk + TAG);
const chunkPlainLen = (h, i) => Math.min(h.chunk, h.plainLen - i * h.chunk);

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
/// The header is fetched once, at the first call. A request is clamped to
/// the plaintext, as a ranged GET is to its object, and answers with the
/// plaintext's length as `total`.
export function decryptingFetcher(fetcher, key) {
  let opened = null;
  const open = async () => {
    const r = await fetcher(0, HEADER);
    const h = parseHeader(r.bytes);
    if (r.total !== h.storedLen) {
      throw refused(
        `an encrypted object of ${r.total} bytes whose header describes ${h.storedLen}: cut short, or grown`,
      );
    }
    return { h, k: typeof key === "function" ? await key(h) : key };
  };
  return async (off, len) => {
    // A failed open is not remembered: the next call asks again.
    opened ??= open().catch((e) => {
      opened = null;
      throw e;
    });
    const { h, k } = await opened;
    const end = Math.min(off + len, h.plainLen);
    // Nothing asked, or nothing there: the first chunk is still verified,
    // since `total` is the header's word and a tag is what makes it true.
    const [first, last] = end > off ? [Math.floor(off / h.chunk), Math.floor((end - 1) / h.chunk)] : [0, 0];
    const at = storedOff(h, first);
    const want = storedOff(h, last) + chunkPlainLen(h, last) + TAG - at;
    const r = await fetcher(at, want);
    // A short answer would fail its last chunk's tag anyway; said here, it
    // is the fetcher's fault by name and not the key's.
    if (r.bytes.length < want) {
      throw refused(`asked ${want} stored bytes at ${at} of ${h.storedLen}, got ${r.bytes.length}`);
    }
    const parts = await Promise.all(
      Array.from({ length: last - first + 1 }, (_, n) => {
        const i = first + n;
        const lo = storedOff(h, i) - at;
        return openChunk(h, k, i, r.bytes.subarray(lo, lo + chunkPlainLen(h, i) + TAG));
      }),
    );
    if (end <= off) return { bytes: new Uint8Array(0), total: h.plainLen };
    const out = new Uint8Array(end - off);
    let done = 0;
    for (let n = 0; n < parts.length; n++) {
      const base = (first + n) * h.chunk;
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
  const plain = decryptingFetcher(whole, key);
  const { total } = await plain(0, 0);
  return (await plain(0, total)).bytes;
}
