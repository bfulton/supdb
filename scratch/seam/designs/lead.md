# Pagefile: the lead's sketch, completed

## 0. Verdict and what changed in the sketch

The seam is a file layer under the `vault::` door. Every formatter writes plaintext at plaintext offsets, exactly as a plain store does. Below the door, each logical file is one ciphertext file. A 4096-byte plaintext page is a 4096-byte stored page, and the tags are kept apart from it. The layer knows pages, syncs and names. It knows nothing about segments, forms or runs. The WAL, the manifest slots and the marker keep their existing record sealing.

I checked the sketch against the code. Twelve corrections:

1. **The sketch says nothing a recovery reads is ever patched. That is false.** The direct run's finish rewrites page 0 (superblock, db.rs:7818-7824). It also rewrites the section header at `key_off = SUPER` (db.rs:7728, `HEADER=192`, flatindex.rs:56). That header sits in page 1, which also holds committed records: recovery walks from `SUPER+HEADER` (db.rs:6950). And the temp file is what recovery reads until the manifest names the segment (db.rs:27726-27769, the `close_direct` doc). If that write tears, every record in page 1 is lost. The fix is a per-file durability class. A file that recovery reads while it is being written is *journaled*, and its patches of synced pages go to shadow pages inside the commit record (§3).
2. **Tag blocks are not interleaved every G pages.** They live in a tag region at the head of each 256 MiB extent. That keeps the data of a 1 MiB piece at a 1 MiB-aligned stored offset, so the kernel builds the same folios a plain store gets (db.rs:5726-5733).
3. **Tag entries carry no version.** A page's epoch is 0 unless the record's patch list says otherwise.
4. **Two alternating slots are not enough once records are written at unsynced flushes.** Alternation overwrites the synced record. Instead, the slot holding the last *synced* record is never written until the next sync. Every record carries `synced_len`, and it carries the tags of the group that was open at the last sync.
5. **"Commit record at every unsynced flush" applies only to journaled files, and only where bytes land.** A buffered run's commit writes whole 1 MiB pieces only (`stream_flush(false)`, db.rs:5707-5745). The partial tail stays in the writer's userspace buffer until a sync (db.rs:6917-6936, 28788-28796). So in plain too, a process crash loses the commits held there.
6. **Random directory names need a sequence number.** The engine renames over an existing name in two places: an ordered index rebuilt over a torn one (db.rs:27764) and a form file rebuilt over its own name (zfile.rs:4360). The longest logical name is `par-{8}-{16}-{32hex}-{32hex}.sup`, 99 characters (db.rs:12711, fences of 16 bytes). Padded to 112 bytes, the sealed name is 220 characters, which is under NAME_MAX.
7. **Recovery and the ordered index.** The ordered index is never fsynced (`ord_durable: false`, db.rs:4026). A plain store catches a torn one with the whole-file CRC at open (ordindex.rs:424-427). Under the cipher that check is skipped (ordindex.rs:413). The new design restores the check by the piece. A tag failure at or past a file's `trust_from` is an I/O error on that read. It is not a poison of the store (§4).
8. **The run's mapping stays resident while it is written.** `RunMap` maps the file raw and reads by pointer (db.rs:11465-11469). So a journaled file's view is write-allocated and held until its last mapping drops: a seal's worth of memory, as today. Every other file is not held in memory while it is written.
9. **No site passes file-mapping memory to a syscall.** `AlignedWriter::write` copies into its buffer (db.rs:6294-6315). The streamed run `writev`s the memtable's anonymous arena (db.rs:5726-5741, 6009-6017). The finish reads into heap vectors (db.rs:7753-7771). No call uses `sendfile` or `copy_file_range` (grep). The question only mattered for a userfaultfd SIGBUS pager, which stays rejected.
10. **The finish reads back through the layer.** The row, fence and directory are read with `read_exact_at` (db.rs:7753-7771) from pages sealed moments earlier. A write-behind ring of plaintext serves those reads (§3).
11. **Every sync writes a record.** That includes the helper's fdatasync every `ahead` bytes (db.rs:6218). One code path, at most three pages a sync.
12. **sync_dir is unchanged.** There are 9 sites (db.rs:11736, 11796, 12867, 13035, 27087, 27322, 27765, 37798, 38830) plus `open_dir` (db.rs:37738, synced at 36684). All act on the real directory, which now holds sealed names, so the kernel's semantics are the plain store's.

## 1. Seam and door surface

```rust
pub(crate) enum VFile { Os(File), Paged(Arc<pagefile::Handle>) }
// write, write_vectored, write_all_at, read_exact_at, sync_data, sync_all,
// try_clone, len, start_writeback(at,len), advise(at,len,Advice)
```

Each method is one `match` beside the syscall it wraps.

**Writers.** `vault::create` and `create_rw` return a `VFile`. `create_journaled(path)` declares "recovery reads this before it is finished". It has one caller: the run's temp, reached through `SegmentWriter::create_with` from `commit_direct` (db.rs:28698).

**Readers and mappings.**
- `vault::open` returns a `VFile` for reading.
- `vault::map` returns an `MmapBytes`: a plain mmap, or `MmapBytes::paged` over the pagefile (bytes.rs:365).
- `vault::open_mapped(path, cap)` returns a write-allocated, held view for `RunMap`.

**Directory.** `rename`, `hard_link`, `remove_file`, `names`, `exists`, `len`, `read`, `sync_dir` and `open_dir` all take logical names. Names that pass (`wal-`, `spare-`, `manifest*`, vault.rs:562) go to the real directory unchanged. The WAL, the spare and the manifest keep `VFile::Os`. All of `O_APPEND`, `set_len` and `seek` are the WAL's alone (db.rs:4261-4503, 27933; vault.rs:1914), so the pagefile needs none of them.

## 2. On-disk layout (pagefile v1; every kind except WAL, spare, manifest, marker)

The stored file is sparse.

| stored offset | content |
|---|---|
| 0 | header page: magic `SUPDBPF1`, version u16, class u8 (0 sealed-at-sync, 1 journaled), file_id[16], file key K_f wrapped under the store key (nonce12, ct32, tag16; AAD = the bytes before it), zeros |
| 4 KiB-60 KiB | record slot A (15 pages) |
| 64 KiB-120 KiB | record slot B |
| up to 1 MiB | hole |
| `E(e) = 1 MiB + e·257 MiB` | tag region of extent e (1 MiB): the tag page of group g is at `E + g·4096`, 256 tags of 16 B |
| `E(e)+1 MiB+(p&0xFFFF)·4096` | data page p, where e = p>>16 and g = (p>>8)&255 |

All address math is shifts and masks. Data overhead is 1/256 (0.39%). A small file costs about 3-5 written pages of overhead on disk: the header, one tag page and the records.

**Record.** It has a clear part: `seq u64`, `body_len u32`. Then a sealed body:
- `len`, `synced_len`
- the open group's index and tags (up to 256×16)
- the carried group's tags (journaled class only, up to 4 KiB)
- the tail plaintext (`len % 4096` bytes)
- patch runs: up to 64 × (first page u64, count u32, epoch u32, shadow u16)
- the shadow tags

After the body come the shadow ciphertexts, raw, at most 4 pages. The worst case is about 8 pages, and the slot holds 15.

## 3. Write path

The layer keeps a per-file `Handle`, keyed by file_id and shared by every name and clone. It holds `len`, `home` (the pages sealed home), the tail page, the open group's tags, the patch list, `synced_len`, the slot roles (S = last synced, U = the other), and a mutex for writers and fills. A file is written by one process exactly once: no store reopens a file to write it. Recovery rewrites a run into a new file (db.rs:27751-27762).

**Append.** Bytes go into the tail page. When the page fills, it is sealed into the call's staging buffer and its tag goes into the open group. When a group fills, its tag page is staged too. One write call issues one `pwritev` of the data. A 1 MiB piece is one aligned stored range, plus a 4 KiB tag-page write. Plaintext is not kept, except:
- (a) in pages the plaintext cache already holds, which are updated in place under the page's chunk state, as the page cache is under a plain mapping;
- (b) in a write-behind ring of the last 4 MiB, for read-back;
- (c) for a journaled file's held view.

**Patches** (`write_all_at` below `len`; only the finish makes them):
- Sealed-at-sync class: the page is rewritten home at epoch+1. Its 16-byte tag is rewritten in the tag region, and the epoch goes into the patch runs. This is safe because nothing names such a file until its final sync: a seal renames, publishes and fsyncs before its manifest, and the open sweeps what no manifest names.
- Journaled class: if the page is below `synced_len`, the new image is sealed at epoch+1 into a shadow and the home page is never rewritten. Shadows are copied forward, as ciphertext, into each later record. The run's finish makes 2 (page 0 and page 1). More than 4 is refused, and a test asserts the bound.

**Sync** (`sync_data`/`sync_all` on any handle, including the helper's dup, `close_presync`'s clone at db.rs:29034, and the form file's reopen at zfile.rs:4380): under the mutex, snapshot the state, seal the record and `pwrite` it into U; release the mutex; `fdatasync`; then U becomes S. A writer's drop writes a record without syncing if anything changed since the last one. That covers the unsynced ordered index.

**Journaled class, additionally.** Each write call that lands pages writes a record into U, unsynced, after the data. That gives process-crash parity for the whole pieces a buffered commit writes.

**Hints.** `sync_file_range` and fadvise are forwarded over stored ranges: the data range and its tag page. `start_writeback` (db.rs:6071) and zfile's drops (zfile.rs:3932-3940) need no change at the call sites.

**Read-back.** `read_exact_at` is served from the ring, then the cache, and otherwise from pread plus decrypt into the caller's buffer. The tail comes from the Handle.

**Commit sequence of a run.**
- Durable: records and marker → `stream_flush(true)` lands whole pages, and the tail stays in the Handle → record into U (tail, open tags, carried tags) → `fdatasync`.
- Buffered: the same without the sync. Only whole pieces land, so U is written once a MiB.

`RUN_LOGGED` and `log_run` (db.rs:28844-28881) go. The run is its own log again.

## 4. Read path and the plaintext cache

The pager is kept: epochs, Grace, rests, holds, `check`/`poison`. The chunk becomes 4096 bytes (pager.rs:108 `CHUNK`). A unit is 256 chunks, which is exactly 1 MiB, and the divide (`Per`) goes.

**Hit.** As today: one load, `== HOT`.

**Miss.**
- `pread` the stored page straight into the cache's memfd page.
- `pread` its 16-byte tag; for a unit fill, `pread` the tag page once.
- Decrypt in place under nonce (p, epoch). The epoch is 0 unless the patch runs, loaded at open, say otherwise.
- A miss on the tail page decrypts it from the record.

Nothing reads the ciphertext through a mapping any more. Today's copy out of the envelope mapping and its minor faults are gone.

**Memory.** The memory bound is the plaintext budget, as now. Files being written are no longer counted as memory that cannot be given back, except a journaled file's held view. So `could_give` and the unbacked-file lines (pager.rs:509) shrink to that one case.

**Eviction under borrows.** Unchanged: mark, bump the era, wait for `oldest_lent`, free by `MADV_REMOVE` in 1 MiB units.

**Trust.** At open a file gets `trust_from = synced_len`; for a file this process wrote, `trust_from = len`.
- A tag failure below `trust_from` poisons the store, as today.
- At or past it, the failure ends that read with `InvalidData`, and `slice_at` answers `None`. Then:
  - `vault::read` returns the longest prefix that verifies, which is what `recover_direct` walks;
  - `OrdIndex::open` checks its CRC by the piece through `read_at` and recomposes on any error, which is the plain path;
  - `ZFile` takes a failing form file out of service.

## 5. Crash consistency

| kind | power loss | process crash |
|---|---|---|
| segment and merge output (sealed-at-sync) | before the final record and its fdatasync, the file is unnamed by any manifest and is swept; after, it is whole | the page cache holds the data and the record; same as plain |
| direct run temp (journaled) | the newest valid slot wins. U torn: S, the last commit. U whole but data missing: pages past `synced_len` fail and the file ends there; the marker walk stops at its last good marker. Torn during the finish: S, with home pages 0 and 1 intact | the last U, written after its pieces; an unacknowledged commit may be lost, as in plain |
| ordered index | torn or record missing: open fails and the index is recomposed (db.rs:8503-8526) | same |
| form file | synced after rename (zpages_build_sync); otherwise the form is failed and rebuilt | same |
| WAL, spare, manifest slots, marker | unchanged: sealed records and slots, torn tail = the end; the marker is temp + rename (vault.rs:436-449) | unchanged |
| names | rename(2) of the old entry to a new sealed name at name_seq+1, then unlink of the target's entry. An open that finds two entries for one logical name keeps the higher seq | same |

No sector or page write is assumed atomic: every torn page fails its tag, and no acknowledged byte lives only in a page that is rewritten.

## 6. Cryptography

**Keys.**
- The marker wraps the store key K_s under the caller's key (unchanged; `rewrap` still writes 100 bytes).
- Each file has a random K_f wrapped under K_s. The wrap nonce is K_s's random prefix plus a counter; AAD `"pf"|file_id|class`.
- Names are under K_s with random 96-bit nonces, AAD `"name"`.

**Nonces under K_f.**
- data page and shadow: `p u64 ‖ epoch u32`
- record: `(1<<63 | seq) ‖ 0`

They are unique because one writer, in one process, draws each (page, epoch) once and each seq once. Shadows are copied forward as ciphertext and never re-sealed.

**What is authenticated.** Every data byte, the length, the tail, the epochs and the patch list (by the record). A file's identity is bound by the header's file_id and the name's file_id, and a swap is refused.

**What is not authenticated.**
- Rolling back the whole directory.
- Rolling one file back to its other slot. For a finished file that reads short or unfinished and is refused by its own trailer or superblock checks. For a run temp it is an earlier acknowledged commit.

**What leaks.** File count; sizes at page granularity; commit cadence; hard links (inode link counts); access patterns. Name lengths are hidden by the padding.

The crate still ships no cipher. Every operation is the caller's `Aead` (crypt.rs:141).

## 7. What goes, what stays

**Goes** (from the maps):
- vault.rs: about 1,300 of its 2,925 lines. That is memfd make/rewrite/reopen, `Kind`, tokens, `persist`/`persist_one`/`persist_form`/`companion`/`kept`, `adopt_all`, the sweep by table, the `.z` cascade, `Mapped`/`MappedFile::owed` beyond the run's held view, `Ahead`, and the token list in `seal_manifest`.
- db.rs: about 250 lines. `RUN_LOGGED`, `log_run`, `Direct::logged`, `cipher_controller`, 7 persist sites (12866, 13034, 11669, 28999, 29525, 12574, 27632) and the run counters.
- zfile.rs: about 150 lines (`SegSrc::Kept`, 4302-4310, 4373).
- pager.rs: about 100 lines (the `Per` math, the 4080 unit, unbacked files).
- ordindex.rs:413-428: the skip.

**New:** `src/db/pagefile.rs`, about 900 lines plus about 600 of tests.

**Stays:**
- the readers' explicit-ask path (`Paging`, `Pieces`, `Sect`, `scan_pieces`), about 2,800 lines;
- the pager's epochs and rests;
- `ATTN_PAGED`;
- `LogCipher` and the WAL's cipher branches (db.rs:4203-4580);
- manifest slot sealing;
- the marker and the decoy directory;
- crypt.rs, which is still used for published files.

## 8. Why a store in the clear pays nothing

The plain readers are untouched, and so is `MmapBytes`. `VFile::Os` adds one `match` per write, sync or hint syscall: once a 1 MiB piece, and never once a key or a block.

Plain work also disappears:
- the `RUN_LOGGED` test in `commit_direct`;
- each `vault::persist*` call, which is a load and a branch.

The check is to count these under callgrind by function, built at `origin/main` from this tree's example sources and at the tree: `read_only.rs supdb 100000 1 n-none`, `lag0_probe.rs supdb 100000 1`, `work_probe.rs 100000 whole`, `merge_walk.rs 100000 4 1`, `load_only.rs supdb 100000 1`, and the form's scan and point read (`supdb-nocachezpages`). The risk to watch is `AlignedWriter` holding a `VFile` and changing what is inlined in `write_buf`.

## 9. Predictions

1. **Out-of-cache point read: 0.17x → 0.20-0.24x.** The misses per read are unchanged (about 1.2), and each miss costs about 2.8 µs instead of 4. The rest needs batched fills or a larger budget.
2. **A miss: 4 µs → 2.5-3.2 µs.** The fresh memfd page stays at about 1.9 µs. The copy and fault through the envelope mapping are replaced by a 0.75 µs pread into the page plus a 0.4 µs tag pread. Decrypt is about 0.5 µs.
3. **Ordered load: 0.68x → 0.88-0.97x durable; 0.52-0.57x → 0.80-0.92x buffered.** Device bytes per byte stored go from 2.5 to 1.35-1.45. The log double write goes, and so do the drain's envelope (43-55 ms) and the 14 ms unlink of the run's filled log. What is added is about 5 ms of AEAD a 40 MB run, one 4-12 KiB record a commit, and the finish's read-back. Buffered loses more because its load is shorter.
4. **Compact form under a bound: 0.73x → 1.0-1.3x of the raw form under the cipher.** A form page is one stored page. A tag page covers 1 MiB, is touched 256 times as often as a data page, and stays resident. The controller can then run under the cipher.
5. **In-core point read: 339 instructions over 818, unchanged by stages 1-5** (the ask protocol is kept), minus the `Per` math. In-core loads go from 0.84-0.92x to about 0.95x, since the seal's 21 ms envelope step goes.

## 10. Risks

- **Complexity.** Record roles, the carried group, shadows and `trust_from` are new state machines. Each torn state needs a test that builds it.
- **Tag reads.** A second pread per miss. If tag pages do not stay resident out of core, a cold read costs two device pages.
- **Write-allocate.** Without it, the finish's read-back and the first pass after a landing decrypt from ciphertext.
- **Sparse holes.** About 2 MiB of holes per file. A filesystem without holes would allocate them.
- **Opening.** The open reads the directory and decrypts every name; about 1 µs a file is assumed and not measured.
- **Per-file rollback.** It is refused only by each format's own checks.
- **Folios.** The layer's 4 KiB tag-page writes and records may change folio composition beside the data.
- **The run's held view.** It keeps a seal's worth of plaintext resident, as today, until `RunMap` asks.
- **The class flag.** One new piece of semantics at the door. A future file that recovery reads while written must declare it.

## 11. Experiments owed before building

1. **Upper bound for the form:** plain `pair.sh supdb-autozpages supdb-nozpages 5000000 --bound 671088640 --passes read`. This is what an aligned page buys with no tag page.
2. **Plain cost of the door type:** a stage-1 callgrind of the probes in §8.
3. **Miss cost:** extend `seam/uffd.c` with "pread a 4 KiB page into a fresh memfd page, pread a 16 B tag, decrypt" against "copy out of an envelope mapping". The decrypt is a stand-in priced by `crypt_cost`. Run it with the machine to itself.
4. **Tag-page residency out of core:** `mincore` over the stored tag regions after the bounded read pass on a stage-3 build.
5. **The run as its own log:** stage-4 `pair.sh supdb-cipher supdb-cipherenv 300000 --passes load`, durable and buffered, with `SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1`, and `/sys/block/vda/stat` read around each load.
6. **The write-behind ring:** finish steps with the ring on and off.
7. **Name opening:** decrypting 10,000 names, and the rename-over crash window built in a test.
8. **Torn states,** each built by hand: U torn, U valid with data missing, a shadow, a tag page lost, an unsynced ordered index.

## 12. Staged plan (each stage leaves the gate green)

- **S1** `VFile` with only `Os`, threaded through every door caller. Callgrind proves the plain cost.
- **S2** `pagefile.rs` and its unit tests, with the torn states built by hand. Not wired.
- **S3** Kept files move to the pagefile: segments, ordered indexes and form files, sealed at sync. Names are sealed, the manifest carries no tokens, and the pager chunk is 4096. The old shape stays behind `Options::cipher_envelopes` (arm `supdb-cipherenv`) for pricing. `RUN_LOGGED` still covers runs. `tests/cipher.rs`, the fuzz with `ZFUZZ_CIPHER=1`, and mutants of the new paths.
- **S4** The journaled class: the run becomes its own log, `log_run` and `RUN_LOGGED` are deleted, and loads are paired.
- **S5** Bounded pairing of the form; then `cipher_controller` is deleted and the controller turned on.
- **S6** Delete the envelope arm, after its pairings are recorded in the PR.
- **S7, separate:** reads in core at the plain rate (a WARM whole-lend state per file), counted under callgrind.
