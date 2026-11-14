// `crypt.mjs` against the envelope on record.
//
//   node web/crypt.test.mjs
//
// `tests/data/crypt-v1.bin` is an envelope the Rust writer made, and
// `tests/crypt.rs` requires the writer to make exactly those bytes still.
// This requires the browser's reader to read them: the layout is written
// down twice, once in each language, and the one file is what both are held
// to. It runs under Node because the reader is WebCrypto and plain modules,
// and Node has both; `scripts/check.sh wasm` runs it.
//
// It ends in one line, `crypt.mjs: N checks passed`, and exits non-zero on
// the first check that fails.

import { readFileSync } from "node:fs";
import { decryptingFetcher, decryptObject, importKey, parseHeader } from "./crypt.mjs";

let checks = 0;
function check(ok, what) {
  if (!ok) {
    console.error(`crypt.mjs: FAILED: ${what}`);
    process.exit(1);
  }
  checks += 1;
}
const same = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
async function rejects(promise, what, saying = /./) {
  let threw = null;
  try {
    await promise;
  } catch (e) {
    threw = e;
  }
  check(
    threw !== null && threw.name === "SupdbEncrypted" && saying.test(threw.message),
    `${what}: ${threw ?? "no error"}`,
  );
}
function throws(f, what, saying) {
  return rejects(new Promise((resolve) => resolve(f())), what, saying);
}

// What the record holds, as `tests/crypt.rs` has it.
const stored = new Uint8Array(readFileSync(new URL("../tests/data/crypt-v1.bin", import.meta.url)));
const plain = Uint8Array.from({ length: 1200 }, (_, i) => (i * 7 + Math.floor(i / 251)) & 0xff);
const rawKey = Uint8Array.from({ length: 32 }, (_, i) => i);
const key = await importKey(rawKey);
const [C, T, H] = [512, 16, 48];

// A fetcher over bytes in memory that remembers what it was asked for.
function memory(bytes) {
  const log = [];
  const fetcher = async (off, len) => {
    log.push([off, len]);
    return { bytes: bytes.subarray(off, Math.min(off + len, bytes.length)), total: bytes.length };
  };
  return { fetcher, log };
}

// The header.
const h = parseHeader(stored);
check(h.chunkShift === 9 && h.chunk === C && h.plainLen === 1200 && h.chunks === 3, "header geometry");
check(h.storedLen === stored.length && stored.length === H + 1200 + 3 * T, "stored length");
check(h.keyId === 0x0102030405060708n, "key id");
check(same(h.id, Uint8Array.from({ length: 16 }, (_, i) => 0xa0 + i)), "object id");

// A header this reader does not read is refused by name, before any tag is
// asked: each field on its own, in a header otherwise the record's.
{
  const with_ = (at, v) => {
    const b = stored.slice(0, H);
    new DataView(b.buffer).setUint32(at, v, true);
    return b;
  };
  await throws(() => parseHeader(with_(0, 0x64707574)), "another magic", /magic/);
  await throws(() => parseHeader(with_(8, 2)), "a later version", /version 2/);
  await throws(() => parseHeader(with_(12, 8)), "a chunk too small", /chunk shift is 8/);
  await throws(() => parseHeader(with_(12, 25)), "a chunk too large", /chunk shift is 25/);
  await throws(() => parseHeader(with_(20, 1 << 21)), "a length past a number", /too long/);
  await throws(() => parseHeader(stored.subarray(0, H - 1)), "a byte short of a header", /too short/);
  check(parseHeader(with_(12, 24)).chunk === 1 << 24, "the largest chunk parses");
}

// The whole object.
check(same(await decryptObject(stored, key), plain), "the record decrypts to its plaintext");
check(same(await decryptObject(stored.buffer, key), plain), "from an ArrayBuffer too");

// Ranges: the plaintext asked for, and the stored bytes fetched for it.
{
  const { fetcher, log } = memory(stored);
  const f = decryptingFetcher(fetcher, key);
  const cases = [
    // plaintext off, len, then the stored range the whole chunks lie in
    [0, 512, H, C + T],
    [511, 2, H, 2 * (C + T)],
    [600, 10, H + (C + T), C + T],
    [1024, 176, H + 2 * (C + T), 176 + T],
    [0, 1200, H, 1200 + 3 * T],
    [1199, 1, H + 2 * (C + T), 176 + T],
  ];
  for (const [off, len, at, want] of cases) {
    const r = await f(off, len);
    check(same(r.bytes, plain.subarray(off, off + len)), `plaintext ${off}+${len}`);
    check(r.total === 1200, `total is the plaintext's length at ${off}+${len}`);
    const asked = log.at(-1);
    check(asked[0] === at && asked[1] === want, `stored range for ${off}+${len}: ${asked}`);
  }
  // The header is fetched once, at the first call, and each call after is
  // one request.
  check(log.length === cases.length + 1, `one request a call after the header: ${log.length}`);
  check(log.filter(([o, l]) => o === 0 && l === H).length === 1 && log[0][1] === H, "the header once, first");

  // A request past the end is clamped, as a ranged GET is: the cache's
  // first fetch is a whole page of an object it has not sized yet.
  const over = await f(1000, 65536);
  check(same(over.bytes, plain.subarray(1000)) && over.total === 1200, "clamped to the plaintext");
  const none = await f(1200, 10);
  check(none.bytes.length === 0 && none.total === 1200, "nothing past the end");
}

// A key chosen from the header.
{
  let seen = null;
  const f = decryptingFetcher(memory(stored).fetcher, async (hdr) => {
    seen = hdr;
    return key;
  });
  check(same((await f(3, 5)).bytes, plain.subarray(3, 8)), "a key from a function");
  check(seen.keyId === 0x0102030405060708n, "the function sees the header");
}

// Every byte of the stored object, changed, is refused: by the header's
// checks or by a tag, and never read as something else.
for (let at = 0; at < stored.length; at++) {
  const bad = stored.slice();
  bad[at] ^= 0x01;
  await rejects(decryptObject(bad, key), `byte ${at} changed and read`);
}

// A changed byte in one chunk fails a read of that chunk and of no other.
{
  const bad = stored.slice();
  bad[H + (C + T) + 5] ^= 0x80;
  const f = decryptingFetcher(memory(bad).fetcher, key);
  check(same((await f(0, 512)).bytes, plain.subarray(0, 512)), "the chunk before is untouched");
  await rejects(f(600, 4), "the changed chunk");
  await rejects(f(0, 1200), "a range over the changed chunk");
  check(same((await f(1024, 100)).bytes, plain.subarray(1024, 1124)), "the chunk after is untouched");
}

// Two chunks in each other's place.
{
  const bad = stored.slice();
  bad.set(stored.subarray(H + (C + T), H + 2 * (C + T)), H);
  bad.set(stored.subarray(H, H + (C + T)), H + (C + T));
  const f = decryptingFetcher(memory(bad).fetcher, key);
  await rejects(f(0, 10), "a chunk in the first's place");
  await rejects(f(600, 10), "a chunk in the second's place");
}

// Cut short, grown, the wrong key, not an envelope.
await rejects(decryptObject(stored.subarray(0, stored.length - 1), key), "a byte short");
await rejects(decryptObject(stored.subarray(0, H + C + T), key), "two chunks short");
await rejects(decryptObject(stored.subarray(0, 40), key), "less than a header");
await rejects(decryptObject(Uint8Array.from([...stored, 0]), key), "a byte long");
{
  const other = rawKey.slice();
  other[31] ^= 1;
  await rejects(decryptObject(stored, await importKey(other)), "the wrong key");
  await rejects(decryptObject(new Uint8Array(4096), key), "zeroes");
  // A chunk short with the header's length rewritten to agree.
  const cut = stored.slice(0, H + 2 * (C + T));
  new DataView(cut.buffer).setUint32(16, 1024, true);
  check(parseHeader(cut).storedLen === cut.length, "the rewritten header agrees with the length");
  await rejects(decryptObject(cut, key), "a chunk short under a rewritten header");
}

// A fetcher that answers short, or about another object, is refused.
{
  const { fetcher } = memory(stored);
  const short = async (off, len) => {
    const r = await fetcher(off, len);
    return off === 0 ? r : { bytes: r.bytes.subarray(0, r.bytes.length - 1), total: r.total };
  };
  await rejects(decryptingFetcher(short, key)(0, 10), "a short answer", /asked 528 stored bytes at 48/);
  const lying = async (off, len) => ({ ...(await fetcher(off, len)), total: stored.length + 528 });
  await rejects(decryptingFetcher(lying, key)(0, 10), "a total that is not the header's");
}

// An object of no bytes is a header and one tag, and the tag is asked for:
// a header alone cannot say "empty". Made here, since the record is not one.
{
  const writing = await crypto.subtle.importKey("raw", rawKey, { name: "AES-GCM" }, false, ["encrypt"]);
  const header = stored.slice(0, H);
  new DataView(header.buffer).setUint32(16, 0, true); // plaintext length: none
  const iv = new Uint8Array(12);
  iv.set(header.subarray(24, 32));
  const tag = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: header }, writing, new Uint8Array(0)),
  );
  const empty = Uint8Array.from([...header, ...tag]);
  check(tag.length === T && parseHeader(empty).storedLen === empty.length, "an empty envelope's shape");
  check((await decryptObject(empty, key)).length === 0, "an empty envelope reads as no bytes");
  const { fetcher, log } = memory(empty);
  const none = await decryptingFetcher(fetcher, key)(0, 4096);
  check(none.bytes.length === 0 && none.total === 0, "and fetches as none");
  check(log.at(-1)[0] === H && log.at(-1)[1] === T, "its one tag is fetched");
  const forged = empty.slice();
  forged[H + 3] ^= 1;
  await rejects(decryptObject(forged, key), "an empty envelope nobody with the key made", /failed its check/);
  await rejects(decryptingFetcher(memory(forged).fetcher, key)(0, 0), "asked for nothing", /failed its check/);
}

// A failed open is asked again, not remembered.
{
  let fail = true;
  const { fetcher } = memory(stored);
  const flaky = async (off, len) => {
    if (fail) throw new Error("network");
    return fetcher(off, len);
  };
  const f = decryptingFetcher(flaky, key);
  let threw = false;
  await f(0, 10).catch(() => (threw = true));
  fail = false;
  check(threw && same((await f(0, 10)).bytes, plain.subarray(0, 10)), "the open is retried");
}

console.log(`crypt.mjs: ${checks} checks passed`);
