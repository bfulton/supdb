# Hostile input: what every reader does with bytes an attacker made

Run at `98e4e8f4` (C0 = `b3a043dc` plus the cost count's fix, phase 1's first removals and `release/`; `src/` here is the tree the coordinator's branch carries), 2026-11-28 between 09:10 and 10:40 UTC, on a machine of its own. Read-only to the engine. Nothing under `src/` or `tests/` was changed.

Every reader was driven with bytes made to break it:
- the segment readers (`Blob` lent from memory, `Blob` over a real file mapping, `Blob` over a copying source, `SparseBlob`);
- `Db::open` over a store directory with one of its files changed (segments, both manifest slots, the WAL, the run temp file, ordered indexes), as a plain store, a store in one file (`store.one`) and a store under a cipher (envelopes, sealed slots, the sealed log, `supdb.cipher`);
- the `crypt` envelope through `crypt::decrypt` and `Encrypted`, with the segment readers on top;
- the browser reader (`web/supdb.mjs` over the wasm module) through `openMemory`, `openCached` and `openSparse`.

A reader passes a case when it returns an error, or answers what was written. It fails when it panics, hangs, allocates more than 256 MiB in one request, traps (wasm), or answers, without an error, a value that was never written (a *misread*). Store cases are held to the key's whole history: an answer that equals any earlier committed state counts as an answer, not a misread. Those are counted separately as "opened at an earlier state".

## Findings

| id | sev | finding | where | proposed fix |
|---|---|---|---|---|
| HI-1 | **high** | **Zeroing one 8-byte word turns off the key section's checksum, and the reader then answers from damaged bytes.** `checksum_words` takes `crc_off == 0` to mean "this section has no row". A section that has a row and has its `crc_off` word zeroed is read unverified, so any other damage in the section is answered as fact. With the word zeroed and one bit flipped past the header, 385 of 600 cases open and answer wrong (keys absent, wrong counts), in all six segment shapes, natively and in the browser. Raw damage finds it unaided: all 26 silent misreads in 20,000 random raw mutations were zero runs over byte +160 of the key section, and all 26 vanish when that word is restored. The zeroed word alone misreads nothing (6 of 6 cases). The rule "a row named with an impossible shift is damage, not absence" covers the shift and not this word. | `src/flatindex.rs:366-370` (`checksum_words`); `Blob::open_with` at `src/blob.rs:1110` (`if idx.crc_off != 0`); `SparseBlob::open_with` at `src/blob.rs:3400` | Take a row as absent only where the section's stored length (`key_stored`, under the superblock's FNV) is exactly the section with no row. Otherwise `crc_off == 0` is damage and refuses. Add a test that zeroes the word and flips a byte behind it. |
| HI-2 | **high** | **Lengths in the file size an allocation before anything checks them against the object.** `bytes::take` resizes its scratch `Vec` to `len` and only then calls `read_at`, which refuses. `len` is a block's stored length from the block table, which no checksum covers. One flipped byte asks for up to 4 GiB per read. In the same way, `Blob::with_extent` sizes its decode buffer from the block table's uncompressed length, and `Sec::read` runs `vec![0u8; len]` with the superblock's `key_stored`/`blk_stored` before any range check; `pick_super` checks `high_water` against the object but neither section's end. Over a copying source, a forged superblock then panics on "capacity overflow" (len > isize::MAX), in both profiles. Below that it is one allocation of the forged size, which aborts the process under any memory bound smaller than the request. Seen: 9 of 20,000 raw cases and 9 of 20,000 forged cases asked for 0.5 to 4.0 GiB in one allocation; 3 forged superblocks panicked. The browser reader did not trap on these cases. | `src/bytes.rs:284-298` (`take`); `src/blob.rs:2168-2170` (`dec.resize(un, 0)`); `src/blob.rs:481-496` (`Sec::read`); `src/blob.rs:308-339` (`pick_super`) | In `take` and `Sec::read`, check `off.checked_add(len) <= src.len()` before sizing anything. In `pick_super`, refuse a section whose `checked_add` end passes `high_water` or the object. At open, hold each block-table entry's stored length to the file and its uncompressed length to the block size, or put the table under the key section's row. |
| HI-3 | **high** | **`SparseBlob::open` panics on a forged superblock offset.** The superblock is guarded by an FNV, which anyone can recompute. With field 3 (`key_off`) at `0xfffffffffffff000` or `u64::MAX`, in every shape: a checked build panics on an add overflow (24 cases). In release the add wraps, and the open panics on an out-of-range index into its verified-piece bitmap (10 cases). Random forgeries of the superblock page reach the same line (1 in 17,000). | `src/blob.rs:3397` (`sb.key_off + foff as u64`); `src/blob.rs:3509` (`s.verified[p / 64]` in `verify_span`) | `checked_add` every offset sum in the sparse open, and refuse a section that does not lie inside the object (the `pick_super` check in HI-2 covers it). |
| HI-4 | **high** | **The browser reader traps on a key-section header word that the 64-bit reader refuses.** `parse_head` computes `recs_cap = rd_u64(104).max(recs_len) as usize`. On wasm32, `0x1_0000_0000` or `1 << 62` narrows to 0, and the next line's `clamp(recs_len, recs_cap)` has min > max and panics. Under `panic = "abort"` that is `unreachable`, and the instance is dead. This happens in `openMemory` and `openCached`, in all six shapes (24 traps), with the row recomputed. The native reader refuses the same bytes. `format.md` F13 named this narrowing by reading, and this is its reproducer. | `src/flatindex.rs:2181-2182` (`parse_head`); every `rd_u64(..) as usize` in `parse_head`, `Header::parse`, `Super::decode` | Use `usize::try_from(..).ok()?` for each u64 word on every path the wasm build compiles. Never `clamp` file values: use `min`/`max`, which cannot panic. Run `wasm/drive.mjs` in CI over a dumped set. |
| HI-5 | **high** | **A key over 65,535 bytes is stored as a different key, silently, in release** (`format.md` F1, now run). `db.put` of a 65,536-byte key, then a commit, stores it under the **empty key** (`65536 as u16 == 0`): a read of the real key answers nothing, and a scan shows a 0-byte key, so the write lands on another key's values. A 70,000-byte key is stored as its first 4,464 bytes. A checked build panics (`a key fits a record's length word`) instead. 65,535 bytes round-trips through the memtable, a flush and a reopen. | `src/db.rs:19773` (the debug assertion); the `key.len() as u16` sites `format.md` F1 lists | `format.md` F1's: refuse a key over 65,535 bytes at every write entry point, and test 65,536 at put, read, flush and reopen. |
| HM-1 | medium | **A changed byte in the middle of the newest WAL reads as a torn tail.** The open drops every acknowledged, synced commit after it, with no error, in all three kinds of store. A single-bit flip at any of bytes 8..511 of the newest log opens at an earlier state in 504 of 512 cases (the other 8 refuse), in the plain store, under a cipher and in one file alike. Over the whole file, 848 of 3,000 plain-WAL mutations (900 of 3,000 under a cipher) open at an earlier state. The commits after the damage are still in the file, intact, with consecutive sequence numbers. The walk does not look past the first frame that fails. `crypto.md` reported this under a cipher, by argument, for the log header; it holds for every frame, and for the plain store. | `src/db.rs:10272-10295` (`Wal::replay_frames`: a frame that does not verify `break`s the walk) | Where a frame fails, scan on for a later frame that verifies with a sequence above the last applied one. If one is found, refuse as damage (`Db::lost`), not as a tear. A torn tail has nothing valid after it; damage does. |
| HM-2 | medium | **A valid segment from the public writer is refused on read: a key with no values, written with `set_inline_max(0)`.** All three `Blob` readers answer "a partial block read needs per-chunk checksums" for each such key, on the file exactly as `SegmentWriter` wrote it, in a process of its own: 3 of 302 keys, which are the shape's three keys with no values. The other five shapes, which inline small runs, read every key. This is not hostile input. It is a valid file that a reader refuses, found by the harness's pristine check. | `src/blob.rs:2063-2071` (`verify`'s `whole` arm, reached from `with_extent` for an empty block extent) | Read a zero-length extent as an empty run without touching its block, or have the writer inline an empty run whatever `inline_max` says. Add the shape (no values, `inline_max = 0`) to `tests/segwriter.rs`. |
| HL-3 | low | **A segment lent from an address that is not 8-aligned is refused.** `MappedBlocks::parse`/`get` and `parse_record_tail` check that the bytes they are about to view as `BlockRec` or `Ext` are aligned, and refuse where they are not. That keeps the `unsafe` casts sound, but it makes a valid file's readability depend on where the caller's buffer starts. Every shape, lent through `SliceBytes(&buf[k..])` for k = 1..7, is refused at open ("the block table is not readable"): 42 of 42 cases. Miri found it first: three `flatindex` unit tests fail under Miri because Miri does not give a `Vec<u8>` the 16-byte alignment a native `malloc` does (`unsafe.md`). Mapped and copied sections are aligned in practice, which is why nothing native had seen this. | `src/flatindex.rs:731`, `3824-3826`, `3877-3879` | Read `BlockRec` with `ptr::read_unaligned` (it is `Copy`, all integers). For `&[Ext]`, borrow where aligned and copy into a small buffer where not, rather than refusing. |
| HL-4 | low | **`format.md` F5, now run: a segment written with checksums off becomes unreadable once a writer with checksums on runs in the same process.** Shape 4 (checksums off) reads every key when it is the only segment its process wrote. Written beside segments with checksums on, 178 of its 302 keys answer "block checksum mismatch" in all three readers. | `block::CHECKSUMS` (`format.md` F5) | `format.md` F5's: record the setting in the file, and pass it per writer. |
| HL-1 | low | **Under a cipher, a damaged derived file refuses the whole store.** A changed byte in an ordered index's envelope (`x-<token>.o`) fails its tag, and the open answers "a file of the store under its cipher did not read, and nothing read from the store since is to be trusted". The same damage in a plain store's `ord-*.oidx` is rebuilt (`Db::ord_rebuilt`). This is availability, not safety: nothing is misread. | `src/db/pager.rs` (`Back::open_chunk`, the poison); `src/db/vault.rs` | Treat a failed tag on a `.o` or `.z` envelope as the plain store treats a bad ordered index: rebuild it from the segment. `pager.rs`'s `regenerable` already does this for form files. |
| HL-2 | low | **`Db::crash` keeps about 10 mappings and 0.5 MB of the process for good, each call.** 500 open-and-crash cycles grow the process from 73 mappings and 20 MB resident to 5,062 mappings and 329 MB. After about 7,000 the open fails with `block: out of memory (mmap)` at `src/db/blocks.rs:1267`. Open-and-close holds steady at 70 mappings and 20 MB. The seam is `#[doc(hidden)]` and keeps its store by design (`ManuallyDrop`). A test that crashes thousands of times in one process will run out of mappings, as this harness did at first. | `src/db.rs:53553` (`Db::crash`) | Say in its doc that the memory is kept for good. Or free the store's anonymous memory in `crash` once its threads have ended, since a dead process's memory is gone anyway. |

No hang was found anywhere. No crash or misread was found in: the manifest slots (3,000 targeted cases per kind of store), the run temp file, a store in one file's `store.one` (3,000), the vault marker (3,000), the sealed slots and sealed log, or the `crypt` envelope (4,150 cases: random changes, every header field at its extremes, and truncations at, beside and inside each chunk boundary, at three chunk sizes). Every change to an envelope was refused by `crypt::decrypt`. The answers of every `Encrypted` that did open were exact.

## Evidence

Every command below runs from the repository root unless it says otherwise, on a tree at `98e4e8f4`, with a `TMPDIR` of its own:

```sh
export TMPDIR=$(mktemp -d) && TMPDIR=$(cd "$TMPDIR" && pwd -P)
cd release/findings/hostile-input/harness
cargo build --release                 # the checked profile's checks: overflow checks, debug assertions
cargo build --profile plain           # the shipped release profile's: neither
```

`release` here is the harness's own profile, which turns overflow checks and debug assertions on to match `--profile checked`. `plain` is the crate's shipped release behaviour. Each harness target prints only failing cases, each as `PANIC`, `ALLOC`, `MISREAD` or `HANG`, followed by one summary line. `HOSTILE_BT=1` adds the engine's frames to a panic or to the first large allocation, and `HOSTILE_WHY=1` prints each refusal and each earlier-state open. The harness is `harness/src/main.rs`. Its segments come from the public `SegmentWriter` in six shapes: default; every value in blocks; compressed; 512-byte blocks; checksums off; compact records off. Each holds 302 keys, including the empty key and a 33-byte `0xff` key, with runs of 0, 1-3 and 30 values, fixed-width runs, and empty and 3,000-byte values.

### Segment readers

| run | command | cases | opened | refused | failures |
|---|---|---|---|---|---|
| raw mutations (flips, set bytes, `0xff` and zero runs, u32/u64 words at 0, 1, 0xff, 0xffff, 2^31-1, 2^32-1, 2^32, 2^63-1, 2^64-8, 2^64-1, truncation, extension) over the superblock page, the key header, the key section, the block table and blocks, or anywhere | `./target/release/hostile segment 0 20000` | 20,000 | 14,322 | 5,678 | 26 MISREAD (HI-1), 9 ALLOC (HI-2) |
| the same, shipped profile | `./target/plain/hostile segment 0 2000` then `2000 20000` | 20,000 | 14,322 | 5,678 | the same 26 + 9 |
| the same mutations, the key section's row and both superblock FNVs recomputed (a forger's file) | `./target/release/hostile segforge 0 20000` | 20,000 | 16,150 | 3,846 | 9 ALLOC (HI-2), 4 PANIC: 3 capacity overflow at `Sec::read` (HI-2), 1 at `blob.rs:3397` (HI-3) |
| the same, shipped profile | `./target/plain/hostile segforge 3000 20000` | 17,000 | 13,732 | 3,265 | 7 ALLOC, 3 PANIC (capacity overflow) |
| each of the 16 superblock words at 15 extremes, in one slot and in both, FNV recomputed; each of the 24 key-header words, and each half-word, at 12 extremes, row recomputed | `./target/release/hostile superfields` | 8,064 | 6,754 | 1,286 | 24 PANIC at `blob.rs:3397` (HI-3) |
| the same, shipped profile | `./target/plain/hostile superfields` | 8,064 | 6,754 | 1,300 | 10 PANIC at `blob.rs:3509` (HI-3) |
| the key section's `crc_off` word zeroed, one bit flipped behind the header | `./target/release/hostile crcoff 0 600` | 600 | 600 | 0 | 385 MISREAD (HI-1) |
| the word zeroed alone | `./target/release/hostile crcoff 4294967296 4294967302` | 6 | 6 | 0 | 0 |
| HI-1's attribution: every raw misread rerun with the `crc_off` word put back | `HOSTILE_RESTORE_CRCOFF=1 ./target/release/hostile segment S S+1` for each of the 26 seeds | 26 | | | 0 |

Single reproducers:
- HI-1: `./target/release/hostile segment 673 674` (a 59-byte zero run in the key header, after which `read_all` answers 0 values where 1 was written) and `./target/release/hostile crcoff 0 1`.
- HI-2: `HOSTILE_BT=1 ./target/release/hostile segment 52 53` (4,294,967,295 bytes at `bytes.rs:296`), `segment 9967 9968` (4,294,907,220 bytes at `blob.rs:2170`), and `HOSTILE_BT=1 ./target/release/hostile segforge 6936 6937` (capacity overflow, `Sec::read` at `blob.rs:494` under `Blob::open`).
- HI-3: `./target/release/hostile superfields 2>&1 | grep f3=`.

The pristine files, read whole through each `Blob` reader (`lent`, `copying`, and `mapped` over a real file mapping):
- `./target/release/hostile pristine` with all six shapes written in one process: shape 1 has 3 errors (HM-2) and shape 4 has 178 (HL-4).
- `HOSTILE_ONLY_SHAPE=N ./target/release/hostile pristine`, one shape a process: shape 1 has 3 errors (HM-2), and every other shape reads every key. `HOSTILE_WHY=1` names the erring keys, which are those with no values.
- `./target/release/hostile unaligned`: each shape lent at +0..+7 bytes from an 8-aligned address. Every +1..+7 case is refused at open (HL-3), 42 of 42.

Each misread line names only the first reader that answered wrong, which is the lent `Blob` because it runs first. The three `Blob` sources and `SparseBlob` share the key section's parse. The browser reader's 50 misreads under `crcoff` (below) are the same defect in a different reader.

### Store directories

`build_store` writes three phases: a load of 1,502 keys with appends, flushed; overwrites, flushed and settled; then appends left in the WAL, synced, and a crash. Every key's state after every commit is kept. Each case copies the directory, changes one file (half the time in its first 4 KiB), opens with the same `Options`, reads every key, counts and scans, and closes.

| run | command | cases | opened | refused | failures |
|---|---|---|---|---|---|
| plain store, any file | `./target/release/hostile store 0 20000` | 20,000 | 14,827 | 5,173 | 0 |
| store in one file, any file | `./target/release/hostile onefile 0 8000` | 8,000 | 6,623 | 1,377 | 0 |
| store under a cipher, any file | `./target/release/hostile cipher 0 8000` | 8,000 | 3,785 | 4,215 | 0 |
| per file (append the file's prefix): `wal`, `manifest`, `direct`, `par` (plain); `wal`, `manifest`, `direct`, `store.one` (one file); `wal`, `manifest`, `supdb.cipher` (cipher) | `./target/release/hostile KIND 0 3000 PREFIX` | 3,000 each | | | 0 each |
| the newest WAL, every mutation | `HOSTILE_WHY=1 ./target/release/hostile store 0 3000 wal` | 3,000 | 1,719 (848 at an earlier state) | 1,281 | 0 (HM-1) |
| the same under a cipher | `HOSTILE_WHY=1 ./target/release/hostile cipher 0 3000 wal` | 3,000 | 1,567 (900 earlier) | 1,433 | 0 (HM-1) |
| bit 0 of each of the newest WAL's first 512 bytes | `HOSTILE_WHY=1 ./target/release/hostile walhead 0 512`, `walhead-cipher`, `walhead-onefile` | 512 each | 505 (504 earlier) each | 7 each | 0 (HM-1) |
| a key of 65,535, 65,536 and 70,000 bytes | `./target/release/hostile bigkey`; `./target/plain/hostile bigkey` | 3 | | | 2 PANIC; 2 MISREAD (HI-5) |
| mappings over repeated opens | `./target/release/hostile leak 0 501 crash`, `... close` | 501 each | | | 73→5,062 mappings and 20→329 MB (crash); 70→70 and 20→20 MB (close) (HL-2) |

HL-1: `HOSTILE_WHY=1 HOSTILE_TRACE=1 ./target/release/hostile cipher 0 20` prints each case's file and its refusal. In the run here, seed 17 changed `x-<token>.o` and the open refused the store. Under a cipher, tokens are random, and a store's seals and merges land on the upkeep's schedule, so the file list, and so the file a seed picks, can differ between runs. The store rows' counts therefore move a little from one run to the next, and their zeros of failures do not.

### Under AddressSanitizer

The `unsafe` code past each reader's checks (the `&[Ext]` and `BlockRec` views, the LZ4 decoder, the mapping lends, the pager) is checked for out-of-bounds access by building the harness under ASan:

```sh
cd release/findings/hostile-input/harness
CARGO_TARGET_DIR=$(mktemp -d) RUSTFLAGS=-Zsanitizer=address \
  cargo +nightly build -Zbuild-std --target x86_64-unknown-linux-gnu --release
ASAN_OPTIONS=detect_leaks=0:allocator_may_return_null=1 HOSTILE_SECS=300 \
  $CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/hostile TARGET FROM TO
```

The result is **0 AddressSanitizer reports** in 22,410 cases. The table gives the harness's own failures, which are the findings above:

| target | cases | harness failures |
|---|---|---|
| `segment 0 5000` | 5,000 | 11 (HI-1, HI-2) |
| `segforge 0 5000` | 5,000 | 4 (HI-2, HI-3) |
| `superfields` | 8,064 | 24 (HI-3) |
| `crcoff 0 120` | 120 | 69 (HI-1) |
| `envelope 0 1000` | 1,326 | 0 |
| `store 0 1500` | 1,500 | 0 |
| `onefile 0 800` | 800 | 0 |
| `cipher 0 800` | 800 | 0 |

### The browser reader

```sh
cargo build --release --lib --target wasm32-unknown-unknown \
  --config 'profile.release.opt-level="z"' --config 'profile.release.lto="fat"' \
  --config 'profile.release.codegen-units=1' --config 'profile.release.panic="abort"' \
  --config 'profile.release.strip=true' --config 'profile.release.debug=false'
# (web/build.sh's line, with CARGO_TARGET_DIR set to a directory of your own)
D=$(mktemp -d); cd release/findings/hostile-input/harness
for t in "segment 0 3000" "segforge 0 3000" "superfields 0 0" "crcoff 0 300"; do
  HOSTILE_DUMP=$D ./target/release/hostile $t; done
cd ../../../..; node release/findings/hostile-input/wasm/drive.mjs $CARGO_TARGET_DIR/wasm32-unknown-unknown/release/supdb.wasm $D
```

The result: 14,364 cases, 11,605 opened, 24 traps (HI-4: `keyheader shapeN w13=0x100000000` and `w13=0x4000000000000000`, in `openMemory` and `openCached`; none in `openSparse`), 50 misreads (`crcoff`, HI-1, capped at 50 lines), and no other driver error. The driver checks itself on the six pristine shapes, each marked for checking: 6 opened, 0 misreads. The four HI-2 cases (segment and segforge, seeds 52 and 74) open and read in the browser reader with no trap.

### The differential fuzz past the gate's ranges

`sh release/findings/hostile-input/fuzz-long.sh MODE FROM TO 50 2` runs one process per 50 seeds, each in a `TMPDIR` of its own, under a 30-minute timeout. It keeps each block's summary line in `out/fuzz-MODE-FROM-TO.log`. Between 09:19 and 10:04 UTC (from 09:19:56Z, through `fuzz_range` unless noted):

| mode | seeds | blocks | mechanism count |
|---|---|---|---|
| `ZFUZZ_CIPHER=1` | 24..3024 | 60 ok, 0 failed | 180,860 chunks decrypted, summed over the blocks' lines |
| `ZFUZZ_ONEFILE=1` | 24..3024 | 60 ok, 0 failed | the range asserts a store kept files in its data file (`tests/fuzz.rs:3243`) |
| plain | 24..3024 | 60 ok, 0 failed | |
| `ZFUZZ_OPTVARY=1 ZFUZZ_CACHE=all` | 0..1000 | 20 ok | |
| `ZFUZZ_ONEFILE=1 ZFUZZ_OPTVARY=1 ZFUZZ_CACHE=all` | 0..1000 | 20 ok | |
| `fuzz_range_under_a_cipher_that_does_not_fit_its_cache` | 24..1024 | 20 ok | |
| `concurrent_range` | 4..104 | 2 ok | |

The gate runs 0..24 of each. Nothing failed past it.

## What was not run, and why

- **Form files** (`zpg-*.zpg`, `x-*.z`): no store the harness built kept one under the default options, so no case changed one. Their trailer and page checks were read in `format.md` and not driven here. A `ZFUZZ_ZPAGES`-shaped store in `build_store` would reach them.
- **Values or records of 4 GiB or more** (`format.md` F9, F13's 4 GiB object): not run, because they need many gigabytes of input per case.
- **A newer sealed-slot version** (`format.md` F3's medium): this is a refusal a later release would need, not a damage case. Not run.
- **AddressSanitizer over the whole store paths' longer ranges:** the ASan runs below are shorter than the plain runs, because ASan is slower. The plain runs' ranges were not repeated under it.
- **An attacker who rewrites a mapped file while it is open:** a `MAP_SHARED` file truncated or rewritten by another process under a live reader is SIGBUS, or bytes changing under a borrow. That is a contract of the mapping (`unsafe.md`) and not something a harness over copies can show.
