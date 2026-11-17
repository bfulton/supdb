# Transparent plaintext space over a page-granular cipher file ("cspace")

## 0. Verdict
A transparent mapping cannot be the only mechanism, for three reasons:
- **The handoff costs too much.** In the mode where a handler thread fills the page, which is the only mode in which a kernel-mode access blocks instead of returning EFAULT, a miss costs 30-45 µs here (probe.md). Today a miss costs 4 µs.
- **The fast mode breaks kernel access.** In the fast mode (`UFFD_FEATURE_SIGBUS`, where the faulting thread fills its own page, 4.5 µs), a syscall that touches a missing page gets EFAULT.
- **The default containers deny it.** The default seccomp profiles of Docker, containerd and Podman refuse `userfaultfd`. Docker/moby's `default.json` has no entry for it, so the default `SCMP_ACT_ERRNO` applies. containers/common's `seccomp.json` lists it under an explicit EPERM block. containerd's `contrib/seccomp` does not allow it. Kubernetes `RuntimeDefault` is the runtime's profile.

The design therefore has:
- **One plaintext space** with two interchangeable fill backends:
  - **M1**: userfaultfd in SIGBUS mode on a memfd, opened with `UFFD_USER_MODE_ONLY`, or through `/dev/userfaultfd` when that device is passed into a container.
  - **M2**: `PROT_NONE`/SIGSEGV plus a writable alias of the same memfd. It runs in any Linux container and is the portable shape.
- **A lend guard** that makes kernel access safe for values lent to callers.
- **A page-granular cipher file layer** below every file the engine writes.

Both backends are worth having. M2 alone would make a miss about 3x slower. M1 alone would fail on the default container. They share everything but about 300 lines.

## 1. The seam and what the engine sees
The cipher sits between the engine's finished bytes and the disk. It knows pages, lengths and names, and nothing about segments, logs, manifests or forms. The door (`vault::`) stays the only choke point. What changes is that it returns `vault::File` instead of `std::fs::File`:

```
struct File { os: std::fs::File, c: Option<Box<CFile>> }   // c is None for a plain store
write_at / write_all / write_vectored / read_at / set_len / sync_data / sync_all
writeback(at,len)  drop_cache(at,len)  len()
```
Every method is one `if let Some` beside the syscall. The door's other functions:
- `create`, `open`, `open_write`, `open_append` (the layer keeps the offset itself), `rename`, `hard_link`, `remove_file`, `names`, `len`, `exists` and `sync_dir`.
- Two new functions that are generic file operations, not forms:
  - `recycle(spare, path)`: a new life, with a new id and key and length 0. On a plain store it is rename plus `open_write`, which is today's `Wal::recycle` (db.rs:4280).
  - `preallocate(file, len)`: `fallocate` on the ciphertext. On a plain store it is today's zero loop (db.rs:4252).
- Mappings:
  - `map(path) -> MmapBytes`: plain is unchanged (bytes.rs:344). Under a cipher it returns a range of the space with its full lendable length, so `Blob::flat` lends whole and `zero_copy()` is true.
  - `map_growing(file, cap)`: for `RunMap` (db.rs:11461), replacing `open_mapped`/`Mapped`.
- Advice: `MmapBytes::{resident_at, advise_willneed, advise_dontneed}` branch on the mapping's kind, beside the madvise.
- Failures: `vault::check()` is per file, so the engine can tell a damaged form file from damage anywhere else.

## 2. On-disk layout (one layout, every kind)
The page size P is 4096, and the same layout serves every kind of file: segment, direct run, ordered index, form file, WAL, manifest slot and name table. The marker `supdb.cipher` is unchanged (vault.rs:362-449).

| offset | size | content |
|---|---|---|
| 0, 4096 | 1 page each | **H0/H1**, the life header. Magic `supdbCH1`, version, `life u64`, `file_id[16]`, the file key wrapped by the store key (nonce 12, ct 32, tag 16, AAD `"file"‖file_id`). Sealed under the store key with AAD `"H"‖slot`. Newest valid `life` wins. |
| 8192, 16384 | 2 pages each | **J0/J1**, the journal. Page 1: magic, a random nonce of 12, then a sealed body: `seq`, `plain_len`, `tail_page`, the tail's nonce and tag, `open_group`, `n ≤ 126` entries of the open group (28 B each), an optional carried patch page index with its nonce and tag, and the last completed group's meta seq. AAD `file_id‖"J"‖slot`. Page 2: the tail page's ciphertext. A carried patch page, if any, is a third page at +8192 of the slot's 12 KiB span, so slots are spaced 12 KiB apart when patches are on. Newest `seq` whose pages verify wins. |
| 32768 + g·512 KiB | 2 pages | **MA/MB**, the meta of group g. `supdbCM1`, `seq`, `g`, `count`, crc32c, then 126 entries of nonce 12 and tag 16. Not encrypted, because nonces and tags are public. A CRC catches tears and is checked at open only. |
| +8192 + i·4096 | 126 pages | data pages. Plaintext page `p = 126g+i` is stored as exactly one aligned ciphertext page: AEAD under the file key, nonce from meta or J, AAD `file_id‖p`. |

- **Overhead:** 32 KiB a file plus 1.6% of the data.
- **Address arithmetic:** `p/126` is a multiply and a shift. The handler keeps a bit a group saying which meta is newest, filled at open and on writes.
- **Nonces** are 96 random bits a page write. They come from a per-thread pool filled by `getrandom`. Every file life has its own key, and a life does far fewer than 2^32 page writes.

## 3. Write path
Write-once pages, meaning the data pages of a segment, an index or a form file, and the new full pages of a log or run:
- They are encrypted into a staging buffer of the size of the piece, with the meta of each completed group inline, so it is one contiguous `pwrite` per piece.
- After every `write()` the J slot not written last gets an 8 KiB `pwrite` with the length, the tail page and the open group's entries. That keeps what the kernel holds decryptable after a process crash, as a plain file's page cache is.

The tail page (the WAL and the direct run at every commit) is held as plaintext in the `CFile`. It lives only in J and goes to its home slot once it is full. Home is a fresh location, and the other J still holds its prefix, so a torn write cannot cost bytes that were committed before.

In-place patches:
- A write that changes exactly one page already made durable is carried in J until a sync after its home write. This covers the WAL header kind byte (db.rs:4502), a superblock page, and H on recycle. The page then reads as old or new and is never damaged.
- A write over several durable pages in place gets meta A/B and per page reads old, new, or unauthentic, reported as `InvalidData`. This covers the segment finish (db.rs:7728-7824) and the manifest slot (db.rs:11719).
- The segment finish's patches land before the file is named.
- The manifest is already A/B slot files that tolerate a garbage slot. Owed: confirming that its reader maps an `InvalidData` to "slot invalid".

Other operations:
- **Reading back unsynced bytes** (db.rs:7753, 6198) goes to `read_at`, which decrypts from the page cache or from the held tail.
- **`set_len`** re-seals the tail into J, and the ciphertext past the new group is truncated.
- **Rename and link** change only the name table.

The **name table** is two cipher files (`n0`, `n1`). It is appendable records `{op, token[16], name}`.
- A rename or link appends one record with a `write()`, so it survives a process crash.
- `sync_dir` runs fdatasync on the table and then fsync on the real directory.
- Compaction writes the other file under `life+1`.
- On disk the directory holds `x-<32hex>` files, `n0`, `n1` and the marker, and no other names, the WAL and manifest included.
- An unlink of a token file is deferred to after the sync that drops its last name, which also takes the inode round trip (1.2-20 ms) off the publish paths.

**Files mapped while they are written** (the run's `RunMap`) have a byte of state a page:
- The filler CASes EMPTY→FILLING, copies, and stores RESIDENT.
- The writer updates the held tail, then reads the state. If FILLING it spins. If RESIDENT it writes the new bytes through the memfd alias. Only then does it publish the length.
- This is Dekker on SeqCst. The bytes are append-only below `written`, the property `RunMap` already relies on (db.rs:11431-11435).

**Populate on write:** while the budget has room for the file, a fill thread decrypts each finished piece from the page cache into the space with batched `UFFDIO_COPY`, off the writer's path. Without it, the first scan after every seal would fault each page of the segment, where the memfd shape today has them resident.

## 4. Read path and the plaintext cache
- **The space** is one memfd (`MFD_CLOEXEC`) mapped once, `MAP_SHARED|MAP_NORESERVE`, `MADV_DONTFORK|MADV_DONTDUMP`, and registered once for MISSING. Files get 1 MiB-aligned ranges. An owner table indexed by `(addr-base)>>20` names the file.
- **A hit is a bare load.** No ask, no `ensure`, no `Sect`/`Pieces`, no rests.
- **M1 miss:** SIGBUS on the faulting thread, which then:
  1. Looks up the owner.
  2. Reads the meta entry: one load from a page kept in the cache, which the newest-meta bit selects.
  3. `pread`s the 4 KiB ciphertext page into a thread-local scratch buffer. `pread` is async-signal-safe, and no nested SIGBUS can occur.
  4. Calls `Aead::decrypt` in place (crypt.rs:150). It does not allocate.
  5. Does `UFFDIO_COPY` with DONTWAKE. EEXIST means another thread filled the page first.
  6. Returns.
  - Under SEQUENTIAL or WILLNEED advice it fills 16-64 pages a fault, or queues them to the fill thread.
  - A failed tag fills zeros and latches the file's poison. The read's end, behind `ATTN_PAGED` (db.rs:15624), replaces the answer, as `end_slow` does today.
- **M2 miss:** SIGSEGV with `SEGV_ACCERR`. The thread decrypts a 64 KiB extent through the RW alias and then calls `mprotect(PROT_READ)`. The extent size is `max(64 KiB, budget/(max_map_count/4))`, so the VMA count stays bounded.
- **Bounding memory:** `fstat(memfd).st_blocks` is the exact resident size. The evictor wakes on fills. A filler past 1.5x the budget does a futex wait for the evictor, which takes no engine lock, so the waits are bounded and there is no pin ring.
- **Hot and cold** are told apart with an emulated accessed bit, CLOCK over 256 KiB extents:
  - Hand A zaps PTEs (`MADV_DONTNEED` on the reader mapping) a window ahead. The memfd keeps the pages, and a touch refaults as an ordinary shmem minor fault, which costs no uffd and no decrypt and is safe from the kernel too.
  - Hand B, a window behind, reads `/proc/self/pagemap` present bits (or `PAGEMAP_SCAN`). Any page present means the extent was referenced. All absent means it is punched with `fallocate(PUNCH_HOLE)`, contiguous extents in one call, as today's megabyte give-back does.
- **User-mode safety needs no grace.** Addresses never move, and plaintext is a function of immutable ciphertext, so a page punched under a reader refaults to the same bytes.
- **Kernel-mode safety (EFAULT).** Engine syscalls never take space slices, because all store writes go through `CFile`, which reads its source in user mode. A caller's syscall on a lent value is guarded:
  - The API entry (`read_all` db.rs:20219, `scan` 20586, cursor, snapshot) branches once on the existing `ATTN_PAGED` bit to a guarded callback.
  - The guarded callback, for a slice inside the space and outside the slot's current hazard, stores an extent-aligned hazard in the reader's own slot line with SeqCst. It then loads the extent state, waits if EVICTING, and touches each page of the slice.
  - The evictor marks EVICTING, reads every slot's hazard, and spares the extents a hazard covers. The hazard clears at the operation's end.
  - Its cost is a compare a lend and a store per 256 KiB crossed.
- **Signal handler:**
  - It is installed once with `SA_SIGINFO` and without `SA_ONSTACK`, so it runs on the thread's own stack.
  - It chains to the previous handler for foreign addresses. That previous handler is Rust std's guard-page handler, or the default.
  - It saves errno and keeps a thread-local reentry flag. A fault in the space while inside the cipher aborts with a message.
  - It wraps the cipher in `catch_unwind`. A panic poisons.
  - It delegates to the fill thread (the 30 µs path) when the thread is on an alternate stack or has less than 32 KiB of stack left.
- **The hook contract:** `decrypt` must not allocate, lock or block.

## 5. Crash consistency

Two definitions:
- **Process crash:** every `write()` leaves H, J and the meta in the kernel, so a new process decrypts exactly the bytes written. This equals plain for every kind, the buffered policy included. The space is a cache and dies with the process.
- **Power loss:** what an fsync covers is durable, and the newest valid H and J select the state.

| kind | power loss | vs plain |
|---|---|---|
| segment and form file | temp, rename (table record), fsync, `sync_dir`, then the manifest. A table record that is not durable means the file is swept at open. | equal |
| direct run | records, marker, J, fdatasync. A torn J falls back to the other, which is the state as of the previous commit, and the marker walk is unchanged. The run is its own log again. | equal |
| WAL | the same. A torn tail is the log's end. The header byte is carried in J. Recycle is a new H life, and a stale J (wrong life) reads as an empty log. | equal; the `recycles` exception goes |
| manifest slot | multi-page in place: old, new, or unreadable means the slot is invalid and the other slot is the store | equal, given the open's mapping is confirmed |
| ordered index | an unreadable file is "torn" and is rebuilt | equal |
| name table | append plus J, fdatasync at `sync_dir` | equal |

Contract C3, a write over several durable pages in place, is weaker than plain's sector mix, and is owed an audit of every such site.

## 6. Cryptography
- **Keys:** caller master → store key (marker) → per-file-life key (in H) → pages with random nonces. `rewrap` is unchanged.
- **Uniqueness under every rewrite:** a nonce is fresh for every page write, every J and every H, and a new life gets a new key. Nonces cannot repeat across a crash because none is derived from a counter.
- **Authenticated:** every data page, bound to its file and index; J (which holds the length, so a truncation is refused); H; the name table.
- **Not authenticated:**
  - The whole directory restored to an earlier state.
  - A file rolled back to an older J or meta version, which is an earlier state of that file. This is the same class as today's (vault.rs:109-111).
- **Leaked:** the file count, sizes to the page, which pages are rewritten when (commit sizes, patch sites), and timing. Names are no longer leaked: today `wal-`, `manifest` and `spare-` pass to the directory in the clear.
- **Plaintext exposure:** plaintext can reach swap, as today. It is excluded from core dumps by `MADV_DONTDUMP`, and a child process never sees it because of `MADV_DONTFORK`.

## 7. What goes, what stays
Deleted, roughly 5,300 lines:
- `pager.rs`: about 1,600 lines plus 530 of tests.
- `bytes.rs`: `Paging`, the paged kind and `piecewise` (about 150).
- `flatindex.rs`: `Sect` and `Pieces` (about 200), and the 23 functions go back to slices. Their only other user was the cipher; checked that `crypt.rs` does not use them.
- `blob.rs`: the piecewise `Sec` arms, `indexed!`'s second arm, `scan_pieces`, `scan_either`, `scan_refused`, `by_the_piece` and `not_lent` (about 250).
- `db.rs` pager sites (about 600): `PagerGrace`, `Slot.page`, `unlent`/`unlend`, `build_rest`, `REST_HERE`, `AHEAD_REST`, `look_mask`, `SCAN_REST`, `MergeRest`, `page_snap`, `paged_begin`/`swamped`/`breathe`.
- `vault.rs` (about 1,800 of 2,925): `Kind`, `companion`, `persist*`, `Mapped`/`MappedFile`, `seal_manifest`/`open_manifest`/`adopt_all`, `LogCipher`, the sweep's form rules, the memfd table and `passes()`.
- The WAL's cipher branches (db.rs:4134-4580, about 150).
- `RUN_LOGGED`/`log_run` (about 100).
- `cipher_controller` (db.rs:11529).
- zfile's `SegSrc::Kept`/`Walked::Held` (about 200).

Added, roughly 3,400 lines:
- `cfile.rs`, about 1,400.
- `space.rs`, about 1,300: M1, M2, the evictor, the guard and the handler.
- The name table, about 350.
- `vault::File`, about 250.
- The tests.

Kept: the marker and key hierarchy, the `StoreCipher` API, `crypt.rs` (frozen), `ATTN_PAGED` and the end-of-operation poison check, and `tests/cipher.rs`'s properties together with the `ZFUZZ_CIPHER` run.

## 8. Why a store in the clear pays nothing
- **Reads:** a plain mapping is the `MmapBytes` it is today, so the readers are today's slice code with the piecewise arms deleted. That is fewer instructions, and it reverses `blob.rs`'s shapes.
- **Operation start and end:** `ATTN_PAGED` is already in the start and end tests every read makes (CLAUDE.md, "the question rides a test").
- **File operations:** every `vault::File` method is a branch beside a syscall.
- **The lend guard** is selected at the API entry by that same byte. Its cost on the plain instantiation is an open question, counted under experiment E4 by function against `origin/main`.
- **Writes:** no write path of the engine changes for a plain store. `prefill` and `recycle` keep their bodies behind the door.

## 9. Predictions
1. **Out-of-cache point read, 0.17x → 0.15-0.24x, neither a loss nor a win.**
   - An M1 miss is about 4.5 µs measured, plus about 0.75 µs of `pread` and about 0.4 µs of decrypt, so 5-5.5 µs against 4.
   - Against that: one ciphertext page a plaintext page (today a 4096-byte record page spans two 4080-byte chunks), a hit at a load instead of about 340 instructions, and finer eviction.
   - The CLOCK tax works the other way: a hot page takes one shmem minor fault a revolution.
   - Under M2 it would be about 0.07-0.1x.
2. **A miss, 4 µs → 5-5.5 µs single-page under M1** (about 1.5-2 µs a page in sequential fills); **10-15 µs under M2.** The fresh-page fault becomes the COPY's allocation, with the signal added.
3. **Ordered load, 0.68x / 0.52-0.57x → 0.85-0.95x / 0.8-0.9x; device bytes per byte stored 2.5 → about 1.35.** The double log write goes, as do the envelope pass (43-55 ms of the drain) and the log `unlink` (14 ms). What is added is encryption inline, about 5 ms a 40 MB at 7.9 GB/s, an 8 KiB J a commit, and 1.6% meta.
4. **Compact form under a bound, 0.73x → 0.95-1.05x of the raw form under a cipher.** A form page is exactly one ciphertext page, so the controller can stay on.
5. **In-core point read, +339 instructions over 818 → +10-40; scan mix 0.78-0.87x → 0.93-1.0x.** What is left is the guard's compare a lent slice and the end-of-operation check. The first scan after a seal depends on populate-on-write.

## 10. Risks
- The caller's cipher runs in a signal handler, so the contract is no allocation, no locks, no panic and limited stack. A lock shared with the lending thread deadlocks.
- Foreign runtimes' handlers (JVM, Go) with SIGBUS/SIGSEGV chaining. Go requires `SA_ONSTACK`.
- A caller keeping a lent pointer past the callback gets EFAULT or a refault instead of stale bytes. That is UB in Rust anyway, but C-ABI callers can do it.
- M2: `vm.max_map_count` (65,530 by default), `mprotect` taking `mmap_lock` for write and contending with every thread's faults, and fills of 64 KiB extents at random reads.
- The CLOCK tax, the cost of `pagemap` reads, and TLB shootdowns at 18-88 µs a call with threads running.
- C3 is weaker than plain for in-place multi-page rewrites of durable data, and the audit could miss a site.
- 8 KiB a commit against 4 KiB for tiny durable commits.
- Ciphertext folios are no longer piece-aligned, because the meta pages are inline.
- Populate-on-write doubles fresh pages in core, as today's memfd does.
- ptrace or a debugger reading a punched page gets EIO.

## 11. Experiments owed, before building
- **E1:** `uffd.c sigbus` with a real fill: a `pread` from a cached ciphertext file, AES-GCM decrypt, then COPY, at 1 and 3 threads, and 16-page fills. Run on a quiet machine. This settles predictions 1 and 2.
- **E2:** the cost of a shmem minor refault of a zapped page with MISSING-only uffd registered, both kernel-mode (`write(pipe)`) and user-mode; `pagemap` against `PAGEMAP_SCAN` per MiB; SIGBUS mode combined with that refault.
- **E3:** the availability matrix: `uffd avail` under `docker run` (default, `--device /dev/userfaultfd`, seccomp unconfined), podman, and k8s `RuntimeDefault`. Also M2 at 16K extents against `max_map_count`, and `mprotect` beside three faulting threads.
- **E4:** a stub API-entry guard dispatch, with callgrind by function on `read_only.rs supdb 100000 1 n-none`, `lag0_probe.rs`, `work_probe.rs` and `merge_walk.rs` against `origin/main`.
- **E5:** a caller `send()`s a lent value while the evictor punches, with and without the guard; Rust-std handler chaining; fork.
- **E6:** a `cfile` micro-writer: 40 MB in 1 MiB pieces, and WAL-shaped 100 B and 100 KB commits with fdatasync, reading `/sys/block/vda/stat` beside it.
- **E7:** an audit by grep of every `rename`/`hard_link` followed by `sync_dir` before a manifest names the file, and of every multi-page in-place write.

## 12. Stages, each leaving the gate green
- **S0:** E1-E3 and E7. If E1 reads an M1 miss above 6 µs, stop at S3 and keep the pager, now reading page-aligned cfiles.
- **S1:** `vault::File`. Plain is a passthrough, counted by E4's probes. The cipher is unchanged behind it.
- **S2:** `cfile` and the name table for the WAL, manifest and spare. `LogCipher` and the sealed slots go, and `tests/cipher.rs`'s crash properties run over the layer.
- **S3:** segments, indexes, form files and the run as cfiles. `Kind`, `persist*`, tokens and `RUN_LOGGED` go. The pager reads cfile pages, one chunk a page and no straddle. Price `pair.sh supdb-cipher supdb 300000 --passes load` and the bounded form pairing.
- **S4:** the space (M1 and M2) as an arm beside the pager (`Options::cipher_reads`), with the guard. Price `pair.sh supdb-cipher64 supdb-cipher 1000000 --reps 4`, in-core pairings, and the fuzz under a cache of 16 KiB with both backends.
- **S5:** if S4 wins, the space becomes the default. Delete the pager, `Pieces`, `Sect` and the rests, recount the plain store, and turn the compact-form controller on under a cipher once priced.
