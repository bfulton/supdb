# PageFS: one page-granular encrypting file layer under every format

## 0. Verdict

The owner's seam, taken strictly, works for writes and for names. It removes the vault's form knowledge and the double write. It also fixes the compact form's 0.73x, because it moves from 4080-byte chunks to 4096-byte plaintext pages and keeps the tags apart.

Three places do not pay:
- **Mutable files need a small per-file shadow journal.** AEAD with fresh nonces turns a page rewritten in place into a lost page if the write tears, which a plain page cache never does.
- **The readers' explicit-ask protocol stays.** Nothing below the formats makes zero-copy lending safe with eviction on this machine (section 4).
- **The in-core read cost is bought back by a WARM state per file, not by the seam.**

## 1. The seam and the door

The seam sits below every formatter and above the kernel. The engine keeps calling `vault::` (vault.rs:1860-2229). Today a door call returns a raw `std::fs::File`, and every byte after that goes straight to the kernel (writepath.md section 2). That changes in one place: the door returns

```rust
pub enum VFile { Os(std::fs::File), Enc(Arc<pagefs::Handle>) }
// write_all, writev, write_all_at, read_exact_at, read_to_end, seek, set_len,
// sync_data, sync_all, start_writeback(off,len), advise(off,len,Advice),
// len(), try_clone()
```

Every method is one `match` beside the syscall it wraps. The rule allows that ("a branch beside a syscall").

The free functions keep their shape:
- `create`, `create_rw`, `open`, `open_write`, `open_append` and `open_slot` return a `VFile`.
- `map(path)` returns an `MmapBytes`: a plain mmap, or the layer file's plaintext view (`MmapBytes::paged`, bytes.rs:365).
- `open_mapped(path, cap)` returns a growable view over the same layer file at a fixed base. RunMap (db.rs:11454, 28741) remaps in place.
- `rename`, `hard_link`, `remove_file`, `names`, `len`, `exists`, `sync_dir`, `open_dir` and `read` work on logical names.

One piece of new semantics enters the door: a durability class given at creation, `create_regenerable`. It is used by `write_ord` (db.rs:99-131) and by zfile's sink (zfile.rs:4253). The class means "a page of this file that fails its tag is this file's damage, not the store's" (section 5). The class names no format.

These go: `persist`, `persist_one`, `persist_form`, `companion`, `kept`, `Kind`, and `Mapped`/`MappedFile` (vault.rs:468-484, 609-635, 2118-2197).

## 2. On-disk layout (one format for every kind)

The layer stores data in 4096-byte pages. Plaintext page `p` is one stored page, and tags live apart from the data.

| stored page | content |
|---|---|
| 0 | Header copy A. Magic `SUPDBPF\0` (8), version u16, shift u8=12, class u8, file_id[16], open_count u32, hdr_seq u32, the file key wrapped under the store key (nonce12 + ct32 + tag16; the AAD is every header field), and zeros. |
| 1 | Header copy B. The same layout, so a header update is A/B. |
| 2-6 | Shadow slot A: page 2 is the slot meta, pages 3-6 are up to 4 data pages. |
| 7-11 | Shadow slot B. The same layout. |
| 12 + 129g | Tag page of group g: 128 entries of 32 B. |
| 12 + 129g + 1 + i | Data page p = 128g + i. |

- **Address math.** `stored(p) = 12 + 129*(p>>7) + 1 + (p&127)`: a shift, a mask and an add, with no divide (the divide the `Per` path in `pager` avoided becomes unnecessary).
- **Tag entry (32 B):** `nonce[12] | tag[16] | len u16` (0 means absent, 1..4096) `| flags u8` (DATA, ZERO) `| 0`.
- **AAD of a data page:** `file_id | p u64 | len u16 | flags` (27 B). Only the valid bytes `[0,len)` are encrypted. A ZERO page is a tag over empty plaintext with the ZERO flag in its AAD. Its stored bytes are real zeros, so the blocks are allocated and written, which is the purpose prefill serves (db.rs:4236-4262).
- **Slot meta**, sealed under the file key: `seq u64 | logical_len u64 | n u8 | n × (p u64, len u16, nonce12, tag16)`.
- **Overhead:** 1/129 of the file (0.78%), plus 48 KiB of prefix that stays a sparse hole unless used. A 15 MB segment stores about 15.2 MB.

The same format holds every file kind: segment, direct run, ordered index (`.o`), form file (`.z`), WAL, spare and both manifest slots. The engine's plaintext is byte for byte the plain store's: the superblock at 0..`SUPER`=4096 (format.rs:43), records from `SUPER+HEADER`, WAL frames after `WAL_HEAD`.

**Marker `supdb.cipher`** is the layer's root and is not in page format. It is two 512 B slots at offsets 0 and 4096. Each holds magic, store_id[16], gen u64, a `clean` byte, key_id, and the store key wrapped under the caller's master with the slot's header as AAD. Rewrap and the clean flag write the older slot and fdatasync it.

**Names.** A stored name is `x` + base64url(`nonce12 | AEAD_storekey(name padded to 112 B | file_id[16] | name_seq u64) | tag16`), 220 characters. The longest logical name is `par-{8}-{16}-{32hex}-{32hex}.sup.z`, 101 characters (db.rs:12711). The directory itself is the name table:
- an open lists it and decrypts each name;
- `rename` is rename(2) to a freshly sealed name for the same file_id;
- `hard_link` is link(2) under a second sealed name;
- a header whose file_id differs from its name's is refused, which catches swapped entries.

Rename over an existing name cannot be one syscall, because sealed names are random. It is rename(2) to the new sealed name followed by unlink of the old entry. An open that finds two entries for one logical name keeps the higher `name_seq` and unlinks the other. That is POSIX rename-over semantics, either old or new. Name durability is exactly the plain store's: the same creates, renames and directory fsyncs.

## 3. Write path

Each file has one `pagefs::File` in memory. It is keyed by file_id, so two names and two handles share it: `chain` writes through a second handle (db.rs:4493-4502), and a run is hard-linked (db.rs:28989). It holds the logical length, a page-state table shared with the read cache, the tag pages of open groups, the shadow state, and one mutex for writers.

**Write-through to the kernel at every call.** The rule is that every `write`/`pwrite` reaches the stored file before it returns. Under a buffered policy a plain store's writes survive a process crash in the page cache, and the layer's userspace cache does not. So:
- Full pages are sealed (AEAD from the caller's buffer into a sealing buffer) and pwritten home, together with their tag page. A whole group is one pwritev: the tag page plus 128 data pages, 516 KiB contiguous, which keeps the folios large.
- A partial last page is sealed over its valid bytes only.

**The tail rule (the shadow).** A page is written home only if one of these holds:
- it is full, or
- it has never been written, or
- the write replaces it whole.

A partial rewrite of a page that already exists in the stored file goes to the next shadow slot instead. That covers the tail extended at the next commit, the chain byte, and the finish's patch of the section header at `key_off` (db.rs:7728), which shares a page with the first records. The shadow is a ping-pong:
- Writing slot X first writes home any page X holds that a later slot has not superseded. That is safe, because the page's durable copy is in the other slot.
- The slot is written as one pwritev (meta plus data pages), so a process crash cannot tear it.
- Shadowed pages stay pinned in the cache until they go home.
- With more than 4 such pages in one call, the layer runs rounds of 4, each followed by an fdatasync. This is counted and is not expected.

Patterns:
- **WAL commit** (db.rs:4436-4459): usually one pwritev of 8 KiB (the slot). Pages the commit completes go home together with the tag page.
- **Direct-run commit** (db.rs:6917-6936): the batch's full pages and the tag page go home, the tail goes to a slot, then the fdatasync passes through.
- **O_APPEND** (vault.rs:1914): the layer owns the offset and writes at the logical length.
- **`set_len`** (db.rs:27932): the cut page is rewritten as a partial page through a slot, and the later tag entries are set to absent. The stored file is not shrunk inside a group.
- **Prefill** writes ZERO pages: zeros on disk and a 16-byte tag per page, with no 32 MiB AEAD pass.
- **Recycle** (db.rs:4280-4319) is rename plus header rewrite. Logically it is the plain spare, and wal_seed refuses the last life's frames as it does for a plain store.
- **Segment finish:**
  - page 0 is rewritten whole and goes home in place (db.rs:7824);
  - whole pages of the reserve go home in place;
  - partial pages go through slots (db.rs:7728-7771).
- **Read-back of unsynced bytes** (db.rs:6198, 7753) goes through the cache, or decrypts from the stored file, which always holds what was written.
- **`sync_file_range`** (db.rs:6072-6085) and fadvise act on the matching stored ranges. The mapping is 1:1 with no straddling.
- **`sync_data` and `sync_all`** pass through to the stored fd. The ciphertext is already in the kernel, so an fsync is a real fsync again, and the list of four fsyncs a cipher store takes another way (CLAUDE.md, vault section) goes.

**Cache placement of writes.**
- A file with a live mapping or reader (a run, or a file being finished) is written through the plaintext cache: the bytes are copied into the page, then sealed from it.
- A write-once file is written around the cache unless the cache has room to keep the whole file. In that case it is written through and becomes WARM when its writer closes (section 4).

**Nonces.** A page nonce is `open_count u32 | rand u32 | counter u32`.
- `open_count` is bumped in the header, A/B, once per process per writable file. The bump rides the next fdatasync with no sync of its own.
- If a crash loses the bump, the same open_count comes back, but that repeats a nonce only if the 32 random bits also collide: 2^-32 for each lost bump.
- One open can make 2^32 page seals, which is 16 TiB.
- Store-key seals use random 96-bit nonces: file-key wraps, header updates, names and slot metas under the file key. Collision stays under 2^-32 until about 2^32 seals, which is years at ten files a second.

## 4. Read path and the plaintext cache

The pager keeps its protocol (pager.rs: the states EMPTY/BUSY/COLD/HOT/DYING, units, the mark, era and grace, `Slot.page`/`oldest_lent`, `Hold`, the rests). Its arithmetic and its fill change:
- **A chunk is a page.** `CHUNK=4080` (pager.rs:109) becomes 4096. The unit is 256 pages (1 MiB), now chosen for TLB shootdown and no longer forced by straddling. The probe measured 57-88 µs for 1 MiB against 19 µs for one page with threads running.
- **The cache is anonymous** `MAP_NORESERVE` memory, reserved at the file's growth cap, rather than a memfd. A first-touch write costs 1.5 µs on anonymous memory against 1.9 on a memfd (probe.md).
- **Hit:** unchanged, one load and compare per page asked (`ensure`, pager.rs:1312).
- **Miss:**
  1. CAS EMPTY→BUSY, then rescue.
  2. `pread` the stored page straight into the plaintext page; the kernel's copy takes the fresh-page fault inside the kernel.
  3. Take the tag entry from the file's tag-page cache (heap, counted in the budget).
  4. AEAD decrypt in place (`Aead` is in place only, crypt.rs:141-151), then mark HOT.
- **Sequential advice batches the fill.** It populates a run of up to 16 EMPTY pages in one unit with `MADV_POPULATE_WRITE` and fills them with one `preadv`.
- **Bound:** the budget counts asked pages, WARM files, pinned shadow pages and tag pages. The only dirty pages are the shadowed ones, so the evictor never writes back.

**WARM.** An immutable file whose whole plaintext fits the budget's headroom is decrypted whole: in the background, or at its write-through birth. It is then flagged warm and never evicted. `MmapBytes::lend_at` refuses through its inline bounds check (bytes.rs:481) and then calls a cold `lend_slow`, which returns the whole range if the file is warm. With that, `Sec::get` (blob.rs:504) lends whole and `Blob::flat` succeeds, so the point read and the scan take the plain loops. The ask path and `not_lent`, 95 of today's 339 instructions, are skipped.

Demoting a file under budget pressure is:
1. clear the flag (SeqCst);
2. bump the era and wait out the grace on `oldest_lent`, so that every borrow taken whole has ended (borrows live within an operation; snapshot handles are idle between reads through `page_snap`);
3. let its pages enter the clock.

A store that fits its budget runs entirely WARM. A store 10% over keeps about 90% of its files WARM.

**Rejected: userfaultfd** (probe.md). There are two modes and neither works:
- Handler-thread mode: a miss costs 30-45 µs here, from two cross-CPU wakes.
- SIGBUS mode: 4.5 µs, but a kernel access to a missing page returns EFAULT. A caller's callback that write(2)s a lent value would then fail at random after an eviction, and preventing that brings back per-read pins.

## 5. Crash consistency

| kind | power loss | process crash | torn write |
|---|---|---|---|
| segment | as plain: tmp, fsync, rename, directory fsync, then the manifest; the engine's own sweep removes unnamed files by logical name (db.rs:27632) | as plain | page 0 and the reserve are rewritten before the name exists; partial pages go through a slot |
| direct run | commit k durable: its full pages are home and its tail is in slot k; `recover_direct` (db.rs:6948) reads the same logical bytes | every write call reached the kernel | if home or a slot tears, the other slot holds commit k-1's tail; the recovered prefix ends at a marker |
| WAL | as plain; the loose and chain rules are unchanged | as plain | the tail goes through slots; a torn frame is the log's end |
| manifest slot | as plain | as plain | a whole-slot rewrite in place may tear; a page that fails its tag reads as zeros through `read`, the CRC fails, and the slot is ignored (db.rs:11893-11920 is unchanged) |
| ordered index, form file | unsynced by design; a tear is possible | as plain | see below |
| marker | A/B | A/B | A/B |

The regenerable files are handled through the `clean` byte in the marker:
- The byte is cleared at a clean close, after every layer file with unsynced writes has been fdatasynced.
- An open that finds the byte unclean verifies the regenerable files before trusting them, and a failure there is `Err`. `OrdIndex::open` (ordindex.rs:403) then rebuilds as a plain store does (db.rs:8503-8524).
- After a clean close nothing can be torn, so there is no verification.
- A form file that fails later is taken out of service, as a plain store's is. That is the ROADMAP item "Form damage taken as the form's alone".

**Tag failures.** Through a mapping, a page that fails its tag poisons the store (`Pager::check`, unchanged). Through `read` or `read_at`, it reads as zeros and is counted: those callers are formats with their own checksums.

**The shadow recovery rule.** The authoritative copy of page p is decided by the latest slot by seq whose meta verifies:
- If that slot lists p, the slot's copy wins.
- Otherwise the home copy wins.

The logical length is the larger of the last valid home extent and that slot's `logical_len`. Any subset of one call's writes landing reads as an earlier prefix or as the new state, which matches the plain torn-tail semantics.

## 6. Cryptography

- **Keys.** The master wraps the store key in the marker. The store key wraps a random 256-bit key per file, in that file's header. The file key seals the file's pages and slot metas.
- **Authenticated:** every page's bytes and length, its position in the file (AAD), and its binding to the file (file_id in both the header and the name).
- **Not authenticated:**
  - rollback of the whole directory, as today;
  - rollback of a single page of a mutable file to an earlier valid version, which is the same class as "log cut at a record end";
  - the extent of a file past its valid pages, where formats carry lengths;
  - splicing of versions across pages inside one multi-page mutable record (a manifest slot, a WAL frame that crosses pages), which the format's CRC32C catches with probability 1-2^-32 per splice. Today's slot is a single AEAD, so this is weaker; see the risks.
- **Leaked:** the file count, sizes to the page, which pages are rewritten and when (commit cadence, finish patches), and renames as events. Name lengths are padded.
- **Not leaked:** names, fences, keys and values.

## 7. Code that goes, code that stays

The line counts are estimates from the maps and were not checked line by line.

| goes | lines |
|---|---|
| vault.rs memfd make/rewrite/reopen, envelopes, `Kind`, persist*, companion, adopt_all, the companion rules in sweep, seal_manifest/open_manifest, LogCipher, `Ahead`, Mapped (of about 2,500 non-test lines) | ~1,700 |
| db.rs: the WAL's cipher branches (4203-4580), `RUN_LOGGED`/`log_run` (28219-28862), `manifest_image`/`manifest_open`'s cipher halves (11661-11685), persist call sites (12574, 12865, 13033, 28999, 29525), `cipher_controller` (11529) and `zpages_under_cipher` | ~450 |
| zfile.rs `SegSrc::Kept`/`Walked::Held` build path and `persist_form` (4204, 4302-4373) | ~300 |
| the twin-era vault tests | ~600 |

| stays or reshapes | lines |
|---|---|
| marker and keys, the door, the decoy directory, `the_engine_names_no_file_but_through_the_door` | ~700 |
| pager.rs: reshaped (chunk=page, fill from pagefs, write-through states), not shrunk | ~2,100 → ~1,900 |
| the readers' ask protocol (blob, flatindex, bytes) and the db.rs rest and pin sites (readpath.md section 4) | ~2,800, unchanged |

| new | lines |
|---|---|
| `src/db/pagefs.rs`: format, write path, shadow, recovery, names, nonces | ~1,800 |
| tests and a crash model | ~1,200 |
| `VFile` at call sites | ~200 |

## 8. Why a store in the clear pays nothing

- The door's free functions are unchanged: they load the `locate` counter and make the std call.
- `VFile::Os` adds one discriminant branch per syscall wrapper (writev per 1 MiB piece, a WAL write, a sync, a pwrite).
- No per-key, per-entry, per-block or per-step code on the plain path gains anything. Some lose things: the commit loses the `RUN_LOGGED` bit (folded into `MemTable::after` today), and the build loses the `SegSrc::Kept` selection.
- The one read-path touch is `lend_at`'s refusal arm, which goes from `None` to a cold `lend_slow`. It is held by count. If it moves the plain count, the fallback is to publish a new `MmapBytes` with `lend=len` when a file goes warm, which touches the plain read path not at all.
- **What holds it:** callgrind by function against `origin/main` for each of these: `read_only.rs supdb 100000 1 n-none`, `lag0_probe.rs supdb 100000 1`, `work_probe.rs 100000 whole`, `merge_walk.rs 100000 4 1`, `load_only.rs supdb 100000 1`, and the form rows (`supdb-nocachezpages`).

## 9. Predictions (reasoning in section 13)

- **Out-of-cache point read:** 0.17x → 0.25-0.35x.
- **A miss:** 4 µs → 2.0-2.8 µs, and about 1.5 µs a page when batched.
- **Ordered load:** 0.68x → 0.90-0.97x durable, 0.52-0.57x → 0.80-0.90x buffered, and 2.5 → about 1.35 device bytes per byte stored.
- **Compact form under a bound:** 0.73x → at least 1.0x of raw.
- **In-core point read:** +339 → +20-60 instructions when WARM; about +300 when not.

## 10. Risks

1. The shadow is a new crash-consistency mechanism under the WAL. A bug in it loses acknowledged commits. It needs a crash model and fuzzing with tearing at page and sector granularity before it carries a log.
2. Splicing versions across pages of a mutable record is caught only by the format's CRC.
3. WARM demotion and grace races with snapshot handles and builders. The cost of `lend_slow` to the plain path is unknown until counted.
4. Rename-over takes two syscalls, resolved at open by `name_seq`. A logical name longer than 112 B cannot be stored.
5. Two caches in series under a bound, the plaintext cache and the kernel's ciphertext cache. Tag pages are 1/129 of the store and compete for residency. The controller's model is still unheld (ROADMAP).
6. The write-through rule adds one or two syscalls per small buffered commit.
7. The stored layout's groups of 129 pages shift 2 MiB alignment. The folio composition of stored files differs from plain (CLAUDE.md warns that reads trail by folio composition).
8. The `clean` byte costs an fdatasync per open and a sync of unsynced regenerable files at close.
9. Verifying regenerable files after an unclean shutdown lengthens recovery opens.

## 11. Experiments owed, cheapest and most decisive first

See the `experiments_owed` field.

## 12. Staged plan (each stage keeps the gate green)

- **S0.** The experiments E1-E4, and the plain callgrind baselines at the tree.
- **S1.** `VFile` as the door's type with only `Os` present. Memfds are `Os` too. Mechanical change. The gate plus every callgrind row equal.
- **S2.** `pagefs` as a module: format, write path, shadow, recovery and names. Unit tests, a crash model (torn pages and sectors, kill at every syscall), and mutants. Not wired.
- **S3.** Wire `pagefs` under `Encryption::Cipher` behind a pricing arm, `Options::cipher_pages` (`supdb-cipherpages` beside `supdb-cipher`). The WAL, the manifest and runs go through it, and `log_run` is off under the arm. Run the fuzz under `ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384` and `ZFUZZ_RECYCLE=1` on both arms, and `tests/cipher.rs` on both.
- **S4.** The pager reshaped: chunk=page, fill from pagefs, batching, then WARM. Re-run `a_chunk_is_not_lent_out_of_a_unit_marked_again_under_its_rescue` and the guard mode.
- **S5.** The pairings on the five cells (E5-E7). Flip the default, then delete the old shape (section 7). `zpages_under_cipher` goes and the controller is on.
- **S6.** The mutant pass over pagefs (nonce reuse, slot choice, name_seq, tag-zero semantics, clean flag), and the leak and resident tests.

## 13. Reasoning behind the predictions

**Miss.** The current 4 µs breaks down as over half the fresh-memfd fault, a fifth the copy and a tenth the cipher (CLAUDE.md). The new miss:
- `pread` into the page: 0.75 µs including the kernel's copy (probe.md);
- a fresh anonymous page faulted inside the kernel: about 1-1.5 µs, or about 0.5 µs in bulk;
- cipher: 0.4 µs;
- no `Per` or unit arithmetic.

**Out-of-cache point read.** A record no longer straddles chunks. The hash slots and tag pages stay resident, so a read takes about one miss at roughly half today's cost. It is still bounded by the decrypt into a fresh page.

**Ordered load.** No log frames, no envelope at the close (43-55 ms of a 77-94 ms drain), and no 14 ms log unlink. What is added:
- AEAD at about 7.9 GB/s, which is 5 ms per 40 MB;
- the plaintext pages the run's mapping needs, about 0.5 µs a page;
- tag pages at 0.8%;
- 8 KiB of slot per commit.

**Compact form.** A form page is one stored page plus a hot tag page (CLAUDE.md compact-form section).

**In-core point read.** WARM lends whole sections, so the plain loops run. What remains is the cold `lend_slow` call per lookup and the read's start and end out of line (`paged_begin`, `end_slow`).
