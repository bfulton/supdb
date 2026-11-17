# PageFS: one page-granular encrypting file layer under every format

(Condensed version, about 2,500 words. The full version with every derivation is env-full.md beside this file.)

## 0. Verdict
The owner's seam taken strictly pays for writes and for names:
- it removes all of the vault's knowledge of the forms;
- it removes the double write;
- it fixes the compact form's 0.73x, because data is held in 4096-byte plaintext pages with the tags kept apart.

It stops paying in three places:
1. A mutable file needs a small per-file shadow journal. With an AEAD and a fresh nonce, a page rewritten in place and torn is a lost page, which a plain page cache never does.
2. The readers' explicit-ask protocol stays. Nothing below the formats makes zero-copy lending safe alongside eviction on this machine (section 4).
3. The in-core cost is bought back by a per-file WARM state, not by the seam.

## 1. Seam and door
- **Where it sits.** Below every formatter, above the kernel. The engine still calls only `vault::` (vault.rs:1860-2229).
- **What changes.** Today the door hands back a raw `File`, and the bytes go straight to the kernel (writepath.md section 2). It now returns `enum VFile { Os(File), Enc(Arc<pagefs::Handle>) }`. Its methods are `write_all`, `writev`, `write_all_at`, `read_exact_at`, `read_to_end`, `seek`, `set_len`, `sync_data`, `sync_all`, `start_writeback`, `advise`, `len` and `try_clone`. Each is one match beside the syscall it wraps.
- **Mappings.** `map` returns an `MmapBytes`: either a plain mmap, or the layer file's plaintext view (`MmapBytes::paged`, bytes.rs:365). `open_mapped(path, cap)` returns a growable view at a fixed base, for RunMap (db.rs:11454, 28741).
- **Directory operations.** `rename`, `hard_link`, `remove_file`, `names`, `len`, `exists`, `sync_dir` and `read` work on logical names.
- **One new semantic.** `create_regenerable`, used by `write_ord` (db.rs:99-131) and the zfile sink (zfile.rs:4253): "a failed page of this file is this file's damage, not the store's". It is a durability class, not a format.
- **What goes.** `persist`, `persist_one`, `persist_form`, `companion`, `kept`, `Kind`, `Mapped` and `MappedFile`.

## 2. Layout (one format for every kind)
The stored file uses 4 KiB pages, and plaintext page p is exactly one stored page.

| stored page | contents |
|---|---|
| 0 and 1 | Header copies A and B: magic `SUPDBPF\0`, version, shift=12, class, file_id[16], open_count u32, hdr_seq u32, and the file key wrapped under the store key (nonce12+ct32+tag16, with the AAD covering every header field) |
| 2-6 | Shadow slot A: one meta page and up to 4 data pages |
| 7-11 | Shadow slot B, the same shape |
| 12+129g | Group g: a tag page (128 entries of 32 B), then data pages for p = 128g+i |

- **Address of page p.** `stored(p) = 12 + 129*(p>>7) + 1 + (p&127)`: a shift, a mask and an add, no divide.
- **Tag entry.** `nonce12 | tag16 | len u16 (0 = absent) | flags (DATA, ZERO) | pad`.
- **Page AAD.** `file_id | p | len | flags`. Only the valid bytes are encrypted.
- **ZERO pages.** A ZERO page carries a tag over empty plaintext. Its stored bytes are real zeros, so its blocks are allocated, which is what prefill is for (db.rs:4236-4262).
- **Slot meta.** Sealed under the file key: `seq u64 | logical_len u64 | n | n×(p, len, nonce, tag)`.
- **Overhead.** 0.78%, plus 48 KiB of prefix that stays a sparse hole until it is used.
- **What uses it.** Segments, direct runs, `.o`, `.z`, the WAL and spare, and both manifest slots. Their plaintext is byte for byte the plain store's: the superblock at 0..4096 (format.rs:43), the records from `SUPER+HEADER`.
- **Marker `supdb.cipher`.** This is the layer's root and is not in page format: two 512 B slots at 0 and 4096. Each holds store_id, gen, a `clean` byte, key_id, and the store key wrapped under the caller's master.
- **Names.** A stored name is `x` + base64url(`nonce12 | AEAD_store(name padded to 112 | file_id | name_seq) | tag16`), 220 characters. The longest logical name is 101 characters (db.rs:12711).
  - The directory is the name table. An open decrypts the listing.
  - `rename` is rename(2) to a newly sealed name with the same file_id, and `hard_link` is link(2).
  - A header whose file_id does not match its name is refused.
  - A rename over an existing name is rename(2) followed by an unlink. An open that finds a logical name twice keeps the higher `name_seq`.
  - The durability of names is exactly the plain store's.

## 3. Write path
**One file object.** There is one `pagefs::File` per file_id, shared across names and handles: `chain` writes through a second handle (db.rs:4493-4502), and a run is hard-linked (db.rs:28989). It holds the logical length, the page states it shares with the cache, the tag pages, the shadow state and a writer mutex.

**Write-through.** Every write call reaches the stored file before it returns. Under a buffered policy, a plain store's writes survive a process crash in the page cache, and the layer's own cache would not.
- Full pages are sealed from the caller's buffer and written home with their tag page. A whole group, the tag page plus 128 pages (516 KiB), goes as one pwritev.
- A partial page is sealed over its valid bytes only.

**Tail rule.** A page goes home only if it is full, never written before, or replaced whole. A partial rewrite of an existing page goes to the next shadow slot instead. That covers:
- the tail extended at each WAL commit (db.rs:4436-4459) and at each direct-run commit (db.rs:6917-6936);
- the chain byte;
- the finish's section header at `key_off` (db.rs:7728), whose page holds records.

**Ping-pong.** Before slot X is written, X's unsuperseded pages are written home; their durable copy is in the other slot. The slot itself is one pwritev, so a process crash cannot tear it. Shadowed pages stay cached until they are home. More than four such pages in one call are handled in rounds, each with an fdatasync, and counted.

**Each pattern:**
- O_APPEND: the layer keeps the offset.
- `set_len`: the cut page goes through a slot and the later entries are marked absent.
- Prefill: written as ZERO pages, with no 32 MiB cipher pass.
- Recycle: a rename plus a header rewrite, logically the plain spare.
- Segment finish: page 0 (db.rs:7824) and whole reserve pages go home in place, partial pages go through slots.
- Read-back of unsynced bytes (db.rs:6198, 7753): served from the cache or the stored file.
- `sync_file_range` and fadvise: applied 1:1 to the stored ranges.
- `sync_data` and `sync_all`: passed through. An fsync is real again, and the list of four fsyncs a store under a cipher has to get another way goes.

**Cache placement.** A file with a live mapping is written through the plaintext cache. A write-once file is written around the cache, unless the cache can hold the whole file, in which case it is born WARM.

**Nonces.** A page nonce is `open_count u32 | rand u32 | ctr u32`.
- `open_count` is bumped once per process per writable file, in the A/B header, and rides the next fdatasync.
- A bump lost to a crash repeats a nonce only if the random 32 bits also collide: probability 2^-32 per lost bump.
- One open can seal 2^32 pages, which is 16 TiB.
- Seals under the store key use random 96-bit nonces: key wraps, headers and names. Slot metas take nonces from the file's counter space.

## 4. Read path and plaintext cache
The pager keeps its protocol (pager.rs): the states, the mark/era/grace cycle, `Slot.page`/`oldest_lent`, `Hold`, and the rests. Five things change.

- **A chunk is a page.** `CHUNK` goes from 4080 to 4096 (pager.rs:109). The unit is 256 pages, now kept for TLB shootdowns rather than for straddling. The probe put a 1 MiB give-back at 57-88 µs against 19 µs for a single page.
- **The cache is anonymous** `MAP_NORESERVE` memory instead of a memfd. A first-touch write costs 1.5 µs against 1.9.
- **A hit** is still one load and a compare per page (pager.rs:1312).
- **A miss:**
  1. CAS from EMPTY to BUSY, then rescue.
  2. `pread` the stored page into the plaintext page; the fresh-page fault is taken in the kernel.
  3. Take the tag from the file's tag-page cache.
  4. Decrypt in place (`Aead` works only in place, crypt.rs:141-151), then mark HOT.
- **Batching.** Under sequential advice, up to 16 EMPTY pages are made present with `MADV_POPULATE_WRITE` and filled with one `preadv`.

**Bound.** Asked pages, WARM files, shadowed pages and tag pages all count against the budget. Only shadowed pages are dirty, so the evictor never writes back.

**WARM.** An immutable file that fits in the budget's headroom is decrypted whole, in the background or at its birth, flagged warm, and never evicted.
- `lend_at`'s refusal branch (bytes.rs:481) calls a cold `lend_slow` that lends the whole file when it is warm.
- `Sec::get` (blob.rs:504) then lends whole and `Blob::flat` succeeds, so point reads and scans run the plain loops. This skips the 95-instruction `not_lent` path and every ask.
- **Demotion:**
  1. Clear the flag.
  2. Bump the era and wait out the grace on `oldest_lent`. Borrows are scoped to one operation, and snapshot handles are idle between their reads (`page_snap`).
  3. The file's pages then join the clock.

**Rejected: userfaultfd** (probe.md). Handler-thread mode costs 30-45 µs a miss on this machine. SIGBUS mode costs 4.5 µs, but a kernel access to a missing page gets EFAULT, so a caller's `write(2)` of a value it was lent would fail at random after an eviction.

## 5. Crash consistency
- **Segment.** The plain order holds: temp file, fsync, rename, directory fsync, manifest. The engine sweeps by logical name (db.rs:27632). Pages are patched before the segment is named.
- **Direct run.** A commit is durable when its full pages are home and its tail is in slot k. A torn home page or a torn slot falls back to the other slot, which holds commit k-1's tail. `recover_direct` (db.rs:6948) reads the same logical bytes it reads for a plain store. For a process crash, every write call has already reached the kernel.
- **WAL.** The plain rules hold, loose and chained alike. A torn frame is the log's end.
- **Manifest slot.** A whole-slot rewrite happens in place. A page that fails its tag reads as zeros through `read`, the CRC then fails, and the slot is ignored. The code at db.rs:11893-11920 is unchanged.
- **`.o` and `.z`.** These are unsynced by design, so a tear is possible. The marker's `clean` byte is cleared at a clean close, after every unsynced file has been fdatasynced. An unclean open verifies the regenerable files before use:
  - a failed `.o` makes `OrdIndex::open` (ordindex.rs:403) return `Err` and be rebuilt (db.rs:8503-8524);
  - a failed `.z` is taken out of service (a ROADMAP item).
- **Marker.** Written A/B.
- **Tag failures.** Through a mapping, a failed tag poisons the store (`Pager::check`). Through `read`, it reads as zeros and is counted.
- **Recovering page p.** The latest slot by seq whose meta verifies wins if it lists p; otherwise the home copy wins. The length is the larger of the last valid home extent and that slot's `logical_len`. Whatever subset of one call's writes lands, the file reads as an earlier prefix or the new state.

## 6. Cryptography
- **Keys.** The caller's master wraps the store key. The store key wraps a random key per file, kept in that file's header. The file key seals the file's pages and slot metas.
- **Authenticated.** Every page's bytes and length, its position, and its file binding (file_id in both the header and the name).
- **Not authenticated:**
  - rolling the whole directory back;
  - rolling one page of a mutable file back to an earlier valid version (the same class as a log cut);
  - a file's extent beyond its pages;
  - splicing versions across the pages of one multi-page mutable record. This one is weaker than today's single-AEAD manifest slot: only the format's CRC32C catches it.
- **Leaked.** The number of files, their sizes to the page, which pages are rewritten and when, and when renames happen.
- **Hidden.** Names, fences, keys and values.

## 7. Goes and stays
These are estimates; not verified line by line.

**Goes:**
- vault.rs, about 1,700 of its roughly 2,500 non-test lines: memfd/envelope/`Kind`/persist*/companion/adopt_all/the sweep's companion rules/seal_manifest/LogCipher/`Ahead`/`Mapped`.
- db.rs, about 450: the WAL's cipher branches (4203-4580), `RUN_LOGGED`/`log_run` (28219-28862), the cipher halves of the manifest image and open (11661-11685), the persist sites (12574, 12865, 13033, 28999, 29525), `cipher_controller` (11529) and `zpages_under_cipher`.
- zfile.rs, about 300: `SegSrc::Kept` and the held build (4204, 4302-4373).

**Stays:**
- The marker and keys, the door and the decoy, about 700.
- pager.rs, reshaped from about 2,100 to about 1,900.
- The ask protocol and the rest/pin sites, about 2,800 (readpath.md section 4).

**New:**
- pagefs.rs, about 1,800.
- Tests and a crash model, about 1,200.
- `VFile` at the call sites, about 200.

## 8. A store in the clear pays nothing
- **The door.** Its free functions are unchanged: a load of a counter, then the std call.
- **`VFile::Os`.** One discriminant branch per syscall wrapper: a writev per 1 MiB piece, a WAL write, a pwrite or a sync.
- **Per-key, per-entry, per-block and per-step code.** None changes, and some gets smaller: the commit's `RUN_LOGGED` bit goes, and so does the build's `SegSrc::Kept` choice.
- **The one read-side touch.** `lend_at`'s refusal branch goes from `None` to a cold call. If counting shows it moves the plain path, the fallback is to republish an `MmapBytes` with `lend=len` when a file goes warm, which leaves the plain read path untouched.
- **How it is held.** Callgrind by function against `origin/main`, on `read_only.rs supdb 100000 1 n-none`, `lag0_probe.rs supdb 100000 1`, `work_probe.rs 100000 whole`, `merge_walk.rs 100000 4 1`, `load_only.rs supdb 100000 1`, and the rows through the form.

## 9. Reasoning for the predictions
- **Miss.** Today's 4 µs is over half the memfd fault, a fifth the copy and a tenth the cipher. The new miss is a pread into the page (0.75 µs including the copy), an anonymous page faulted in the kernel (1-1.5 µs, about 0.5 in bulk) and the cipher (0.4 µs), with no `Per` or unit arithmetic.
- **Out-of-cache point read.** A record no longer straddles two chunks, and the hash slots and tag pages stay resident, so a read is about one miss at about half the cost.
- **Load.** Gone: the log frames, the envelope at the close (43-55 ms of a 77-94 ms drain) and the 14 ms unlink. Added: AEAD at about 7.9 GB/s (5 ms per 40 MB), the run's plaintext pages, 0.8% for tag pages, and 8 KiB of slot per commit.
- **Compact form.** A form page is one stored page plus a hot tag page.
- **In core.** WARM lends whole, so point reads and scans run the plain loops. What is left is a cold call per lookup and the read's start and end out of line.

## 10. Staged plan (each stage leaves the gate green)
- **S0.** Run E1-E4, and take the plain callgrind baselines.
- **S1.** `VFile` with only `Os`; memfds are `Os` too. Gate: every callgrind row equal.
- **S2.** pagefs as a module (format, writes, shadow, recovery, names), with unit tests, a crash model and mutants. Not wired in.
- **S3.** Wire it in under `Encryption::Cipher` behind the arm `Options::cipher_pages` (`supdb-cipherpages`), with `log_run` off under it. Run the fuzz under `ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384` and `ZFUZZ_RECYCLE=1`, and `tests/cipher.rs`, on both arms.
- **S4.** Reshape the pager (chunk = page, fill from pagefs, batching), then add WARM. Re-run the rescue interleaving test and the guard mode.
- **S5.** Pair the five cells, flip the default, delete the old shape, and turn the controller on under the cipher.
- **S6.** A pagefs mutant pass (nonce, slot choice, name_seq, zero-on-failure, clean flag), plus the leak and resident tests.
