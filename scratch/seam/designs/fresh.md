# Sealed pages under the door, with a WARM/PAGED plaintext tier

## 0. Verdict
The cipher moves below every formatter. It becomes a page file layer (`pfile`) behind the existing `vault::` door. The layer knows pages, lengths, names and three durability classes. It knows nothing of segments, logs, manifests or forms.

Four choices carry the performance case:
1. **A plaintext page is exactly one stored 4096-byte page, and tags are kept out of line.** This fixes the compact form's two-chunk read and removes the 4080 arithmetic.
2. **Every file is sealed as it is written.** There is no memfd, no envelope pass and no double write of an ordered run.
3. **A file whose plaintext fits the budget is published WARM.** It is lent whole through the plain flat reader, so in core a store under a cipher reads the plain store's code. Only out of core is a file PAGED behind today's explicit-ask pager. There, a miss costs a device read, and the ask's instructions disappear under it.
4. **Plaintext is anonymous memory the engine reclaims, and ciphertext is the kernel's to reclaim.** When the budget is most of memory, reads are O_DIRECT, so the same bytes are not held twice.

Rejected:
- **userfaultfd or SIGSEGV transparency.** In handler mode a miss costs 30-45 µs (probe.md). SIGBUS mode returns EFAULT on a caller's `write(2)` of a lent value. Default container seccomp denies userfaultfd. It would also mean signal handlers inside an embedded library.
- **Sealing at the writer's 1 MiB piece.** A random read would decrypt 1 MiB.
- **Pinning hash slots whole.** The flat reader borrows the whole key section, and inline records live inside it (CLAUDE.md, the model's populations), so pinning the slots alone does not let `flat()` run.

## 1. The seam and the door
The door stays the only way to a file (vault.rs:1860-2229). The engine stays pointed at a decoy directory under a cipher (vault.rs, "a directory that cannot exist"), so a call that bypasses the door fails and writes nothing.

New and changed calls:
- `vault::File` replaces `std::fs::File`:
  ```
  enum File { Os(std::fs::File), Pg(Arc<pfile::Handle>) }
  write_at / write_all_at / write_vectored_at / append(&[IoSlice])   // O_APPEND: the layer owns the offset
  read_at / read_exact_at / set_len / len / sync_data / sync_all / try_clone / seek
  writeback(off,len)    // sync_file_range(WRITE) on the stored range
  drop_cache(off,len)   // fadvise DONTNEED on the stored range
  ```
  Every method is one match beside its syscall.
- **Directory calls:** `create(path, Class)`, `create_rw`, `open`, `open_write`, `open_append`, `rename`, `hard_link`, `remove_file`, `names`, `len`, `exists`, `sync_dir`, `open_dir`, `read`. All take logical names.
- **Two new generic calls:**
  - `preallocate(file, len)`. Plain: today's zero loop and `sync_all` (db.rs:4252-4262). Paged: raw zero blocks past the logical length, which stays unchanged.
  - `recycle(spare, path)`. Plain: rename then `open_write` (db.rs:4280). Paged: a new life (new file key, length 0) is written into the spare's header and synced, then renamed.
- **Classes:** `Once` (trusted only after a sync and a name; writes may be pipelined), `Log` (each write reaches the page cache before it returns; a torn tail is the end), `Regen` (an authentication failure is this file's error, not the store's). These are durability classes, not forms.
- **Mappings:**
  - `map(path) -> MmapBytes`: a plain mmap, or a pager view (`MmapBytes::paged`, bytes.rs:365) with `lend = 0` (PAGED) or `lend = len` (WARM).
  - `map_growing(path, cap)` for `RunMap` (db.rs:11461): a WARM view the writer fills as it writes.
- **Unchanged:** the `Paging` trait (bytes.rs:300-330: ensure, piece_end, piece_from, advise, resident, forget, willneed). Its advice is translated to stored ranges.

Deleted from the door: `persist`, `persist_one`, `persist_form`, `companion`, `kept`, `Kind`, `open_mapped`/`Mapped`/`MappedFile`, `passes`, the memfd table, the write-once rule, `seal_manifest`/`open_manifest`/`adopt_all`, and `LogCipher`.

## 2. On-disk layout (one layout for every kind)
| stored bytes | content |
|---|---|
| 0-32 KiB | **Header A.** 32 KiB = 8 pages. |
| 32-64 KiB | **Header B.** Same shape. |
| 64 KiB-1 MiB | **Journal ring**, 240 pages, sparse until used. |
| 1 MiB + g·1 MiB | **Group g**: one tag page, then 255 data pages. |

**Header content.** Each copy holds:
- magic `SUPDBPG\x01`, class, file_id[16], checkpoint seq u64, logical length u64, flags (sealed, synced-closed);
- a session table: up to 32 × {file key wrapped under the store key (nonce12 | ct32 | tag16), AAD `"pfile"|file_id|idx`};
- an exception table: {page u30, ver u16, sess u8} for pages not written once by their group's session;
- the journal's head position and frame seq.

The header is sealed under session 0's key. Its nonce is (domain=header, seq), and A and B alternate by seq.

**Tag page.** 255 × 16-byte tags, then 16 bytes: group index u32, `split` u8 and two session indexes, covering pages [0,split) and [split,255).

**Where page p is stored.** `stored(p) = 1 MiB + (p/255)·1 MiB + 4096·(1 + p%255)`. The divide is by a constant, a multiply and a shift, and it runs on a miss only. A hit indexes chunk states by `off>>12`. Groups are 1 MiB-aligned, so a whole group is one aligned write. Overhead is 1/256 plus a sparse 1 MiB prefix.

**Journal frame.** `seq u64 | kind u8 | page u30 | off u16 | len u16 | ct | tag16`, with nonce (domain=journal, seq). Kinds:
- `TAIL`: bytes of the current partial page;
- `TAGS`: tags of full pages in the open group;
- `IMAGE`: a whole new version of a page that a sync had already covered;
- `LEN`, `SESSION`, `CKPT`.

**What each kind stores.** Plaintext is byte for byte the plain store's for:
- the segment, its superblock at 0..4096 (format.rs:43);
- the direct run;
- `.o`, `.z`, the WAL and spare, and the two manifest slots.

So the manifest names fenced logical names, with no tokens.

**Marker `supdb.cipher`.** Two 512-byte slots at 0 and 4096, not in page format. Each holds store_id, gen, key_id, and the store key wrapped under the caller's key. This is as today (vault.rs:362-449), with gen bumped and synced at every open.

**Names.** Each name is `x` + base64url(nonce12 | AEAD_store(name padded to 112 | name_seq u64) | tag16), about 180 characters.
- At open the layer lists the directory and decrypts the names into an in-memory map.
- `rename` and `hard_link` are `link(2)` to a newly sealed name, then `unlink(2)` of the old one.
- If an open finds two stored names for one logical name, the higher `name_seq` wins and the other is removed.
- The header's file_id binds the content. An entry whose header does not open is refused.

## 3. Write path
**R1.** A page that a completed sync covered in this life is never overwritten in place without its new version first going to the journal as `IMAGE`, in the same sync. The home write follows, and the IMAGE frame stays live until a later sync covers the home copy. Everything else follows from R1.

- **Full pages.** Each page is sealed from the caller's buffer into a reused, pre-faulted 2 MiB staging buffer and written home with `pwritev`. When a group completes, its tag page is written once, beside the group. Under `Once` the sealing is pipelined onto the existing `WriterHelper` thread (db.rs:6091), and `sync` waits for it.
- **Partial tails** (the WAL at every commit, db.rs:4436-4459; the direct run, db.rs:6917-6936). The new bytes go as one `TAIL` frame of exactly their length, plus about 32 bytes. The open group's new tags go as a `TAGS` frame. Both are appended to the ring in one `pwrite`, beside the home `pwrite` of any pages that filled. A partial page is never written home until it is full, or the file is sealed with a final length. Under a buffered policy this happens at every `write`, so a process crash keeps it, as the page cache does for a plain store.
- **In-place patches.** The segment finish rewrites the reserve and page 0 (db.rs:7728-7824), the WAL chain byte (db.rs:4493-4502) and the manifest slot (db.rs:11719). The patch is sealed under (page, ver+1) and goes as `IMAGE` and then home. The exception table records the new version. That is a few pages a seal, one page a WAL life, and the slot's pages a publish.
- **Journal space.** The ring is reused only behind the oldest frame a durable state still needs. If it would lap, the layer folds by writing a `CKPT` header (tags and exceptions) with one `fdatasync`. This is rare: the ring holds 960 KiB of frames.
- **Read-back of unsynced bytes** (finish db.rs:7753, helper db.rs:6198). `read_at` flushes the pipeline, then serves the tail from memory, the plaintext tier if resident, and otherwise `pread` of the ciphertext, which is still in the page cache, then decrypt.
- **Mapped while written.** The `RunMap` view is WARM. The layer copies each written page into it, which is the copy a plain `write(2)` makes into the page cache. The view is not evictable until it is dropped, as today's `Mapped` hold works.
- **Other calls:**
  - `set_len`: a `LEN` frame. A later write over the cut is a rewrite under R1.
  - `O_APPEND`: the offset is the layer's.
  - `rename`/`hard_link`: name operations only. The key is per file, not per path.
  - prefill becomes `preallocate`, so the WAL's commits are first writes, not rewrites.
- **Ordered run.** It returns to the plain path: `RUN_LOGGED`/`log_run` (db.rs:28834-28862) go, and so does the envelope at close (db.rs:28999). The cipher's cost is one sealing pass on the commit thread, about 7.9 GB/s with `ring` (CLAUDE.md, crypt_cost).

## 4. Read path and the plaintext tier
**Plaintext is anonymous memory, never a memfd.** Each file gets a private `MAP_NORESERVE` address range of its plaintext length, with `MADV_DONTDUMP`. Addresses do not move, so the rests' re-asks hold.

**Budget B.** By default, B is the cgroup's `memory.high` or `memory.max`, less the engine's anonymous memory, less a reserve of 5% and at least 64 MiB. It is re-read every 100 ms and shrunk on a memory-pressure PSI trigger. The caller can cap it (`StoreCipher::cache_bytes`).

| holder | what | who reclaims |
|---|---|---|
| WARM files | whole plaintext, lent whole | engine, by demotion only |
| PAGED pages | 4 KiB pages, clock over 1 MiB units | engine evictor (pager.rs:605) |
| tag tier | 16 B/page, loaded a group at a time, pinned up to B/16, LRU past that | engine |
| staging | 2 MiB a concurrent writer, reused | engine |
| ciphertext | page cache: write-back in flight, and, in the inclusive policy, recent reads | kernel; the engine drops written ranges after writeback |

**Policy.** When B ≥ ½ of the bound, reads are **exclusive**: misses use `pread(O_DIRECT)` straight into the destination page, which is 4096-aligned because the page layout makes it so. The fallback is buffered with `DONTNEED` where O_DIRECT is refused. When the caller sets a small B, reads are **inclusive**: buffered `pread`, and the kernel's cache of ciphertext is a second tier sized by what memory is left.

**WARM.** A file is WARM while every file's plaintext fits in 0.85·B.
- Promotion fills the file with background sequential reads (1 MiB `preadv`, then decrypt). The segment work then publishes a remap of its `Seg` with `lend = len`, keeping the blob id and its derived caches (`ranks`/`bounds` are keyed by blob id, db.rs:5192-5230; carrying them across is **not verified**).
- When the total passes B, files are demoted the same way with `lend = 0`. Their units become evictable once the replaced state's grace has passed, through the existing retire machinery.
- Seals and merges write-allocate their outputs into WARM directly when the store fits.
- Plain `MmapBytes` stays as it is: `lend` remains a plain field (bytes.rs:357-371), with no atomic.

**PAGED** is today's pager (pager.rs) with `CHUNK = 4096`. A unit is exactly 256 pages (1 MiB). The 4080 `Per` and the "255 pages" units go.
- **Hit:** one SeqCst load `== HOT` (pager.rs:1312).
- **Miss:**
  - CAS `EMPTY→BUSY`;
  - the group's tags from the tag tier (one 4 KiB read the first time);
  - an exception lookup, in a hash map only for files that have exceptions;
  - one `pread` of the stored run of pages into place (a `preadv` across a tag page);
  - `open_in_place`, then `HOT`.

  Today's fill copies from a mapping of the envelope (pager.rs:966-1000). The new fill has the kernel copy into the page directly and decrypts there. `willneed` becomes async fills on two I/O threads, up to 256 KiB each, which is the roadmap's "run of chunks populated and decrypted as one".
- **Eviction safety** is unchanged. A unit is marked, the epoch bumped, the evictor waits for `oldest_lent` (db.rs:9985), then gives the unit back. `give_back` uses `MADV_DONTNEED` on anonymous memory where today it uses `MADV_REMOVE` on a memfd (pager.rs:1585). Every rest, `unlent` and `Hold` site stays (readpath.md section 3), and is exercised only by PAGED files.
- **Failure.** A tag failure poisons the store (`Pager::check`), except for a `Regen` file, whose map or read returns `Err`. The ordered-index open (ordindex.rs:403-430) then rebuilds it, and the form file is taken out of service as in the clear.
- **Verification at open.** A `Regen` file closed without a sync is verified whole at its first open in the process, at the cost of the plain open's CRC pass over it.

## 5. Crash consistency
Recovery reads header A or B, whichever is valid with the highest seq. It then replays journal frames in seq order up to the first one that fails its tag, and checks each home page referenced past the older header's length. If one fails, recovery falls back to the older header and its frames, which R1 kept intact.

| kind | process crash | power loss / torn |
|---|---|---|
| segment / `.z` (`Once`) | unnamed until fsync → rename → manifest, as plain | torn before the sync: file unnamed and swept. Finish patches are IMAGE-covered |
| direct run (`Log`) | every write is in the page cache, as plain | synced prefix = durable frames + home pages. Torn home page → its TAIL frames. Torn last frame = end, the same as the marker walk |
| WAL (`Log`) | as plain | torn tail = end. Loose/chained kinds are plain bytes. Recycle: new life synced into the spare before rename, so a crash at rotation reads an empty log |
| manifest slot | as plain | the slot rewrite is IMAGE-covered, so an open sees the old or the new slot whole. The A/B slot rule still applies above it |
| `.o` (`Regen`, unsynced) | as plain | torn → `Err` at open → rebuilt, as plain (db.rs:8503) |
| marker | A/B | A/B |

What is stronger than plain: a power loss leaves a prefix of each file, never interleaved garbage.

What is weaker: nothing found. The one new rule is the fold `fdatasync` when the ring is full.

## 6. Cryptography
**Keys.**
- The caller's key wraps the store key, in the marker.
- The store key wraps one fresh 256-bit key per **write session** of each file, kept in its header's session table. A session is one open for writing.
- The store key also seals names, with random 96-bit nonces (about 2^24 names before 2^-49).

Because every write session has a fresh key, reopening a WAL or manifest slot cannot reuse a nonce, even if an attacker rolls the marker back.

**Nonces.** 96 bits = domain (2) | counter (62) | 32 zero bits.
- page: counter = page<<16 | ver;
- journal: counter = frame seq;
- header: counter = checkpoint seq.

When a page reaches ver 65535 within one session, the file moves to a new session. A file with more than 32 sessions is folded: its live pages are re-sealed under the newest session, which matters only for small, long-lived slot files.

**AAD.**
- page: file_id | page | len | ver;
- frame: file_id | seq | kind | page | off;
- header: file_id | seq | copy.

**Authenticated:** every plaintext byte; each page's place and version, through the exception table in the sealed header and journal; the length; and the binding of a name to a file_id.

**Not authenticated (as today):**
- the store taken back to an earlier state of itself;
- a whole file taken back to an earlier synced state of itself.

**Leaked:**
- the number of files and their sizes in pages;
- the commit sizes, through the TAIL frame lengths;
- which pages are rewritten;
- the timing.

Plaintext can reach swap (`MADV_DONTDUMP` keeps it out of core files only).

The AEAD is the caller's hook (crypt.rs `Aead`), with a 96-bit nonce and a 16-byte tag. The crate still ships no cipher.

## 7. What goes and what stays (estimates from the maps; not counted line by line)
**Goes:**
- vault.rs memfd table, `Kind`, `companion`, `persist*`, the sweep's form rules, tokens and the manifest sealing, `adopt_all`, `LogCipher`, `Mapped`/`MappedFile`: about 1,500 of the 2,230 non-test lines (vault.rs:2232 begins the tests).
- db.rs: the WAL's cipher branches (4203-4580), `log_run`/`RUN_LOGGED`, the persist calls (12574, 12865, 13033, 28999, 29525), the `manifest_image` sealing, and `cipher_controller` (11529): about 600.
- zfile.rs `SegSrc::Kept` and `persist_form`: about 200.
- pager.rs: the 4080 unit arithmetic, the memfd, and adopt: about 250.

**New:**
- `pfile`: about 2,000 (layout, journal, recovery, names, sessions, I/O threads).
- the `vault::File` enum and the remap publish: about 400.

**Stays:**
- the pager's state machine and grace;
- `Sect`/`Pieces`/`Sec`/`scan_pieces` (readpath.md section 2) for PAGED;
- every rest site;
- the marker and key wrapping;
- `crypt.rs` v1, untouched.

## 8. Why a store in the clear pays nothing
- Plain `MmapBytes` and every reader are byte for byte as they are; `lend` stays a plain field.
- The only new branch is `File::Os|Pg`, which sits beside a syscall: once a piece (`writev_all`, db.rs:6046), a patch, a commit's write, or a sync.
- The WAL loses its cipher branches and the commit loses `RUN_LOGGED`'s byte test. Both are removals.
- `preallocate` and `recycle` run today's code in the plain arm.

This is held by callgrind by function against `origin/main` on every row:
- `read_only.rs supdb 100000 1 n-none`;
- `lag0_probe.rs supdb 100000 1`;
- `work_probe.rs 100000 whole`;
- `merge_walk.rs 100000 4 1`;
- `load_only.rs supdb 100000 1`;
- the compact form's point read and scan (`supdb-nocachezpages`).

## 9. Predicted effect on the five numbers
1. **Out-of-cache point read, 0.17x.** That figure is a 64 MiB cache with memory to spare. At the default B, 1M keys fits, the store is WARM, and the read runs the plain code: **0.95-1.0x**. With 64 MiB forced (inclusive): **0.25-0.35x**, since a miss falls from 4 to about 2.2 µs and the code path is unchanged. Under a real cgroup bound, both stores miss to the device and the cipher adds about 2 µs to a 30-80 µs fault: **0.9-1.0x**.
2. **A miss, 4 µs.** About **2.0-2.5 µs** when the ciphertext is in the page cache: one `pread` straight into the page, an anonymous first touch (1.5 µs in probe.md, against 3.1 for a memfd read fault), and 0.5 µs of decrypt. There is no envelope-mapping copy and no 4080 misalignment. A device miss costs the device plus about 1.5 µs.
3. **Ordered load, 0.68x / 0.52-0.57x, 2.5 device bytes per byte.**
   - Gone: the log double write and the 43-55 ms envelope plus the 14 ms unlink at the drain (CLAUDE.md:1486-1496).
   - Left: sealing 40 MB is about 5 ms of a 155 ms load, plus a TAIL/TAGS frame a commit.
   - Prediction: **0.88-0.95x durable, 0.82-0.90x buffered, about 1.33 device bytes per byte.**
4. **Compact form under a bound, 0.73x.** A form page is one stored page, and its tags are resident, so a cold form read takes one device page, as in the clear. The form should read ahead of the raw form by the clear's margin (**≥1.0x of raw**, so the controller can be on).
5. **In-core point read, +339 instructions; scan mix 0.78-0.87x.** WARM means `flat()` succeeds, so `not_lent`, the asks and the windows are gone. What remains is the out-of-line begin and end behind `ATTN_PAGED`. Prediction: **+20-60 instructions**, and about 0 if a store with every file WARM makes its handles without the bit (`paged_bits`, db.rs:18204; refreshing that bit is **not verified**). Scan mix **0.95-1.0x**.

## 10. Risks
- **Anonymous plaintext is not kernel-reclaimable.** A misread budget is an OOM where the plain store would have shed page cache. Mitigations: PSI, a 100 ms re-read, and the caller's cap. With no cgroup, MemAvailable decides, which is the RocksDB/WiredTiger stance and differs from the plain store's.
- **Demotion waits for grace.** A long pin delays freeing during a memory spike. The 0.85 hysteresis makes demotion early.
- **Journal and exception bookkeeping.** A nonce bug is catastrophic. Defense: a test hook that records every (session key, nonce) pair sealed, asserted unique across the crash fuzz.
- **O_DIRECT has no readahead.** PAGED scans out of core depend on `willneed` forwarding and sequential detection.
- **The remap of a `Seg`** may drop derived caches, which would make the first scan after a demotion pay.
- **Sessions accumulate on slot files.** Folding rewrites them.
- **Folio shape of the stored file** differs from plain. That matters only to the inclusive tier.
- **Name decrypt at open** scales with the number of files. Names are capped at 255 bytes.
- **Moving to `CHUNK = 4096`** touches the hunted rescue code. Its tests stay.

## 11. Experiments owed before building (cheapest and most decisive first)
- **E1. The miss, in C, in scratch.**
  - Question: does a fill by `pread` into a fresh anonymous page plus in-place GCM beat 4 µs here, and does `MADV_FREE` at give-back avoid the fresh fault on a refill?
  - Method: compare page-cache-hit `pread`, `O_DIRECT`, and the memfd copy.
- **E2. A WARM stand-in.**
  - Question: does lending whole remove the read path's cipher cost?
  - Method: on a branch, a cipher `map` returns `lend = len` while the store fits. Run `read_only.rs supdb-cipher 100000 1 n-none` and `lag0_probe` under callgrind against `supdb`, by function.
- **E3. The plain door.**
  - Question: is the plain door free?
  - Method: callgrind of the `vault::File` enum, plain arm only: `work_probe`, `load_only` and `merge_walk` against `origin/main`.
- **E4. Seal-as-written load.**
  - Question: is the writer's sealing pass the load's remaining cost, or is pipelining needed?
  - Method: a stand-in that encrypts each write in place, with no journal. `bench/scripts/pair.sh supdb-cipher supdb 300000 --passes load` with `SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1`, and the buffered pair.
- **E5. Exclusive against inclusive.**
  - Question: miss rate and device bytes per read against plain.
  - Method: `pair.sh supdb-cipher supdb 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1`, with the faults counted per read.
- **E6. The cell the layout exists for.**
  - Method: `pair.sh supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes read`, run at S3.
- **E7. The tail journal's tax.**
  - Question: what do small commits pay for the tail journal?
  - Method: the buffered mixes A and F, `pair.sh supdb-ingestcipher supdb-ingest 300000 --passes mix`, with `SUPDB_MIX_KINDS=1`.
- **E8. Crash states.**
  - Method: build torn and lapped-ring states by hand in `pfile` unit tests, and run `ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384`, with nonce-uniqueness asserted.
- **E9. Directory operations.**
  - Method: `strace -f -tt -T` over a seal and a merge under a cipher, counting the extra link/unlink and their cost against the discard price (CLAUDE.md, the inode freed).

## 12. Staged plan (the gate stays green at each stage)
- **S0.** Experiments E1-E3. Nothing is built.
- **S1.** `vault::File` with only the plain arm, at every door call, plus `preallocate`/`recycle`. Callgrind rows against `origin/main`.
- **S2.** `pfile` standalone, with unit tests under a stand-in cipher: layout, R1, journal, recovery, torn states built by hand, and nonce uniqueness. Not wired.
- **S3.** The pager reads kept files from `pfile` instead of envelopes, with `CHUNK = 4096`. Files are still memfd while written. This lands the layout first, and E6 (the form cell) is priced here, before anything else is built on it.
- **S4.** Write-through. Kept files are written as `pfile` directly and `persist*` becomes a no-op. The ordered run goes back to the plain path and `log_run` goes. Price the loads (E4).
- **S5.** The WAL and manifest go through `pfile`, `LogCipher` and the tokens go, names are sealed by the layer, and `Kind`/`companion` go. Run the fuzz under a cipher and `tests/cipher.rs`.
- **S6.** Budget from the cgroup, the exclusive/inclusive policy, and WARM/PAGED with the remap. Price E5.
- **S7.** `Regen` scope, per-file poison, the controller on under a cipher, and `cipher_controller` deleted.
