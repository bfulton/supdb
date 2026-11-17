# The branch's history as evidence: what each form cost the cipher, and what Pagefile would have saved

Tree `3df85d22`, read only. Sources: `git log --reverse origin/main..HEAD` (15 commits), each message and
`git show --numstat`, the diffs where needed (cited by hash), the current tree (cited file:line), CLAUDE.md,
`seam/design.md`, `seam/experiments.md`, `seam/maps/*.md`. Line counts are insertions/deletions from
`--numstat` over `src/` (and `tests/` where said), attributed to a feature by reading the hunks' function
context. That attribution is approximate, and nothing was counted line by line. Nothing was built or run.

## 0. The fifteen commits, by what they are

| hash | what | cipher-forced work on a form? |
|---|---|---|
| a7998f70, 7e4e5102 | `crypt.rs` envelope for a finished file, plus the browser half | no. This is the envelope itself, which design §6 freezes |
| 61bc705d | `Encryption::Kernel` (dirkey) | no. The kernel's way touches no form |
| 15d0335e | `Encryption::Cipher`, first shape: memfd files, envelopes, the door, sealed WAL and manifest | **yes**: the door, plus the WAL/manifest/naming. It switched off three features (`under_cipher`) |
| b3213304 | one `Encryption` enum | no |
| eb377fe4 | store larger than memory: the pager, and readers by the piece | **yes**: every reader, and the ordered index |
| 38567647 | out of cache: rests, waits that hold nothing, two lines | **yes**: builders, waits and pins in the engine |
| 9c844bd7 | CLAUDE.md only | no |
| 55c3e526 | the plain store pays nothing: the read side restructured | **yes**: the cost of the read side on the plain path |
| 17c363a5 | recycled log under a cipher | **yes**: the WAL |
| d0816fce | pager race (a chunk lent out of a unit marked twice) | pager-internal; no form |
| 0ea02c5f | ordered ingest under a cipher (`log_run`) | **yes**: the direct run |
| b4d555bc | compact form under a cipher | **yes**: builder, attach, form files, advice |
| 81ca5d3e | controller off under a cipher | **yes**: the controller |
| 3df85d22 | forged key sections | no (security review; plain) |

`src/` insertions in the nine form-relevant commits come to about 13,900, with tests about 2,900 more:

| hash | `src/` insertions | biggest files |
|---|---|---|
| 15d0335e | about 2,800 | vault 2,251 |
| eb377fe4 | about 3,900 | pager 1,648, blob 371, flatindex 306, bytes 217, db 550, vault 402, ordindex 86 |
| 38567647 | about 290 | |
| 55c3e526 | about 920 | db 638/268, blob 233/111 |
| 17c363a5 | about 70 | |
| d0816fce | about 170 | |
| 0ea02c5f | about 305 | |
| b4d555bc | about 1,300 | zfile 425/118, vault 367, pager 220 |
| 81ca5d3e | about 45 | |

## 1. Per feature: what it took, which bugs, whether it was switched off

Each row gives:
- **E**: encryption code (vault/pager/crypt) that changed because of the form;
- **F**: form or engine code that changed because of encryption.

### 1.1 The door (15d0335e)

**E:**
- `vault.rs` free functions (`create`, `open`, `open_write`, `open_append`, `rename`, `hard_link`, `remove_file`, `names`, `len`, `exists`, `sync_dir`, `open_dir`, `map`, `read`).
- The decoy directory.
- The source-scanning test `the_engine_names_no_file_but_through_the_door` (vault.rs:2876).

**F:**
- Every `std::fs` call in db.rs was rewritten to `vault::`. That is 124 added lines naming `vault::` in the 15d0335e db.rs diff, and 143 occurrences today (db.rs), 16 in zfile.rs and 1 in ordindex.rs.
- db.rs churn was 413/170, mostly mechanical.

**Bugs (CLAUDE.md, "Four things building it"):**
- `File::open(&self.dir)` became `vault::open`, which opened the marker. It needed `vault::open_dir`.
- The first test's control found no fence in a name.

**Switched off:** no.

**Under Pagefile:** stays, and is widened. S1 routes more sites through the door:
- `start_writeback` (db.rs:6071);
- `fadvise_dontneed*` (db.rs:14904);
- zfile's `drop_written_range`, `drop_behind` and `swap_ahead`, with the bare `File::open` at zfile.rs:3972;
- `ZFile.file`;
- `Vec<Option<File>>` (db.rs:12762).

The door test is extended to bar `File::open`, `as_raw_fd`, `posix_fadvise` and `sync_file_range` outside the door. This is a discipline every form author follows: *every* file operation, advice and writeback hint included, goes through the door. Forgetting it fails under a cipher only ("not a directory" against the decoy), or in the source scan.

### 1.2 File storage on the write side: memfd, envelope, tokens, sweep (15d0335e, 38567647)

**E:**
- the memfd write-once table;
- `persist` and `persist_one` (vault.rs:1291, 2118), called from the seal and merge threads (db.rs:12574, 12865, 13033, 28999, 29525 per design §6);
- `vault::Ahead` (write-back hints behind the cipher);
- `Kind` (vault.rs:468);
- tokens;
- `sweep` of unnamed envelopes;
- `MADV_POPULATE_READ` in the memfd shape's `map`.

**F:**
- `manifest_image` (db.rs:11661) writes and syncs the envelope of every segment it names before the slot.
- The seal and merge threads persist by name and skip names that are gone (a promotion's link and unlink race).

**Bugs:**
- Two views of one memfd (`Vault::settled`): hash slots read as zeros, one seed in 200.
- The store `POSIX_FADV_DONTNEED`'d its own ciphertext after writing an envelope (38567647): 53 MB read back from the device in one pass. Removed.
- `could_give` counted a file with no envelope yet (38567647).
- 0ea02c5f: `CipherCounts::late`. With the run's envelope deleted, every test passed, because the manifest covered for it.

**Switched off:** nothing directly. This shape is what made the run, the recycled log and the form "impossible".

**Under Pagefile:** **gone**:
- memfd table, `persist*`, tokens, `Kind`, `companion`, `kept`, the manifest's late envelopes, `CipherCounts::late` and `Ahead` (design §6, "Goes");
- the two-views bug class, with the table;
- the fadvise bug class, since there is no envelope written after the fact.

Watch one thing: experiments.md E7 proposes, as a lever on the faults term, "not caching ciphertext the plaintext covers (drop a page's ciphertext behind its fill)". That is the 38567647 bug's mechanism, reborn as a lever. It is safe only while the plaintext is never given back before the ciphertext would be re-read.

### 1.3 Manifest (15d0335e)

**E:**
- `seal_manifest`, `open_manifest` and `adopt_all` (vault.rs:1633, 1657, 1575): a slot sealed with `(token|wrapped key)*` behind it.
- Mutants: the count check moved to where a test can hand it a count.

**F:**
- `manifest_image` and `manifest_open` call the vault.

**Switched off:** no.

**Under Pagefile:**
- The tokens and wrapped keys go at S3.
- `seal_manifest` goes at S6, when the slots become the Replace class.

New: the rename-shape manifest arm (`manifest_slots: false`) is **refused by name under `Pg`** (design §2). That is a new "switched off under a cipher" item the design introduces, small but of exactly the kind the owner objects to. The reason is that the class rule would put a directory sync on its hot rename-over.

### 1.4 WAL (15d0335e) and the recycled log (17c363a5)

**E:**
- `LogCipher` (vault.rs:1773-1858) and `passes()` (vault.rs:562), which gained `spare-` at 17c363a5.

**F:**
- `Wal::begin` and `resume`, the `SUPDBWE` header, a sealed record a write, and replay's cipher branch (db.rs:4134-4580, about 450 lines of the WAL now carrying cipher arms).
- 17c363a5: db.rs 60/13:
  - the recycled spare's header is drawn anew per life;
  - `Wal::replay` gained a `recycles` parameter, so a sealed header of the last life reads as an empty log.

**Bugs:**
- A replay that answered the header as the log's length cut the log to nothing (a mutant; a test for two crashes in a row).
- The first recycle synced the header into the spare before the rename, an order no test could hold.

**Switched off:** **yes**. `recycle_wal = false` in `under_cipher` (15d0335e diff, db.rs `fn under_cipher`), from 15d0335e until 17c363a5.

**Under Pagefile:**
- **Gone at S6.** Recycle draws a new file_id in the layer, and the header-lag state moves into the layer (design §4, Recycle).
- **Until S6, stays.** S6 is decision 2: (a) move, (b) keep `LogCipher` for good.
- Had the design existed first, none of 17c363a5 would have been needed, *provided* the WAL were in the layer from the start.
- The design's own recycle logic replaces it: the `flags.recycled` name bit, `prev_fid_lo`, and the empty-log rule. That is layer code, not form code. A future change to the WAL's format would touch no layer code, but a change to how the engine *recycles* (preallocate, `recycle`) goes through `vault::preallocate` and `vault::recycle`, which the design names (§2).

### 1.5 Readers by the piece (eb377fe4, then 55c3e526 for their plain cost)

**E:**
- `pager.rs` (1,648 lines at eb377fe4).
- `bytes.rs` 217/31:
  - `Bytes::piecewise`, `lend_at`, `ASK_PIECES`;
  - `MmapBytes::paged` with `lend: 0`;
  - `slice_slow`;
  - `Paging`.

**F:**
- `flatindex.rs` 306/92: `Sect` for `&[u8]` and `Pieces`, 23 functions generic over `S: Sect`, and `Pieces::window` up to `RECORD_WINDOW` 16 MiB.
- `blob.rs` 371/138:
  - `Sec` holding sections by place;
  - `Blob::flat` and `Blob::pieces`;
  - `indexed!`'s fallback (8 sites);
  - `not_lent`.
- Plus `tests/blob.rs` (206), a source that lends exactly what is asked and counts it.
- 55c3e526 then spent blob.rs 233/111 and bytes.rs 38 to make the plain store pay nothing:
  - the kind found in the refusal (`Blob::scan_whole` refusing, `scan_either`/`scan_refused` at blob.rs:2412);
  - `scan_at` asking only for `ASK_PIECES` types.

**Bugs, all cost bugs on the plain path, found by count:**
- 6 instructions a point read and 88 a scan (eb377fe4);
- `return self.scan_pieces(..)` named in the walk: 3-4 an entry;
- `by_piece(move || ..)`: 2 and 7;
- the window loop written into `scan_at`: 5 an entry;
- `emit_unsealed`'s RefCell held over the callback;
- 370 instructions a scan through the compact form, from `Blob::read_exts` growing a second block fetch (fixed at b4d555bc with `read_exts_inline`);
- `ZFile::view_of` not inlined: 1.8.

Six shapes of `scan_at` were built and counted before one read level.

**Switched off:** no. But it is the reason the compact form was off: "records borrowed whole".

**Under Pagefile:** **stays in full** (design §6 "Stays: the readers' ask path ... about 2,800"). The design keeps `MmapBytes::paged` with `lend: 0` for `Pg` files. Every future reader must:
- go through `Sect` or a window, or answer `None` under a cipher;
- be counted on the plain path by function after any change to a reader shared with the piece path.

S7 (WARM, `lend = len` for files whose plaintext fits 0.85·B) does not remove this. Demotion republishes `lend = 0`, so the piece path still has to exist and be right. And it makes a reader that only borrows whole *correct in core and wrong out of core*, which is a worse place for the bug to hide. S7 is gated on E3, which was not run.

### 1.6 Scans (eb377fe4, 55c3e526)

**F:**
- `scan_pieces` (blob.rs:2302) with `walk_fixed!` over 64-record windows;
- `scan_either` (blob.rs:2412);
- `SCAN_REST` (db.rs:12459) and `scan_rested` (db.rs:20722): a scan of no limit is cut every 1024 keys over one pinned state.

**Bugs:**
- Scans ran at 0.25-0.45x of the twin before they were given windows.
- A scan of no limit was one pin and held the cache past its budget.

**Not yet done:**
- the rank walks (sparse forms, merge path, cursor) still ask by the record (ROADMAP "in its cache at the plain rate").

**Under Pagefile:** stays. The rank-walk item is not addressed.

### 1.7 Snapshots (eb377fe4, 55c3e526)

**F:**
- `page_snap` (db.rs:15549);
- `Slot.page` (db.rs:9755), `oldest_lent` (db.rs:9985);
- `begin_slow` (db.rs:18278);
- a snapshot handle is pinned for its life, so it says at each read what it holds from.

**Bugs:**
- Four of the seventeen mutants at 55c3e526 were in the snapshot path, because no test had read through a snapshot under a cipher.
- One test hung under its mutant: a reader parked in a callback, with the failure path holding the end that lets it go.

**Under Pagefile:** stays, since the pager's protocol is kept (design §4 "Read and miss").

### 1.8 Merges (eb377fe4, 38567647, 55c3e526)

**F:**
- `MergeRest`/`NoRest`/`&pager::Hold` (db.rs:11982-12008) and `MERGE_REST` 256 (db.rs:12465);
- `merge_ranks` re-asks the keys after a rest;
- `vault::hold` (vault.rs:2227) at three sites;
- `vault::check` (vault.rs:2217) before naming outputs, at db.rs:12865 and 13033;
- `Emitter::leave` calls `vault::persist_one` per finished piece (db.rs:12558-12575);
- `same_blocks` (db.rs:30638) rests, and answers no once crowded.

**Bugs:**
- A merge kept every output in memory until its last.
- Merges read at the cipher's rate filled the cache, which led to `Pager::breathe`.
- `same_blocks` read most of both partitions on one pin (50 ms an eighth-size cache).
- A merge that read no values for a key wrote it without them and deleted its inputs, hence `check` before naming.
- d0816fce: a merge wrote keys out of order. The cause was the pager race, not the merge.

**Under Pagefile:**
- `persist_one` in `Emitter::leave` goes. Write-through makes the output givable once home (design §4).
- The rests, `Hold`, `check`, the `same_blocks` rest and `breathe` all **stay**.

### 1.9 The block cache's builders and the writer's waits (38567647, 55c3e526)

**F:**
- `build_rest` (db.rs:20664);
- `REST_HERE` (db.rs:15637), `AHEAD_REST` (db.rs:15642), `ahead_look`;
- `look_mask` (db.rs:18237): a look every block under a cipher, against every 64 blocks;
- `WRITER_OP` (db.rs:16367): a rest only inside the writer's own operation, never under a caller's callback;
- `writer_rests` (db.rs:18215);
- `unlent`/`unlend` (db.rs:20606, 20696; about 11 and 5 sites): `sync_wal`, `join_upkeep`, `hold_within`, `hold_for_upkeep`, flush and settle;
- `paged_begin`/`end_slow`/`Entered::finish` (db.rs:18176, 18134);
- `ATTN_PAGED` (db.rs:15624), riding the census byte.

**E:**
- `Pager::crowded`/`swamped`/`breathe`/`nudge`/`could_give`.

**Bugs (the six of 38567647, each found by a count):**
- builders gave up under a crowded cache;
- a durable commit's fdatasync was pinned;
- the settle was pinned while waiting for an upkeep pass, which waited for the evictor: three waits in a ring;
- the fill's 64 blocks between looks;
- a descheduled pin;
- evictor lock contention.

Three more mutants survived the first tests at 55c3e526:
- the writer's own fill did not rest;
- the forms' word was left plain;
- a fill that looks every 64 blocks under a cipher.

**Under Pagefile:** **all stay**. They are the explicit-ask pager's discipline, and the design keeps the pager. Every new background job that reads segments must:
- pin a slot or take a `Hold`;
- rest between units of work, at a place where every local is a copy;
- wrap every wait for a thread or the device in `unlent`;
- be counted on the plain path.

### 1.10 Pager internals (eb377fe4, 38567647, d0816fce, b4d555bc)

**E only:**
- the chunk and unit states, grace, `Hold`, `breathe`;
- `check`/`poison`;
- `SUPDB_PAGER_GUARD`.

**Bugs:**
- a reader waited for the evictor holding a busy chunk;
- the reader table was handed over after threads started;
- the 8 MB margin under a 2 MB budget;
- a chunk lent out of a unit marked twice (d0816fce, 168 lines).

**Under Pagefile:** stays, in the layer's encryption code. These are not form-driven: a new form adds none of them. The unit becomes 256 pages, and `Per` and the 4080 math go.

### 1.11 Ordered ingest, the direct run (15d0335e, 0ea02c5f)

**Switched off:** **yes**, `direct_ingest = false` in `under_cipher` from 15d0335e until 0ea02c5f.

**F (db.rs 155/41):**
- `log_run` (db.rs:28862): every run commit written to the WAL as well;
- `RUN_LOGGED` (db.rs:11520), in the `MemTable::after` byte;
- `Direct::logged`;
- the segment named at the log's sequence;
- `close_run` writes the envelope on the run's thread.

**E (vault 150/7):**
- `open_mapped` (vault.rs:1977), `Mapped`, `MappedFile::owed` (vault.rs:609): the run's memfd is held while its table maps it.

**Bugs:**
- The reads after the close decrypted what had just been written, because the vault left the file at its envelope.
- The envelope was left to the manifest, and every test still passed (`CipherCounts::late`).
- The log did not rotate under a run, and every test still passed.
- The plain cost: the commit cloned two `Arc`s, which is plain-path churn.

**Cost:** 0.68x durable and 0.52-0.57x buffered against the plain run, with 2.5 device bytes per byte stored.

**Under Pagefile:**
- S4 deletes `log_run`/`RUN_LOGGED`/`Direct::logged`: the run temp becomes the Log class, with `finish()` turning it into a trusted Once file.
- S4 is *gated* (experiments.md, "Stages"). E5 measured the commit-side gain at about zero at 133 KB, and decision 1 gains option (c), keep `log_run`. So the honest answer is: gone *if* S4 is built and pays.
- `MappedFile::owed` **stays**, renamed as the backing of a mapped file being written (design §2).

S3 adds two **new** cipher-specific engine branches (experiments.md, S3 gaps):
- skip the run temp's per-commit `d.w.sync()` while the run is logged (db.rs:28800-28807);
- do not `recover_direct` a logged run's temp at open (db.rs:27717-27770).

So between S3 and S4 there is more run-under-cipher code, not less.

New coupling: the Log class allows at most **4 overrides** per file. The run's finish patches pages 0 and 1 (db.rs:7727, 7818), and the WAL's chain byte is the third. A future change to the run's finish that patches a third page of its own breaks a counting test. A form change to the segment trailer's layout is therefore checked against a layer constant, which is form-knows-cipher.

Also new: the engine must call `finish()` before the run's final `sync_all` (db.rs:29005). That is one generic call, but a new appended file kind that is later trusted must call it too.

### 1.12 The ordered index (eb377fe4, 55c3e526, 15d0335e)

**F (ordindex.rs 86/19 at eb377fe4, 10/2 at 55c3e526):**
- The whole-file CRC check is **skipped** for paged sources (ordindex.rs:413-428).
- The top level is built late (`top_late`, ordindex.rs:395, 498), not at open.

**E:**
- `Kind::Ord` (`.o`), which shares the segment's token and key;
- persisted with its segment by `persist_one`.

**Switched off:** no. But the CRC check, a damage check, was silently off under a cipher. That is a reduced guarantee nobody wrote down as a switch-off.

**Under Pagefile:**
- `Kind::Ord` goes. The index is the Regen class: "verified whole at every open", by the piece, through the pager.
- That verification is **new engine code** in the form's open (design §4 Trust; §6 "about 10 lines" replacing the skip).
- `top_late` stays.

So a form change to the index still meets a cipher branch at its open. The design turns a silent skip into a by-the-piece CRC.

### 1.13 The compact form (b4d555bc, 81ca5d3e)

**Switched off:** **yes, twice.**
- `zpages = Off` in `under_cipher` from 15d0335e to b4d555bc.
- Then the controller, at 81ca5d3e: `zpages_under_cipher` (db.rs:3782, default false) and `cipher_controller` (db.rs:11529). `Auto` reads as Off under a cipher.

| part | E or F | code | lines | under Pagefile |
|---|---|---|---|---|
| builder reads by the piece | F | `PieceImages` over `flatindex::Pieces::image` (zfile.rs:4397); `Walked` with `Mapped`/`Held` (zfile.rs:4421, 4521); `SegSrc::Kept` and `open_seg` (zfile.rs:4197-4203, 4300-4320) under `vault::hold` | most of zfile 425/118, flatindex +23, blob +85 (`record_pieces`, `read_exts_inline`, `z_facts_in` over `Sect`) | **stays**. It is selected by `vault::paged(path)` instead of `vault::kept` (design §6 Stays) |
| attach and sampler take holds | F | | | **stays** |
| form file as a third envelope kind | E | `x-<token>.z`, `Kind::Form`, `companion` (vault.rs:1489, 2188), `persist_form` with `cold` (vault.rs:1425, 2146), written under a temp name and renamed over itself, the sweep's companion rules, the remove cascade | about 300 of vault's 367 | **gone**. It is the Regen-lazy class under its logical name, and a rename-over gets a `sync_dir` in the layer |
| advice forwarded | E and F | `bytes::Paging::resident/forget/willneed`, `MmapBytes::paged_resident` and `stored_pages` (pager 220, bytes 45) | about 265 | **stays, changed**: it acts on the Handle's ciphertext mapping and descriptor, not a path. The read-side trait remains, and a new advice call is still a `Paging` method |
| damage poisons the store | E (pager `check`) | | | **changes**: a Regen-lazy failure is the file's (design §4 Trust). It needs every `page_bytes → None` site to call `ZFile::fail`. E0 item 6 read the sites and found support |
| controller | F (db.rs 44/2) | | | **stays** until S5, which is keyed on E1c (not run) and S3's pairings |
| plain-cost fixes | F | `read_exts_inline`, `view_of` inlined | | stays; a consequence of 1.5 |

**Bugs:**
- An envelope written in place: there was no test that a form file is written twice. Held by a vault unit test.
- The footprint was read from a section held whole, which is **zero bytes under a cipher**, so the model said every store fits. This is the read-side ask path: any code that sizes from `as_slice()` reads 0 under `Pg`. **It stays as a bug class.**
- The probe for an earlier open's form files passed without the store learning of them. That is form-file-kind code, and it goes.
- An attach that holds nothing hung the writer: a debug assert on the segment work's thread. That is `Hold` discipline, and it stays.
- The 4096-byte page across two 4080-byte chunks gave 0.73x, priced last. The straddle goes with whole-page chunks. But E1b says the coding's file size is a co-equal cause, and E7 says out of core about 69% of the cipher's gap is device faults from a bound split between plaintext and ciphertext, which the design keeps.

**Not addressed:**
- The controller's model of "two caches in series" (the plaintext budget defaulting to half of `z_budget`, vault.rs:637-646) and the coding sized against the whole budget (db.rs:39798-39810).
- This is model-assumes-page-cache: every future out-of-core form whose sizing assumes the kernel's cache is the reader's cache will be mis-sized under a cipher, and nothing in Pagefile fixes that.

### 1.14 Advice and residency outside the form (b4d555bc, 38567647)

These are the same forwarding as in 1.13: the build's ask-ahead and drop-behind, the drop of raw pages, the swap at the crossing, `records_resident` (db.rs:8224), `pieces_willneed`, `blocks_willneed`.

**Under Pagefile:** stays and changes, as in 1.13. Design §2 also routes the raw-descriptor advice sites through `vault::File::advise` and `writeback`. Every new advice a form adds is a door call and a `Paging` method, two places.

### 1.15 The fuzz and the tests

- **Fuzz:** `ZFUZZ_CIPHER` (15d0335e, fuzz.rs 54), `ZFUZZ_CIPHERCACHE` (eb377fe4, 74), `ZFUZZ_RECYCLE` (17c363a5, 6), and the form forced under the cipher (b4d555bc, 43).
- **`tests/cipher.rs`:** about 2,100 lines over seven commits.
- **Mutants:** about 130 across the commits; 40, 17, 6, 10, 24 and others.

**Under Pagefile:**
- The fuzz stays.
- It **gains** a fault-injection mode in the door, because a dropped `Pg` store keeps its files in the page cache and the fuzz's crash model weakens (design §5).
- `tests/cipher.rs` tests that find files by stored name are rewritten through `vault::stored_name` (design §10 table).

## 2. Replay under Pagefile: had the design existed first

**Would not have been needed at all:**
- the memfd table and the envelope-after-the-fact (1.2), with its bugs: the two views, the fadvise drop and `could_give`'s unbacked files;
- tokens and the sealed manifest's token list (1.3, S3);
- `Kind::Ord`/`Kind::Form`, companions and the form file's envelope writing (1.12, 1.13);
- `Emitter::leave`'s `persist_one` (1.8);
- `CipherCounts::late` and its mutant (1.11);
- the WAL's `LogCipher` and the recycled-log work (1.4), **only if S6 lands**;
- `log_run`/`RUN_LOGGED` (1.11), **only if S4 lands**, and S4 is gated on a gain E5 did not find;
- the 4080/4096 straddle and `Per`.

Three of the four switch-offs were caused by the write side: the run, the recycled log, and "attached by its bytes in the page cache", half of the form's switch-off. Had Pagefile existed, those three would not have been switched off for that reason.

**Would still have been needed, with the reason:**
- **The whole read side (1.5-1.10).** The design keeps the explicit-ask pager, so the plaintext is never an ordinary mapping. That is the rule, not the exception: every reader must work over a source that lends nothing whole, and every thread that reads must pin or hold, rest, wait unlent, and check before naming. This was the bulk of the lines (eb377fe4 about 3,900, 55c3e526 about 920, 38567647 about 290, b4d555bc's zfile, pager and bytes about 700) and the bulk of the bugs: the six pins, d0816fce, the four pager bugs, the footprint at zero, the attach hang, the snapshot mutants, and every plain-cost regression.
- **The other half of the form's switch-off,** "built from a partition's records borrowed whole". It would recur for the next feature that borrows a section whole.
- **The controller switch-off.** The model under a cipher is unvalidated, and E7 puts the dominant cost in the bound split, which Pagefile keeps.
- **`MappedFile::owed`,** for any file mapped while written.

**Changed rather than removed:**
- the ordered index's CRC: skipped, becomes by the piece at open;
- the advice: by path or envelope, becomes by Handle mapping;
- form damage: poisons, becomes the file's (needs a site audit);
- the controller: off, becomes conditional at S5.

**New work the design itself brings to form authors:**
- choose one of five classes at create (`SegmentOptions` field, `vault::create(path, Class)`);
- call `finish()` on an appended file before its final sync;
- stay within 4 overrides per Log file;
- keep logical names within the sealed-name budget: two characters under NAME_MAX at the longest `par-` name (design §3), so lengthening `FENCE_MAX` or a name format breaks it, and only under a cipher;
- route every fd-taking site through `vault::File` (`as_os()` for plain-only sites);
- the `manifest_slots: false` refusal;
- S3's two branches in the run;
- a Regen file is verified whole at open, so a new regenerable kind either pays that or takes the Regen-lazy discipline: every read site treats `None` as the form's failure;
- the `File` enum widening is a plain-cost risk in `AlignedWriter`/`Wal`/`SegmentWriter` (design §7), counted by the S1 rows.

## 3. The count

I list 40 discrete changes that features forced on the branch. Each is a mechanism plus its bug fixes, not a line. The list:
- **Write side (W1-W15):**
  - W1 door;
  - W2 memfd plus envelope after;
  - W3 tokens and sweep;
  - W4 manifest-image envelopes and `late`;
  - W5 sealed slot;
  - W6 `LogCipher`;
  - W7 recycle;
  - W8 `log_run`;
  - W9 `Mapped`/`owed`;
  - W10 form-file kind;
  - W11 ord kind;
  - W12 `Emitter::leave` persist;
  - W13 the FADV drop;
  - W14 two views;
  - W15 `under_cipher`.
- **Read side (R16-R35):**
  - R16 `Bytes` piecewise;
  - R17 `Sect`/`Pieces`;
  - R18 `Sec`/`indexed!`/`not_lent`;
  - R19 `scan_pieces`/`either`;
  - R20 rank walks by record;
  - R21 `SCAN_REST`;
  - R22 snapshot idle;
  - R23 merge rests and `Hold`;
  - R24 merge `check`;
  - R25 `same_blocks` rest;
  - R26 builders' rests and looks;
  - R27 `unlent`/`unlend`;
  - R28 `ATTN_PAGED` and `paged_begin`/`end_slow`;
  - R29 pager internals and their bugs;
  - R30 the open's holds and rests;
  - R31 ordindex CRC skip and `top_late`;
  - R32 form builder by piece (`Walked`/`Held`);
  - R33 attach and sampler holds;
  - R34 advice forwarded;
  - R35 form damage poisons.
- **Controller (C36-C37):** C36 controller off; C37 model and budget split.
- **Plain cost (P38):** the 55c3e526 restructuring.
- **Tests (T39-T40):** T39 the fuzz modes; T40 `tests/cipher.rs`.

| fate under Pagefile | count | which |
|---|---|---|
| gone outright (S3) | 7 | W2, W3, W4, W10, W11, W12, W13. W14 goes too, with W2's table. Folded in, that makes 8 |
| gone conditionally | 4 | W5, W6, W7 (S6, decision 2); W8 (S4, gated by E5 and decision 1c) |
| already gone on the branch | 1 | W15 (b4d555bc), except the controller (C36) |
| stays as is | 21 | W1 (widened), W9, R16-R19, R21-R30, R32, R33, P38, T39 (widened), T40 (rewritten) |
| changes | 5 | R31 (CRC by piece), R34 (advice by Handle), R35 (form damage is the file's), C36 (conditional at S5), T39's fault-injection |
| not addressed | 2 | R20 (rank walks), C37 (two caches in series; E7) |

The rows overlap at W14 (counted in the first row's note) and T39 (it both stays and gains fault injection).

**By kind,** the 23 that stay or are not addressed:
- **Discipline a form author must follow:** 17. These are W1's door, R16-R28 except R29, R30, R32 and R33. R20 is the open performance debt of the same discipline.
- **Model assumes the page cache:** 1. This is C37. C36, which changes conditionally, is the same kind.
- **Encryption-internal:** 2. These are R29 and W9. A form adds none of these.
- **Plain cost to recount at each reader change:** 1. This is P38.
- **Tests:** 2. These are T39 and T40.

The ones that go are almost all cipher-knows-form: the vault knowing segment, `.o`, `.z`, tokens, the run and the WAL. The ones that stay are almost all form-knows-cipher discipline on the read side.

**Bottom line for the owner's requirement.** Under Pagefile, a change to a form's bytes needs no edit in the encryption modules. That is the real gain, and it is the half that caused three of the four switch-offs. A change to how a form is read does not get that gain: a new reader, a new background job over segments, a new wait, a new sizing model or a new advice. It still has to be written against the explicit-ask pager (ask by the piece, hold or pin, rest, unlent waits, check, plain cost counted). It still has to be tested under a cipher with a small cache, because forgetting any of these is silent in the clear and in core:
- wrong answers or zero-sized reads under `Pg`;
- a cache held past its budget;
- debug-only asserts;
- hangs.

The design also adds a few new form-knows-cipher constants: the 4-override bound, the sealed-name length budget, the class at create, and `finish()`.

Not verified: every line count above is from `--numstat` attributed by reading. Whether S4, S6 and S7 land is open (decisions 1-2, E3, E1c not run). Whether the S3 gaps are as described is by reading only (experiments.md says the same).
