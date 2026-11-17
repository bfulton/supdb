# Minimal: the page-aligned streaming envelope

Keep today's vault (the door, tokens, the sealed manifest, the sealed log) and today's pager (address space a file, chunk states, units, epoch grace, rests). Change only what the measurements blame:

1. The chunk is the page. A stored data page holds 4096 plaintext bytes; the tags live in a tag page per 128 data pages.
2. The envelope is written as the file is written. Nothing binds the length, and the length lives in a tail slot that says where the file ends.
3. With (2), an ordered run is its own log again (no WAL frames), and `fsync` of a kept file means something, so the `persist*` calls go.
4. A miss costs less: one fill per page instead of up to two, ranges populated in bulk. A cheaper backing is an experiment, not a commitment.

## 1. The seam and the door surface

The cipher layer sits in `vault` at the file handle. It sees byte offsets, lengths, syncs and names, and nothing about records. The door already routes every open (vault.rs:1860-2229). What it lacks is a choke point for bytes: today it returns a raw `std::fs::File` (writepath.md §2). The change:

- **`vault::File`** is `enum { Plain(std::fs::File), Kept(Arc<KeptFile>) }`. Every method is `#[inline] match` plus the syscall: `write_at`, `write_vectored` (with a cursor for the append writers), `read_at`, `sync_data`, `sync_all`, `writeback(range)` (today's raw `sync_file_range`, db.rs:6072-6085), `advise(range, kind)` (fadvise/willneed/dontneed, zfile.rs:3932-3940), `len`, `try_clone` (the helper's dup, db.rs:6091-6130). `set_len` shrinking a kept file is `Unsupported`, and no kept caller does it: the WAL passes through, and replay's `set_len` is on the WAL (db.rs:27933).
- **`vault::create_rw(path, Contract)`**. `Contract` is I/O semantics, not form. The plain arm drops it.
  - `UntilNamed` (default): a crash before the file is named leaves a file nobody reads. In-place rewrites are resealed unjournaled. Used by seals, merge pieces and form files.
  - `ReadAfterCrash`: recovery reads the file after a crash, so rewrites of synced pages are journaled. Used by the direct run (its temp name is what recovery reads, db.rs:6943-6949).
  - `Regenerable`: the file may be named while unsynced, and a torn file must read as torn, not poison the store. Used by the ordered index (db.rs:99-131: named unsynced, rebuilt when torn).
- **Unchanged:** `vault::map` (→ `MmapBytes::paged`), `vault::open_mapped` (the `Mapped` hold for `RunMap`, db.rs:11461-11470), and `rename`, `hard_link`, `remove_file`, `names`, `exists`, `len`, `sync_dir`, `open_dir`, `hold`, `check`.
- **Added:** `vault::read_recovering(path)`, the lenient length rule of §5. `recover_direct` (db.rs:6948) uses it.
- **Deleted:** `persist` and `persist_one` (vault.rs:2118, 2129), `persist_form` (2146), and the write-once refusal in `rewrite` (1090-1102).

Call sites that change type from `File` to `vault::File` (not verified exhaustively):
- `AlignedWriter.file` (db.rs:5472) and `WriterHelper`
- the finish's `write_all_at` / `read_exact_at` (db.rs:7728-7824)
- `zfile::FileSink` (zfile.rs:3852)
- `write_ord_tmp` (db.rs:125)

The WAL and the manifest slot keep `std::fs::File`, because they pass through (vault.rs:562).

## 2. On-disk layout (live envelope v2; `crypt` v1 untouched)

All offsets are into the stored file. P = 4096.

**Page 0, header:**

| offset | size | field |
|---|---|---|
| 0 | 8 | magic `supdbLE\0` |
| 8 | 4 | version 2 |
| 12 | 4 | flags: bit 0 journal, bit 1 regenerable |
| 16 | 16 | file id (random) |
| 32 | 8 | key_id |
| 40 | 8 | boot id hash (`Regenerable`) |
| 48 | 60 | wrapped file key: nonce 12, ciphertext 32, tag 16 |
| 108 | 2+n+28 | creation name under the file key: length, nonce, ciphertext, tag |

- The wrap's AAD is `"lfile" ‖ token ‖ bytes 0..48`.
- The rest of the page is checked to be zero.

**Pages 1-4, tail slots.** Slot A is pages 1-2 and slot B is pages 3-4. Each slot is one page of ciphertext (the partial last page, zero padded) and one meta page. The meta page begins `magic u32 | seq u64 | page index t u64 | used u32 | nonce 12 | tag 16`, and the rest is zero. The AAD is `id ‖ "tail" ‖ seq ‖ t ‖ used`. The logical length is `t·P + used`.

**Journal (only when the journal flag is set), pages 5-36.** There are two halves, J0 (5-20) and J1 (21-36). Each half has one meta page and 15 ciphertext pages. The meta page holds `seq` and up to 15 entries `(page index u64, nonce, tag)`, with an AEAD tag over the meta (AAD `id ‖ "jrnl" ‖ seq`).

**Data region.** D = 5·P, or 37·P with the journal. Group g holds 128 data pages:
- tag page at `D + g·129·P`;
- data page `n = 128g + j` at `D + (129g + 1 + j)·P`.

Tag entry j sits at tag page + 32j: `nonce 12 | tag 16 | rsv 4`. The data page's AAD is `id ‖ n`. Every home data page is full; a partial last page lives only in a tail slot.

**Overhead** is 1/128 (0.78%) plus 20 KiB a file (148 KiB with the journal). Plaintext page n is exactly one stored page, and its tag page covers 512 KiB of data and stays resident as the hash slots do.

**Per file kind:**

| file | layout | journal | notes |
|---|---|---|---|
| segment (seal, merge piece) | v2 | no | `UntilNamed` |
| direct run | v2 | yes | `ReadAfterCrash`; becomes the segment by hard link, as today |
| ordered index | v2 | no | `Regenerable` |
| form file | v2 | no | A form page is one stored page. It is the same 4 KiB the format already has (zpages.rs:99); the compact form does not change. |
| WAL | unchanged | n/a | passes; sealed per record by `LogCipher` (vault.rs:1773-1858) |
| manifest slot | unchanged | n/a | passes; sealed per slot. Its plaintext loses the wrapped keys and keeps `(token)*`. |
| marker | unchanged | n/a | |

## 3. Write path

**Seal-on-write.** A kept file is still a memfd for its plaintext. That keeps `RunMap`, the finish's read-backs and the helper's `pread` working unchanged (db.rs:6198, 7753, 11469). `KeptFile::write_*`:

1. `pwrite`s the plaintext into the memfd, as today.
2. For every page the write completes, copies it into a per-handle 1 MiB staging buffer, encrypts it in place under a fresh nonce, and records its tag entry.
3. `pwritev`s the staging buffer to the stored offsets (one run a group, split at tag pages), then writes the changed sectors of the tag pages.
4. If a partial page remains, seals it into the tail slot `seq+1` (the other slot, ping-pong).
5. Marks the pages present and backed in the pager, so they are evictable.

When the call returns, the file's ciphertext is complete in the page cache up to its plaintext end. Under a buffered policy that is what makes a kept file survive a process crash, as a plain one does.

**`sync_data`** is `fdatasync` of the envelope fd. **`writeback`** is `sync_file_range` over the stored range the plaintext range maps to.

**In-place patches.** The segment finish rewrites page 0 and the head reserve (db.rs:7728-7824). A rewrite of a sealed page reseals it under a fresh nonce.
- `UntilNamed`: rewritten in place. A torn rewrite is a file no manifest names, which is swept.
- `ReadAfterCrash`: a rewritten page that had been synced is staged. At the next `sync_data` the layer writes the journal half `seq%2`, `fdatasync`s, writes the same ciphertext home (same nonce and same plaintext, so no reuse), and the caller's `fdatasync` follows. This is the direct run's close. The section header at `key_off = SUPER` (db.rs:6645, format.rs:43) shares page 1 with the first committed records, so an unjournaled torn rewrite would lose acknowledged records.

**Other cases:**
- Partial tails synced then extended (the run at every commit, db.rs:6917-6936): the tail slot. When the page fills it goes home once and is never rewritten.
- Read-back of unsynced bytes: from the memfd.
- Files mapped while written: `RunMap` over the memfd under `Mapped`, as today.
- `O_APPEND`: only the WAL uses it, and it passes. `KeptFile` keeps its own cursor.
- `rename` / `hard_link`: table only, as today (vault.rs:1013/1022). The key is bound to the token and the file id, never to the path.

## 4. Read path and the plaintext cache

The pager is today's with `CHUNK = 4096` (pager.rs:109). A unit is 256 pages, 1 MiB aligned, in place of 256 chunks over 255 pages (pager.rs:123). `Per`, the multiply-shift divide, becomes a shift. `fill` (pager.rs:966-1006) copies stored page `map(n)` into the memfd page, reads the tag entry from the group's tag page, and decrypts in place. A partial last page is read from the newest valid slot. Eviction, grace and rests are unchanged.

- **Hit:** one load and compare in `ensure` (pager.rs:1312), plus `slice_slow`'s bounds arithmetic, now by shift.
- **Miss:** first-touch fault on a fresh memfd page (about 2.2 µs) + copy (0.8) + cipher (0.4). That is one fill wherever today a range across a 4080-byte boundary took two.
- **Ranges known ahead:** `Pieces::window`, the 64-record scan windows and a builder's runs get `MADV_POPULATE_WRITE` over their EMPTY pages in one call before the fills. The probe and CLAUDE.md put bulk population at half the per-fault price.
- **Memory** is bounded as today, by the budget against `could_give`. The difference: nothing is unbacked except what a `Mapped` hold pins, because every written page is sealed when written. A seal or merge output in flight no longer reads as an overshoot (`Pager::could_give`, pager.rs:509).

**Safety under eviction** is today's. The evictor marks a unit, bumps the era, waits on `oldest_lent` (db.rs:9985), then frees with `MADV_REMOVE`, so a reader holding a borrow is never under a freed page.

## 5. Crash consistency

The tag page assumption: a tag page is rewritten only with new entries, and old entries are rewritten byte-identical. A sector-torn write therefore leaves every acknowledged entry intact. This is the same assumption the plain WAL's tail page relies on.

**Segment / merge piece / form file**
- Plain: temp, fsync, rename, then the manifest names it.
- Here: streamed, `sync_data` really syncs, rename is a table rename, and the manifest names the token.
- Power loss before the name: an unnamed token, which the open lists under its sealed creation name (a temp name), so the engine's own sweep removes it as it does for a plain store.
- After the name: the slots and pages were synced before the manifest that names them.

**Direct run.** A commit is records plus marker (one write call), then `sync_data`.
- Recovery takes the newest tail slot whose home pages since the other slot's `t` all authenticate. Otherwise it takes the older slot, which is the previous acknowledged commit. If neither, it takes the longest authenticating prefix of home pages. Then it applies a valid journal half newer than the home pages, then the marker walk (db.rs:6943-6990).
- A torn tail slot or torn home pages belong to an unacknowledged commit, matching the plain guarantee.
- Process crash under a buffered policy: the ciphertext is in the page cache, the same as the plain run.

**Ordered index.** Named unsynced, as today. The header carries the boot id of the boot that wrote it. A map under another boot verifies every page first, and a failure is "torn", which is the engine's rebuild path (`Db::ord_rebuilt`), never `Pager::poison`. Under the same boot nothing is verified, because only a power loss tears.

**WAL, manifest, marker:** unchanged.

## 6. Cryptography

- **Keys.** The caller's key wraps the store key in the marker (unchanged). The store key wraps one random key a file in the file's own header, with AAD `"lfile" ‖ token ‖ header`. It moves out of the manifest slot (vault.rs:169, 880). The WAL's per-file keys are unchanged. `.o` and `.z` get their own keys and stop sharing the segment's (vault.rs:1366, 1447).
- **Nonces.** Every page seal, tail slot and journal half draws a 12-byte nonce: an 8-byte random prefix drawn per handle open, then a 4-byte counter for that handle. The nonce is stored in the tag entry. Uniqueness under every rewrite follows: each seal takes a fresh counter, and a handle that would pass 2^32 seals reopens with a fresh prefix. The only re-encryption of one nonce is the journal-to-home copy, which is the same plaintext, so it produces the same ciphertext.
- **AAD.** The file id, plus the page index or slot `seq`/`t`/`used`. Pages cannot move between files or between positions, and the length cannot change without failing a tag.
- **Authenticated:** every byte of every kept file, and the length.
- **Not authenticated, and new:** a page put back to an earlier sealed version of itself within the same file, by an attacker who kept old ciphertexts. In write-once files the only earlier versions are the zeroed head and earlier tails. Such a store is refused by its format's own checks, or reads as the run cut at an earlier acknowledged commit. This is weaker than today's whole-envelope binding and is written into the threat model. Directory rollback is as before.
- **Leaked:** file count, sizes to the page, commit boundaries (tail slot writes), and which pages were rewritten. Names stay random tokens.

## 7. What goes, what stays

**Goes (about 750 lines, from the maps):**

| what | where |
|---|---|
| `RUN_LOGGED`, `log_run`, `Direct::logged`, the run's frames in `Wal` replay | db.rs:11520, 28219-28223, 28834-28862, 4746; ~150 |
| `persist`, `persist_one`, `persist_with` | vault.rs:1299, 2118-2132, db.rs:28999, 29525, 12574, 12865, 13033; ~220 |
| `persist_form`, cold drop | vault.rs:1425; ~90 |
| `manifest_image`'s late envelopes and `CipherCounts::late` | db.rs:11669; ~60 |
| wrapped keys in the slot | ~70 |
| `cipher_controller`, once E1 reads ahead | db.rs:11529; ~20 |
| `Per`, the 255-page unit geometry, the straddle cases in `fill`/`ensure` | ~80 |
| `MappedFile::owed`'s envelope-on-drop | ~60 |

**Added (about 900 lines plus about 600 of tests):**
- `src/db/lenv.rs`: layout, sealing, tail slots, journal, recovery; ~550
- `vault::File` and `KeptFile`; ~250
- the call-site type changes; ~80

**Stays:** the door, `passes()`, the tokens and the sealed manifest, `LogCipher`, `Kind` and `companion`, the pager's state machine and every rest site (readpath.md §3), `SegSrc::Kept`, `Mapped`.

**Form knowledge left in the vault, and the verdict:**

| knowledge | why it is there | verdict |
|---|---|---|
| (a) `passes()` and `LogCipher`: the log and manifest by name prefix, sealed by the engine | not blamed by any of the five numbers | Not acceptable against the owner's remark. It is removable by the same envelope: the tail slot covers append-and-sync; an `open_slot` generation (all pages of one slot write share a generation, so a mixed-generation slot reads as torn) covers the slot rewrite; a recycle is a new header. Stage S7. |
| (b) `Kind`/`companion`/the `.z` cascade: knows a segment has `.o` and `.z` companions | name hiding, because fenced names carry key bytes | Not acceptable. Removable by sealing each name into its dirent (`x-` + base32(nonce ‖ name ciphertext ‖ tag)). The longest name, `par-…-<32 hex>-<32 hex>.sup.z` (db.rs:12711, 27256), is about 101 bytes, so about 209 characters, under 255. The directory listing is decrypted at open. That deletes the tokens in the manifest, `adopt_all`'s pairing and the creation name. Stage S8. |
| (c) `Contract`: the engine says which files recovery reads and which may be named unsynced | | Acceptable. It is what a plain filesystem gives everyone for free and a page cipher has to be told, and it names no format. |

## 8. Why a store in the clear pays nothing

- **Read side:** unchanged for plain. `MmapBytes::open`, `lend` is the whole mapping, `ASK_PIECES` is false.
- **Write side:** one `match` beside each file syscall. `AlignedWriter` writes a 1 MiB piece a syscall (db.rs:5464-5470, 7990), the run once a commit, `FileSink` once a piece. That is a branch beside a syscall, which the rule allows. Nothing is added per key, entry, block or step.
- **Removed from the plain path:** the `RUN_LOGGED` bit's read at a commit's end (`MemTable::after`).
- **Held by** the callgrind counts by function, at `origin/main` and at the tree, of the four probes (CLAUDE.md:1150-1158), plus `load_only.rs supdb 100000 1` for `commit_direct` and `read_only`/`lag0_probe` over `supdb-nocachezpages` for the form.

## 9. Predictions

**1. Out-of-cache point read, 0.17x** (`supdb-cipher64` vs `supdb-cipher`, 1M keys)
- No record straddles a chunk boundary, so the fills a read takes fall from about 1.3-1.6 to about 1.0-1.2 (not counted; E5 counts it).
- Prediction: 0.22-0.30x. Above 0.4x needs E5's backing.

**2. A miss, 4 µs**
- About 3.4 µs a fill: the same fault, copy and cipher, with less straddle arithmetic.
- Fewer fills a read, and ranges populated in bulk at about half the fault.
- A miss below the fault needs a backing that keeps physical pages across eviction (anon with `MADV_FREE`, E5). That is not committed.

**3. Ordered load, 0.68x durable and 0.52-0.57x buffered, 2.5 device bytes per byte stored**
- No log frames: the commits drop the WAL write and fdatasync, 20-50 ms over 300 commits.
- The drain drops the envelope, 43-55 ms, and the 14 ms unlink of the log the run filled (CLAUDE.md:1486-1496).
- What it adds:
  - sealing 40 MB on the writer thread, about 8 ms at `ring`'s rate;
  - an 8 KiB tail slot a commit;
  - one journal fdatasync at the close, about 0.3-1 ms.
- Device bytes: about 1.4 (1.3 + 0.8% tags + about 6% tail slots at 300 commits).
- Prediction: durable 0.85-0.95x, buffered 0.80-0.92x.

**4. Compact form under a bound, 0.73x**
- A form page is one stored page (today two), and the tag page is 1/129 of the file and resident.
- The faults a read fall from 0.60 to about 0.3, the raw rate.
- Prediction: 1.0-1.3x of the raw form, the plain store's relation. The controller is turned on under a cipher.

**5. In-core point read, +339 instructions over 818; scan mix 0.78-0.87x**
- `Per` and the straddle cases go, and fewer asks touch two chunk states.
- Prediction: +250-300 from this design alone.
- The 95 instructions of the refused first ask (`not_lent`) are untouched here. The lever for them, `Blob` recording at open that a section is lent, is owed a count on both arms (E7).
- Scan mix: about +0.02.

## 10. Risks

- **Tag page rewrite atomicity** is the sector assumption.
- **Page-version rollback** within a file is newly possible (§6).
- **Seal-on-write puts the cipher on writer threads.** A writer of many small unaligned writes pays an 8 KiB tail slot each call. The rule is a slot a write call, not a slot a byte.
- **Boot-id verification** of the ordered indexes costs the first open after a reboot: 38 MiB at 5M keys.
- **The journal** takes 15 pages a round. A larger rewrite of synced pages takes several flushes.
- **`vault::File`** touches `AlignedWriter`, the plain writer's hot type. An inlining loss would be a cost per piece, so it must be counted.
- **Two copies of written plaintext** (memfd and page cache) remain, as today.
- **Not verified:** that every raw-`File` site on a kept path is listed in §1, and that no kept file other than the run is read after a crash.

## 11. Experiments owed, cheapest and most decisive first

1. **E1, chunk = page alone.**
   - How: build `lenv` with the envelope still written at close (stages S1-S2), then `pair.sh supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1`, with `lagdev` and the faults a read.
   - Question: do the tags kept apart stay resident, and does the form read ahead? This decides the controller.
2. **E2, the fdatasync price of three write sites.**
   - How: a C probe in scratch writing commits as (home pages + tag sector + 8 KiB slot) against plain appends, with `strace -T` on `fdatasync`.
   - Question: does a sync of three locations cost more than one at commit sizes of 1-100 KiB?
3. **E3, seal-on-write on the run's commits.**
   - How: `SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1 load_only supdb-cipher 300000 1` after S4, then `pair.sh supdb-cipher supdb 300000 --passes load` and the buffered pair.
   - Question: the commits' and the drain's share.
4. **E4, the journal's flush at the close.** How: drain stamps, `finishsteps`.
5. **E5, the miss.**
   - How: `perf record -e cpu-clock --call-graph dwarf` over `cipher_store 1000000 64` after S2, the fills a read counted.
   - Plus a C probe in scratch: re-fill of anon pages after `MADV_FREE` against a fresh memfd fault, idle and under a cgroup bound with the balloon's reporting.
   - Question: is a backing change worth its `RunMap` rewrite?
6. **E6, a first open after a new boot.** How: `cipher_store` open time with the boot id forced to differ.
7. **E7, callgrind of the point read and scan** (CLAUDE.md:1825-1835) after S2, then with the lent-section field, both arms.
8. **E8, the plain counts** of §8 at every stage.

## 12. Staged plan (each stage gate green)

- **S1.** `lenv.rs` and its unit tests under the toy cipher: layout, tail ping-pong, journal, recovery rules, the boot rule. Not wired.
- **S2.** Kept files' envelopes become v2, still written at `persist`. Pager `CHUNK = 4096`, units 1 MiB, `Per` gone. Run E1, E5, E7. Delete `cipher_controller` if E1 reads ahead.
- **S3.** `vault::File` and seal-on-write for `UntilNamed`. Delete `persist*`, the late envelopes and the unbacked accounting. Keys move into the headers. `tests/cipher.rs` and `ZFUZZ_CIPHER=1` under a 16 KiB cache. E8.
- **S4.** `ReadAfterCrash` and `read_recovering`. The run as its own log: delete `RUN_LOGGED`/`log_run`. Crash-state tests built by hand: torn slot, torn home, torn journal, page 1 torn mid-close. Mutants for each. E3, E4.
- **S5.** `Regenerable` and the boot rule for the ordered index. A test that a torn `.o` rebuilds and does not poison. E6.
- **S6.** Bulk population for known ranges; E5's backing only if it pays.
- **S7 (form knowledge).** The WAL and the manifest under v2. Delete `LogCipher` and `passes()`.
- **S8.** Names sealed in dirents. Delete `Kind`, `companion`, the tokens.
