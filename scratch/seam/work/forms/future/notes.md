# Future form changes walked through Pagefile

Tree `3df85d22`, read only. Nothing was built or run. The design is `seam/design.md` as written, with the changes in
`seam/experiments.md` noted where they matter. Code citations are to lines I read. Where a line comes from the maps
and I did not re-read it, it says "(map)". The sibling note `work/forms/history/notes.md` reaches the same overall
verdict from the branch's past. This note looks at the branch's future.

The owner's requirement: "we'll need to be able to make form changes easily without constant encryption changes".

## 0. The answer first

Pagefile splits form changes into two kinds. They come out very differently.

**What a form writes is free.** Under the design, every file is plaintext above `vault::File` (design §1-2). So these
need no edit in `pagefile.rs`, `pager.rs` or `vault.rs`:
- a new byte layout;
- a new coding;
- a new record encoding;
- a new frame kind;
- a manifest field;
- a new page size.

Today several of these touch the vault (tokens, `Kind`, `passes()`, `LogCipher`), so this is the design's real gain.
It holds only after S3 for segments and companions, and only after S6 for the WAL and the manifest (decision 2a).

**How a form reads, and when, is not free.** The design keeps today's read side whole: the explicit-ask pager, the
readers by the piece, holds, rests, unlent waits, the check before naming, and the held build (design §6 "Stays").
Every new reader, background job, merge shape or sizing model must therefore follow about a dozen obligations
(§1 below). None of them exists in a plain store. Forgetting one is silent in the clear, and most are silent under a
cipher too, unless a test runs the new path under a cipher with a small cache. The gate does that only for paths the
fuzz reaches with default options (§1, O-test).

**A third group forces an encryption change even under Pagefile.** These are write patterns outside the five classes,
whose limits were sized from today's two Log users, the WAL and the run:
- a file read while written that outgrows memory;
- a Log file never rotated;
- more than 4 patches below the tail;
- truncation or punching;
- asynchronous or direct reads;
- a name longer than the sealed budget;
- a cross-directory link;
- a lock.

**What would make most of this free** (§4):
- (D1) a test mode that runs the existing suites through the page layer with an identity cipher and a small cache;
- (D2, D3) the class contracts, the name budget and the pin rule asserted in the plain arm under debug assertions;
- (D5) the whole-borrow API taken away from file-backed sources;
- (D6) a form's failure handled per operation, not per site;
- (D7) the check moved to the publish;
- (D8) one cache, not two;
- (D9) eight form-agnostic verbs added to the layer.

## 1. What a form author owes under Pagefile as written

Each obligation is cipher-only: a plain store never fails for want of it.

**O1. Door only.** Every open, create, map, rename, link, remove, fadvise and `sync_file_range` goes through `vault::`.
- Where: design §2. Today's test reads db.rs alone (vault.rs:2876-2905). The design extends it to a fixed list: zfile,
  ordindex, bytes, pager.
- If forgotten: under `Pg` the file lands in the directory as plaintext under its logical name. The open's sweep
  leaves it alone, since it lacks the sealed shape (design §4 Open, step 2). Or the read side cannot find it by name.
- Visibility: a leak, functionally silent.
- Caught without a cipher test: by the door test, but only for the listed files. A new module (`src/db/vlog.rs`) is
  not read by it. `tests/cipher.rs:148-207` asserts every directory name is known and no key or value appears, but
  only for the default options its `fill` reaches.

**O2. A class at create.** Once, Log, Replace, Regen or Regen-lazy (design §2 table), through `vault::create(path, Class)`
or `SegmentOptions`. Each class carries a contract the author must fit:
- **Once** is trusted only with a `FINAL` record, which only a final sync writes (design §4 Sync). A Once file never
  synced (plain needs no sync for a regenerable file) stays untrusted for good. Its failures are its own and never
  poison, so a reader must then treat `None` as damage.
- **Log** allows at most 4 overrides below the tail (§4 Patch, "asserted by a counting test"). `set_len` below the
  recovered length becomes a restart copy of the recovered prefix (experiments E4 F4, change 5). The session table
  holds 48; at the 49th resume the file is compacted by copying it (§4 Table capacity).
- **Replace** is rewritten whole in place.
- **Regen** is verified whole at every open, by the form's own check, and a failure after that poisons (§4 Trust).
- **Regen-lazy**: every read site treats `None` as the form's failure (§4 Trust; E0 item 6).
- The plain arm `File::Os` ignores the class, so **no plain test can see a misclassification.** Under a cipher, the
  crash fault-injection fuzz (§5, not yet built) and the S4 override-counting test would see some.

**O3. `finish()`** before the final sync of a Log file that becomes a finished file (§4 finish).
- If forgotten: the file stays Log. Its ring is consulted forever, and a damaged byte reads as a truncation, not a
  poison.
- Plain tests: none.

**O4. Logical name length.** The sealed spelling fits NAME_MAX with two characters to spare at today's longest name,
`par-{id:08}-{end_seq:016}-{hex lo}-{hex hi}.sup`: 115 bytes at full u64 widths with `FENCE_MAX = 16` (db.rs:4724,
12709-12714; design §3 Names). The budget is a padded name of 128 bytes; plain allows 255.
- If broken: create or rename fails under a cipher only, at the first file with the long name. That is loud where
  reached and invisible elsewhere.
- The S2 test (`the_longest_logical_name_seals_under_name_max`) checks today's longest name, not tomorrow's.

**O5. Ask before reading.** No whole borrow of a file-backed source. Under `Pg`:
- `MmapBytes::as_slice()` is the empty slice: `lend` is 0 (bytes.rs:376-379, 469-480).
- `lend_at`, `Blob::flat` and `Blob::record_regions` are `None` (blob.rs:1379-1382).

A reader must therefore be generic over `Sect` (flatindex's 23 functions are), or carry a second walk by the piece
(`scan_pieces`, `record_pieces`, ordindex's `head_absent`, ordindex.rs:505-524).

If forgotten, by shape:
- `as_slice()[i]`: an index panic.
- `as_slice().len()`: a silent no-op. `warm_file` does exactly this under a cipher today, and says so
  (zfile.rs:3783-3809).
- `flat()?` or `record_regions()?`: "absent" or "declined". The zfile mapped build would say "records are not
  resident" (zfile.rs:4269-4271).
- No tag failed, so `Pager::check` never fires, and the wrong answer stands.
- Caught without a cipher test: `tests/blob.rs:208-268, 358` (`Piecewise`, `agree`), for paths that go through
  `Blob` and that the author adds to `agree`, which the three-readers invariant asks for anyway. Engine-level, zfile
  and ordindex readers have no such plain test.
- With WARM (S7, lend = len while the store fits), a flat-only reader works in core under a cipher and fails only
  out of core, which is rarer still to observe.

**O6. Pin.** Every thread that reads holds a reader-table slot or `vault::hold(dir)`. For a plain store that is `None`
(vault.rs:2224-2229).
- If forgotten: `debug_assert!` "read with nothing pinned" in `Paged::ensure` (pager.rs:1316-1320), under a cipher
  only. In release, the evictor gives the page back under the reader, which reads zeros: wrong bytes, no error.
  `SUPDB_PAGER_GUARD` turns that into a fault.
- If the thread is the segment work's, the panic hangs the writer in `land_seal` (ROADMAP.md:274-278).
- Plain tests: none.

**O7. Rest, and ask again after a rest.** A long walk rests: `SCAN_REST`, `MergeRest` with `MERGE_REST = 256`
(db.rs:11980-12008), `build_rest` and `AHEAD_REST` (map). After a rest it asks again for everything it was lent
(db.rs:11983 doc).
- No rest: the cache is held past its budget, the evictor waits (`longest_wait_us`), and other readers breathe.
  Performance only.
- A borrow kept across a rest: zeros read as data. This is the class of the d0816fce hunt: keys out of order, keys
  outside a fence, another key's value.
- Plain: `NoRest` never rests, so a stale borrow is never exercised.
- Caught by `fuzz_range_under_a_cipher_that_does_not_fit_its_cache` (tests/fuzz.rs:1264), if the path is
  default-on.

**O8. Waits unlent.** A wait on a thread or the device while pinned goes under `unlent`/`unlend` (map: db.rs:20606,
20696).
- If forgotten: rings of waits ended by timeout, and the evictor stalls (CLAUDE.md, "settle waited for the upkeep
  thread's pass pinned"). Performance only.
- Caught by no test except the cache-peak assertions in `tests/cipher.rs`.

**O9. Check before naming.** A job that writes outputs from what it read calls `vault::check` before it names them
(map: db.rs:12865, 13033).
- If forgotten: a merge that read nothing for a key writes the key without values and deletes its inputs. That is
  silent data loss under damage.
- Caught by the damage property tests in `tests/cipher.rs:888-951`, and only for merges they reach.

**O10. Advice through `MmapBytes`.** `advise_willneed`, `advise_dontneed_range`, `resident_at`, the `Paging` forwards.
A raw `libc::madvise` on the pointer, or a second `memmap2` over the path, does nothing useful under `Pg`. Out of
core this is a silent loss of performance; the design's barred-call list (§2) catches `posix_fadvise` and
`sync_file_range`, not `madvise`.

**O11. A held build path** for any builder that reads records: `SegSrc::Kept` with `Walked::Held`, beside the mapped
build (zfile.rs:4262-4320; design §6 keeps it, selected by `vault::paged`).
- If forgotten: the builder declines or fails under a cipher, so the feature is off under a cipher by accident. That
  is the owner's anti-pattern, and nothing says so.

**O12. Sizing models.**
- Under a cipher the budget is two caches in series: the plaintext memfd and the kernel's cache of ciphertext. The
  plaintext cache defaults to half the budget (vault.rs:637-646, map). E7 put about 69% of the out-of-core gap there.
- The coding pick prices against the whole `z_budget()` (experiments, "the coding is sized against the whole
  budget").
- A new form chosen by the workload inherits a model the cipher makes wrong.
- If wrong, the controller makes wrong buys. Only pairings show it, and the controller was switched off once for it
  (`cipher_controller`, map: db.rs:11529).

**O13. The plain cost, counted.** A generic reader monomorphises for free. A second walk named inside the first costs
3-4 instructions an entry (CLAUDE.md, the plain store's price). Only the callgrind rows (design §7) see it.

**O-test. What the gate runs under a cipher without being asked.**
- Four seeds each of `fuzz_range_under_a_cipher`, `..._that_does_not_fit_its_cache` and `..._with_the_compact_form`
  (tests/fuzz.rs:1252-1286).
- `scripts/check.sh:58` runs `cargo test --profile checked` with no `ZFUZZ_*` set, so the option draws that vary arms
  (`ZFUZZ_OPTVARY`, tests/fuzz.rs:293-302) are off.
- The cipher fuzz forces only the compact form (`ZPAGES`, tests/fuzz.rs:146-148). A new arm or structure is exercised
  under a cipher only if it is the default, or if its author adds a cipher variant.
- Plus `tests/cipher.rs`, which drives default options and the compact form.

## 2. The changes walked through

Columns:
- (a) what must change in `pagefile.rs`, `pager.rs` or `vault.rs`;
- (b) what the author does that a plain store would not ask;
- (c) what goes wrong if they do not, and how visibly;
- (d) the existing test that catches it with no cipher-specific test written.

### C1. A new index layout for the key section

Sources: Robin Hood probing with displacement bits; the exact-capacity hash with Lemire's reduction, which
`docs/index-theory.md` calls "a format change" and "a trade"; CLAUDE.md's "smaller and faster points on the frontier".
- (a) Nothing.
- (b) Write the probe generic over `Sect`, as `flatindex`'s readers are. A probe sequence is contiguous slots, so a
  `Pieces` window asks for it. Keep `indexed!`'s two arms naming the same generic function. Count the plain cost.
- (c) A lookup written over `flat()` alone leaves `indexed!`'s by-the-piece arm reading the old layout or `None`. A
  key reads as absent, or misdecodes, under a cipher, with no tag failure and so no poison. The wrong answer is
  silent.
- (d) `tests/blob.rs::a_source_that_lends_by_the_piece_answers_the_same` (blob.rs:358), if the new layout's segments
  are built by that test's `build_opts`, which a layout gated on a writer option needs its author to add. Also
  `fuzz_range_under_a_cipher`, if the layout is the default.
- Verdict: **free plus discipline O5/O13**, caught in the clear for the `Blob` paths.

### C2. A new record encoding

Sources: another compact record, a FIXED-style flag, a range-tombstone record (ROADMAP "Range delete"), the
`hash+flatfixed` record of index-theory §8.
- (a) Nothing.
- (b) The record parsers are not generic: by-the-piece reads hand them a window that holds the record, growing to
  `RECORD_WINDOW = 16 MiB` (map: flatindex.rs:678). The parser must not read past what it asked for. A record that
  can exceed 16 MiB needs a rested walk (C15).
- (c) A parser that over-reads a short window answers "not less" or "absent" under a cipher (CLAUDE.md: "a search
  treats a record that will not decode as not less"). Silent.
- (d) `tests/blob.rs` `agree` over `Piecewise`, which counts what was asked (blob.rs:208-268). The fuzz under a
  cipher, if the encoding is default.
- Verdict: **free plus discipline O5**, caught in the clear.

### C3. A new coding for the compact form

Sources: LZ4 HC (CLAUDE.md, the decoder), raw tails, the record coding.
- (a) Nothing. Compression sits above the layer.
- (b) Nothing for the bytes. **But** the coding pick prices against the whole `z_budget()` where the cipher's
  plaintext cache is half of it (experiments E1c), so a new coding's choice is made on the wrong budget (O12).
- (c) A wrong pick under a cipher, seen only in a pairing.
- (d) None.
- Verdict: **free in the bytes; model-assumes-page-cache in the choice.**

### C4. A new page size for the compact form (`ZPAGE`)

- (a) Nothing. Pagefile's page is the stored page (4096, design §3). A form page of 8-16 KiB is 2-4 pages and one tag
  page group, and the straddle that cost 0.73x is gone by construction.
- (b) None. `page_bytes` asks for the whole page (zfile.rs:2147).
- (c) None.
- (d) None needed.
- Verdict: **free.** Today this would interact with the 4080-byte chunk; under Pagefile it does not.
  - A page **below** 4096 is also free functionally.
  - The sampler's price of a fault (one major fault per form page) is then off by the ratio. That is O12 again.

### C5. A second compact structure beside the records

Sources: a compact hash, an over-store, a page fence over the raw records (zpages-design D8's L1/L2), CLAUDE.md's "a
cell a comparator wins is a workload the engine has no strategy for".

This is the change the owner's strategy names.
- (a) Nothing, if it is a Regen-lazy file with a short name.
- (b) Almost everything in §1:
  - O2: choose Regen-lazy and handle `None` as the form's failure at every site;
  - O4: the name;
  - O5: the reader by `slice_at`;
  - O6: holds for the builder, attach and sampler threads;
  - O7: a rest at each page written;
  - O10: advice through `MmapBytes`;
  - O11: a second, held build path (`PieceImages`, `Walked::Held`);
  - O12: a sizing model for two caches;
  - O13: the plain count.

  The compact form paid each of these on the branch (b4d555bc, about 1,300 lines of `src/`, per the history note).
- (c) If missing:
  - With no held build, the build declines under a cipher ("records are not resident") and the structure is never
    built there. That is a switch-off nobody wrote down.
  - A builder with no hold panics in debug under a cipher. On the segment-work thread that hangs the writer.
  - A read site mapping `None` to absent answers wrong, and because Regen-lazy failures never poison, nothing
    replaces the answer.
  - A model with one cache buys wrong, seen in pairings only.
- (d) None. `fuzz_range_under_a_cipher_with_the_compact_form` forces `ZPAGES` alone (tests/fuzz.rs:1277-1286). The new
  structure needs its own cipher variant and its own `tests/cipher.rs` tests, as zpages did (tests/cipher.rs:1517-2005,
  eight tests).
- Verdict: **heavy discipline**; the structure is cipher-aware in five places.

### C6. A new file kind beside a segment

Examples: a filter file, a per-segment range-tombstone file, a block table moved out of the segment.
- (a) Nothing. The design removes `Kind`, companions and cascades (§2 "Calls that go away"). The engine's own sweep,
  by logical name, is the plain one.
- (b) O1, O2, O4.
  - If the file is unsynced and regenerable, it must be Regen, not Once.
  - A temp that must survive an open must not end `.tmp`, since the design's sweep removes `.tmp` in both modes (§4
    Temps). That one is a plain behaviour, so plain tests catch it. That is the right shape.
- (c) If missing:
  - A raw call: a plaintext file in the directory.
  - Once without a sync: the file stays untrusted, and a damaged byte reads as the file's own error and never as a
    poison.
  - A long name: create fails under a cipher.
- (d) The door test, only if the file is in its list. `tests/cipher.rs:148` (directory names and contents), only if
  defaults create the file.
- Verdict: **free bytes, discipline in class and name.** Today this is an encryption change: a fourth `Kind` and a
  token, or `passes()` (vault.rs:562-564).

### C7. A new WAL frame kind

Examples: a range delete, a schema record.
- (a) Nothing, under S6. Nothing before S6 either: `LogCipher` seals a write's bytes as one record and the frames
  are inside it (db.rs:4570-4573).
- (b) None.
- (c) None.
- (d) `fuzz_range_under_a_cipher` replays frames after crashes.
- Verdict: **free**, both today and under the design.

### C8. A change to how the WAL rotates, recycles or truncates

Examples: a new header kind like the loose boundary, a pool of spares, retiring off the drain (ROADMAP.md:224-230),
truncating the front, `fallocate` instead of a zero prefill.
- (a) **Yes, in several sub-cases.**
  - Before S6, or under decision 2b: the replay's cipher branch names header kinds (`wal_kind_named(WAL_HEAD_SEALED,
    ..)`, `recycles`, db.rs:4565-4567), and `LogCipher` holds the header layout. A header change is an encryption
    change.
  - Under S6, recycle is a layer verb (`vault::recycle`) whose crash rule — "a recycled-flagged Log entry reads any
    file_id mismatch as an empty log" (E4 F7, change 6) — is the WAL's recycle state moved into the layer (design §4
    Recycle). A new recycle shape changes that rule.
  - `vault::preallocate` encodes the zero prefill.
  - A cut below the recovered length is a restart copy of the prefix (E4 change 5).
  - A punch or front truncation has no verb at all.
- (b) O2's Log contract, plus whatever verb is missing.
- (c) A cut on a large Log file copies its prefix at each open: performance. A missing verb fails at the call.
- (d) `ZFUZZ_RECYCLE=1` is not in the gate. `tests/cipher.rs:1084` covers recycling as it is today.
- Verdict: **renames and new files are free under S6; recycle, prefill and truncate force encryption changes**,
  because the layer's verbs are the WAL's current verbs.

### C9. A field added to the manifest

Examples: a format version (ROADMAP "A stable on-disk format"), a lock id, a new kind's names.
- (a) Nothing, under S6. Replace rewrites the slot whole.
- (b) None.
- (c) None.
- (d) None needed.
- Verdict: **free.** Today a field is free too, but a new kind of *named file* needs a token and a wrapped key in the
  slot, and a match in `adopt_all` (vault.rs:1575, map). S3 removes that.

### C10. A new background thread that reads segments

Examples: a scrubber, a warmer, a checkpoint copier.
- (a) Nothing for correctness. **Yes** for a scan-resistant read:
  - design §4 makes `forget` and `willneed` act on the ciphertext mapping;
  - a plain thread that drops pages behind its read (`DONTNEED`) would leave every decrypted page in the plaintext
    cache, pushing out what readers use;
  - the pager has no "read without keeping" verb.
- (b) O6, O7, O8, O9 if it writes, O10.
- (c) If missing:
  - No hold: a debug panic under a cipher, or a hang if the thread is the segment work's.
  - No rests: the cache held past its budget.
  - A warmer by `as_slice`: does nothing under a cipher. There is a precedent: `warm_file` (zfile.rs:3783-3790)
    computes `len = 0` under a cipher and returns, and its comment calls that correct.
- (d) None, unless the fuzz turns the thread on by default.
- Verdict: **discipline**, plus **an encryption change for a "don't keep" read.**

### C11. A new reader: a reverse cursor or a bounded scan

Source: ROADMAP "Reverse and bounded scans".
- (a) Nothing.
- (b) O5: either generic over `Sect`, or a reverse twin of `scan_pieces`' 64-record windows, since `scan_either` is
  forward only (map: blob.rs:2302-2448). O7: rest every `SCAN_REST` keys. O13: name neither walk inside the other's
  loop.
- (c) A flat-only reverse walk yields nothing from that segment under a cipher. The merged answer is short and
  silent.
- (d) `tests/blob.rs` `agree`, if extended. `fuzz_range_under_a_cipher` and `..._that_does_not_fit_its_cache`, if
  the fuzz's model draws reverse scans, which the author adds for plain correctness. So this is caught **without a
  cipher test**, in four seeds a gate.
- Verdict: **discipline, mostly caught.**

### C12. A multi-get

Source: ROADMAP "Batched point reads".
- (a) Nothing.
- (b) A batch that sorts keys and walks a segment once must not keep borrows across keys past a rest (O7). A batch
  that asks ahead uses `advise_willneed` (O10).
- (c) A batch under one pin with no rest holds the cache: performance. A kept borrow across a rest reads zeros:
  wrong values, silent.
- (d) The fuzz, if it draws multi-gets and the cache is small (the second cipher fuzz test).
- Verdict: **discipline, partly caught.**

### C13. A new merge shape

Examples: tiered, range-delete compaction, a merge that reads the compact form, a new parallelism.
- (a) Nothing. `persist` goes (design §2).
- (b) O6: `vault::hold`. O7: a `MergeRest` type parameter, with keys asked again after a rest (map: `merge_ranks`
  db.rs:12316). O9: `vault::check` before naming outputs.
- (c) If missing:
  - No check: data loss on damage, silent.
  - A borrow across a rest: keys out of order or outside a fence, which `tiling`/fence assertions catch loudly in
    debug.
  - No rests: the cache held.
- (d) `fuzz_range_under_a_cipher_that_does_not_fit_its_cache`, if the shape is default. The damage tests
  (tests/cipher.rs:888), only for merges their fixture reaches.
- Verdict: **discipline.** The O9 half is silent loss.

### C14. A value log (key-value separation)

- (a) **Yes.** A value log is appended and synced per commit, which makes it Log. It is also read at random while
  written, by offset: through `open_mapped` to read the bytes just written, or through `read_at` on the Handle. That
  hits three limits:
  1. **The owed backing.** A file opened through `open_mapped` is written through always, and its backing is owed
     (`MappedFile::owed`) and cannot be given back until the last mapping drops (design §4 Write-through;
     vault.rs:604-627). A value log mapped for its whole life holds its whole plaintext in memory. That is today's
     "store has to fit memory" for the one file that holds the store's bytes.
  2. **The session table.** A value log is resumed at every open and rotated rarely. At the 49th open it is compacted
     by a copy of itself (§4 Table capacity), which may be gigabytes.
  3. **GC.** Rewrite live values into a new log and remove the old is free. Punching dead ranges in place has no verb.
- (b) O2, O3, plus O5-O9 for its readers.
- (c) Memory grows with the log, unbounded and silent. The 49th open is slow. Punching fails at the call.
- (d) `tests/cipher.rs:372` (`a_store_larger_than_its_cache...`) holds the cache's peak to its budget, if the value
  log is on by default.
- Verdict: **forces an encryption change**:
  - givable home pages under a mapped Log file, owed only past the synced home;
  - a session table that spills instead of copying;
  - optionally, a punch verb.

### C15. Larger pages, or values larger than the cache

Examples: a 2 MiB write piece, larger blocks, a 16 KiB form page (C4).
- (a) Nothing for pieces and blocks. Stored offsets are linear within a 256 MiB extent (design §3), so a piece that
  crosses an extent boundary becomes two writes. That is invisible.
- (b) For values larger than the cache (ROADMAP.md:190-196), a walk of one key's runs must rest between blocks and ask
  again (O7).
- (c) A big value pins the cache past its budget for the read: performance, and `swamped` waits for other readers.
- (d) None. `tests/cipher.rs:742` covers wide values for builders only.
- Verdict: **discipline**, and an open roadmap item already.

### C16. O_DIRECT or io_uring reads

These are levers out of core; design §11 rejects them as a *default*.
- (a) **Yes.**
  - A finished `Pg` file holds no descriptor. It is read through a read-only mapping of its ciphertext (design §4
    Read and miss), so `as_os()` gives nothing to submit against.
  - A plain form that reads by io_uring into its own buffers has two choices under a cipher. It can fall back to
    `slice_at`, which switches the lever off under a cipher. Or the layer gains an asynchronous verb: read stored
    pages p..p+n into a caller's buffer, verify, and decrypt on completion.
  - Stored pages are 4096-aligned one to one, so O_DIRECT's alignment fits.
- (b) A second read path, or nothing if the verb exists.
- (c) The fallback compiles and works, so the switch-off is silent.
- (d) None.
- Verdict: **forces an encryption change** (one form-agnostic verb), or a silent switch-off.

### C17. A format the wasm reader must read

- (a) Nothing in the store's layer. The browser reads published objects through the `crypt.rs` v1 envelope, frozen,
  and `web/crypt.mjs`. It wraps any bytes, and the plans stay plaintext ranges (CLAUDE.md, the envelope).
- (b) None for encryption. `SparseBlob` and the plans are the plain three-readers obligation.
- (c) A format that wants chunks aligned to pages in the browser (a compact form served to the browser) would want an
  envelope v2. That is a new version in Rust and JS and a new file under `tests/data`, but form-agnostic.
- (d) `tests/crypt.rs` and `web/crypt.test.mjs` pin v1.
- Verdict: **free**, except a new envelope layout for performance.

### C18. A change to the segment finish's head patches

Example: the run given a head reserve (design §4: the run is created with `head_reserve: 0`, db.rs:6676), or a
trailer moved to the head.
- (a) For a seal (Once): nothing, since patches before `FINAL` are in place. For the run (Log): the 4-override bound
  was counted from today's finish, pages 0 and 1 plus the WAL's chain byte (design §4 Patch). A run that patches its
  reserve exceeds it.
- (b) Know the bound.
- (c) The override list grows in every `CKPT`. The S4 counting test fails, or would be relaxed.
- (d) The S4 counting test is cipher-specific but exists, so it is caught.
- Verdict: **forces an encryption change** when a Log file's patch pattern changes.

### C19. A change to the run's durability shape

Examples: truncating an uncommitted tail in place, a marker every N commits.
- (a) A truncation below the recovered length on a Log file is a restart copy of the prefix (E4 change 5). For the
  WAL's cut to its header that is one page. For a run cut in its middle it is the run.
- (b) O2's Log contract.
- (c) Performance at every such truncation. Silent.
- (d) None.
- Verdict: **forces an encryption change** if the truncation is common: an in-place tail cut with versioned tags,
  which E4 F4 showed is hard.

### C20. A checkpoint or backup (ROADMAP "Backup or checkpoint")

- (a) **Yes.**
  - A checkpoint is hard links into another directory, and `vault::hard_link` refuses a link across stores
    (vault.rs:1990-2006). The design keeps "today's same-store check" (§4 Rename, link, remove).
  - The target needs a marker, and its sealed names bind `store_id` (design §3 Names, AAD).
  - A Log file's copy must be a consistent prefix.
- (b) None of this is form work.
- (c) The checkpoint fails at the call under a cipher. Loud.
- (d) None.
- Verdict: **forces an encryption change**, one form-agnostic verb (`vault::checkpoint(dir)`).

### C21. A process lock (ROADMAP Security "One writer a directory", PR #21; not on this branch, not verified)

- (a) **Yes.**
  - Today a new name not in `passes()` becomes a memfd kept file (vault.rs:555-564), and a lock on a memfd locks
    nothing across processes. That is silent: two writers corrupt the store.
  - Under `Pg`, a lock needs a real descriptor, and a finished file has none. Another process must also find the lock
    file by a sealed name.
- (b) Nothing.
- (c) Today: a silent non-lock. Under the design: an error at the call.
- (d) None.
- Verdict: **forces an encryption change**: `vault::lock`, or a clear-named, contentless file the layer passes through.

### C22. A longer fence or a new naming scheme

Example: raising `FENCE_MAX` above 16 for keys with long common prefixes, which CLAUDE.md's fence bugs keep meeting.
- (a) **Yes.** The `par-` name is 51 + 4·`FENCE_MAX` bytes at full u64 widths. At `FENCE_MAX = 19` it is 127 bytes,
  which still pads to 128 (design §3), so it still fits. At `FENCE_MAX = 20` it is 131 and no longer fits (my
  arithmetic on the format at db.rs:12709-12714). The margin is three bytes of fence.
- (b) Plain allows names up to 255.
- (c) Create fails under a cipher only.
- (d) The S2 name test, only if it generates the longest name from the formatter, not a literal.
- Verdict: **forces an encryption change.** Today this is free, since names live in the sealed manifest beside tokens.
  **Pagefile regresses here.**

## 3. The three groups

**Free.** No layer change, no cipher-specific author work:
- C4, a form page size;
- C7, WAL frames;
- C9, a manifest field;
- C17, a wasm format;
- the bytes of C3, C6, C8 (new files) and C13 (outputs);
- every compression and layout decision.

This is the set the owner's sentence names, and Pagefile delivers it. Today it does not: C6 and C9-with-new-files
need `Kind`, tokens or `passes()`; C4 hits the straddle; C8 touches `LogCipher`.

**Discipline.** No layer change; the author owes O1-O13:

| change | caught in the clear | caught by the gate under a cipher without asking | silent wrong answer if missed |
|---|---|---|---|
| C1, C2 | `tests/blob.rs` `Piecewise` (for `Blob` paths) | if default | yes |
| C11 | `tests/blob.rs` (if extended) | if the fuzz draws it | yes |
| C12 | no | if drawn, small cache | yes (stale borrow) |
| C13 | no | if default | yes (no `check`: data loss) |
| C10 | no | no | no (panic or hang, or a no-op) |
| C5 | no | no | yes (Regen-lazy never poisons) |
| C6 | door test, if listed | directory test, if default | no (a leak) |
| C15 | no | no | no (performance) |
| C3 choice, C5 model | no | no | no (wrong buys) |

In practice the gate catches a missed obligation only when the new thing is the default and in the fuzz. A
non-default arm, which is how every form enters this repository ("the comparison arms in `Options` are not dead
code"), is never exercised under a cipher until someone writes a cipher test for it.

**Forces an encryption change**, even under Pagefile:
- C14, the value log (mapped Log memory, session table, punch);
- C16, async or direct reads;
- C18, Log patch count;
- C19, truncation of a large Log file;
- C20, checkpoint;
- C21, lock;
- C22, long names;
- C8's recycle and prefill shapes;
- C10's "don't keep" read.

Each of these is a new **file verb** or a lifted **limit**, never knowledge of a form. That fits the owner's seam,
but the five classes as written encode the WAL's and the run's patterns (4 overrides, 48 sessions, recycle's
empty-log rule, TRUNC-as-restart), so the next Log-like form outgrows them.

## 4. Changes to the design that would make these free

Ordered by what each removes, against its cost to a plain store.

**D1. A clear-pages test mode.** It makes O5-O9, O11 and O3 caught by every existing test.
- What it is: an engine-read switch, honoured under `cfg(debug_assertions)` only (the `checked` profile has them), for
  example `SUPDB_TEST_PAGES=<budget>`. With it, `Db::create`/`open` put every file under `pagefile` with an identity
  AEAD, **logical names in the clear**, and a plaintext budget of a few tens of kilobytes. CI runs the test group a
  second time with it set.
- Clear names keep the tests that list or parse file names working (`tests/common/mod.rs:51` `seg_names`, `:70`
  `partition_fences`).
- Tests that read stored bytes or page-cache residency by path need an opt-out. These are `segwriter`'s file
  comparisons, `cached_pages` (common/mod.rs:162) and `tests/resident.rs`. How many tests need it is not counted.
- What it buys: the reader-side obligations stop depending on someone writing a cipher test. A flat-only reverse
  scan, an unpinned scrubber, a merge with a borrow across a rest, or a builder with no held path fails in the plain
  suites' own tests, with debug assertions on. That is where they run already.
- Cost: none in release. CI time is about one more test group, roughly four minutes of tests (CLAUDE.md, the test
  group's figures), more under a small budget; not measured.
- This does not cover naming (O4), which D2 covers.

**D2. Debug-mode class contracts in the plain arm.** It makes O2, O3 and O4 caught by plain tests.
- `File::Os` is a branch beside a syscall (design §7), so `debug_assert!`s there cost nothing in release:
  - Once: no write after the final sync;
  - Log: append only, with a count of patches below the tail held to the bound; `set_len` only where the class
    allows it;
  - Replace: whole writes at 0;
  - every create and rename: the logical name within the sealed budget, computed by the same function the layer uses.
- Then a value log classed Log that patches its head, or a `FENCE_MAX` bump, fails the first plain test that makes
  one, rather than a cipher store in the field.

**D3. Pins asserted in plain debug.**
- `vault::hold` returns a debug-only token for a plain store too, instead of `None` (vault.rs:2224-2229). Plain
  `MmapBytes::lend_at`/`slice_at` `debug_assert!` that the thread holds a pin.
- `Sec::get` asks again at every lookup (CLAUDE.md, the envelope's reader), so most reads pass through the check.
- Release is unchanged, since `debug_assert` compiles out. `checked` tests pay a thread-local load per ask.
- This catches O6 in the clear. It is a subset of D1, for a smaller CI cost.

**D4. Pins and rests in the types.** This is not verified feasible.
- `slice_at(&'p Pin, ..) -> &'p [u8]`, and `Pin::rest(&mut self)`. The borrow checker then refuses a lent slice held
  across a rest: the class behind d0816fce and the merge's re-ask. It also refuses an unpinned read.
- A plain pin is a zero-sized type.
- The risk is plain instruction moves in readers that keep a section borrowed. Count it by the §7 rows before keeping
  any of it.
- Turns O6 and half of O7 into compile errors.

**D5. No whole borrow on a file-backed source.**
- Make `MmapBytes::as_slice`, `Blob::flat` and `Blob::record_regions` private to the generic internals, so a reader
  outside them can only be written over `Sect`, which monomorphises to `&[u8]` for a mapped file at no cost (as
  flatindex's readers do).
- A source-scan test bars the three names outside `blob.rs`, `flatindex.rs` and `bytes.rs`.
- Turns O5 and O11 from "silent under a cipher" into "does not compile".
- `warm_file`'s `as_slice().len()` and ordindex's `as_slice().get(..)` (ordindex.rs:419, 678) are today's instances
  that would have to move.

**D6. The form's failure handled per operation, not per site.**
- The pager records a Regen-lazy file's failure per file (design §4 Trust, E0). The operation's end, which already
  looks at `end_slow` (map: db.rs:18134), sees "a Regen-lazy file failed during this operation", takes that form out
  of service, and runs the operation again on the raw form.
- A new optional form's read sites may then answer anything on `None`, because the answer is discarded.
- Removes O2's Regen-lazy audit (E0 item 6) for every future form.
- Plain cost: none; it sits behind `ATTN_PAGED`.

**D7. The check at the publish.** `Maint::publish` refuses to name outputs while `vault::check` fails. One choke point
instead of each merge's call (map: db.rs:12865, 13033). Removes O9 for every future job that writes segments. For a
plain store it is a load beside a publish, the kind CLAUDE.md allows ("where something is made").

**D8. One cache.**
- Drop each ciphertext page from the kernel's cache behind the fill that decrypted it. The plaintext cache is then
  the store's only cache, its budget can be the group's budget less anonymous memory, and a pager miss is one major
  fault on the ciphertext mapping.
- The controller's model and the coding pick then mean under a cipher what they mean in the clear: one cache, a fault
  per miss. Removes O12 and the case for `cipher_controller`.
- Experiments E7 lists this as an unpriced candidate. It must keep `ru_majflt` counting misses, which it does, since
  the fill still reads through the mapping.
- Not measured. It is the only change here that rests on a pairing still owed.

**D9. Form-agnostic verbs and lifted limits.** These take the "forces a change" group to zero for the cases in §2.
None of them names a form:
- (i) a mapped Log file's home pages givable, with only the tail past the synced home owed (C14);
- (ii) a session table that spills to a page of the prefix region instead of compacting by copy (C14);
- (iii) the override bound as a parameter of the class, with the overrides carried by reference in `CKPT` once
  durable (C18). This needs E4 re-run;
- (iv) `punch(range)` on Once and Log files (C14, C19);
- (v) an asynchronous `read_pages(f, p, n, buf)` that verifies and decrypts on completion (C16);
- (vi) `vault::checkpoint(dir)` (C20);
- (vii) `vault::lock` (C21);
- (viii) names longer than the budget: a fixed-length spelling with the long name sealed in a header page and read at
  the open's listing, at one header read per long-named file (C22). Alternatively, D2's assertion plus a decision that
  logical names stay within 128 bytes, written down where `FENCE_MAX` is;
- (ix) a pager read that does not keep (C10).

Recycle and prefill (C8) are better taken **out** of the layer, not added to it. The empty-log rule for a recycled
header is the WAL's, and E4 F7 shows it has two shapes already. The WAL can read a foreign header as empty itself, as
`Wal::replay` does today for `recycles` (db.rs:4565-4567), if the layer reports "header of another file_id" as a
distinct error instead of deciding.

**D10. The door test reads every source file.** Walk `src/` at test time through `CARGO_MANIFEST_DIR` instead of a
fixed `include_str!` list (vault.rs:2876-2905; design §2's extension is still a list). Then a new module is covered
without its author remembering the test exists. Add `madvise` and `memmap2::` to the barred calls outside `bytes.rs`
and the door (O10).

**D11. The cipher fuzz draws what the plain fuzz draws.** One of the three cipher fuzz tests in the gate sets the
option draws (`ZFUZZ_OPTVARY`) on a seed rotated per run, so a new arm added to the fuzz's draws is exercised under a
cipher with no further step. CI time: the same four seeds. Not run.

**With D1, D2, D5, D6, D7, D10 and D11**, the §3 discipline table collapses:
- what the author must still *know* is the class, and that a reader is written over `Sect`;
- forgetting either is a plain test failure or a compile error.

**With D8 and D9 as well**, the forced group is empty for every change listed here. What is left is real cost work
(O13, counting the plain path) and the out-of-core performance of the miss, which is not a form question.

## 5. Not verified

- D1's feasibility: how many tests read stored bytes or residency by path, and whether an identity AEAD under
  `pagefile` with clear names is a small mode of S2's layer. The layer is not built.
- D4's cost to plain readers that hold a section borrowed for a Blob's life.
- D8's effect: E7 names it as a candidate and nothing priced it.
- C21: PR #21 is not on this branch. `flock` and `LOCK_EX` do not appear in `src/db.rs` or `src/db/vault.rs` (grep).
- C22's boundary is my arithmetic on the name format; the design's 128-byte padded budget is its own arithmetic
  (design §3), not a test.
- The design's verbs for advice (`willneed` on the ciphertext mapping and also as a fill of the plaintext; design §4
  Read and Residency say both) are ambiguous for C12. I did not resolve which applies.
- Lines cited "(map)" are from `seam/maps/*.md` and were not re-read: db.rs:11529, 12316, 12865, 13033, 18134,
  20606, 20696; blob.rs:2302-2448; flatindex.rs:678; vault.rs:637-646, 1575.
