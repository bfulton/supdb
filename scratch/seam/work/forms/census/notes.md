# Coupling census: forms and encryption, at 3df85d22

The question: "if I change a form, what encryption code do I have to touch or think about, and if I change encryption, what form code?"

Method: reading and grep only. Nothing was built, run or timed. Line numbers are from the tree at `3df85d22`. "Not verified" marks what was not read.

How each coupling is tagged:
- **Direction.** E→F means encryption code names or assumes a form. F→E means form code names or assumes encryption.
- **Pagefile.** What the design in `seam/design.md`, as amended by `seam/experiments.md`, does to it: GONE, STAYS, CHANGES or NEW.
- **Forget.** What happens today when a form author forgets it.

## 0. The history

Each cipher feature changed both sides of the seam. From `git show --stat`, lines changed in each file:

| commit | feature | encryption side | form side |
|---|---|---|---|
| `eb377fe4` | store larger than memory, readers by the piece | pager.rs 1648, vault.rs 591, bytes.rs 248 | blob.rs 509, flatindex.rs 398, ordindex.rs 105, db.rs 603, zfile.rs 29 |
| `38567647` | out of its cache: rests, holds, two lines | pager.rs 104, vault.rs 51 | db.rs 205 |
| `55c3e526` | the clear store pays nothing | vault.rs 6 | blob.rs 344, db.rs 906, ordindex.rs 12 |
| `17c363a5` | recycled log | vault.rs 12 | db.rs 73 |
| `0ea02c5f` | ordered ingest under a cipher | vault.rs 157 | db.rs 196 |
| `b4d555bc` | compact form under a cipher | vault.rs 406, pager.rs 220, bytes.rs 47 | zfile.rs 543, blob.rs 88, db.rs 148, flatindex.rs 23 |
| `81ca5d3e` | controller off under a cipher | – | db.rs 46 |

So the coupling runs both ways at every feature. It is not just that forms are written as they are. The vault grew form vocabulary (`Kind`, companion, `persist_form`, `LogCipher`), and the forms grew pager vocabulary (pieces, holds, rests, `Walked::Held`).

---

## 1. Encryption code that names or assumes a form (E→F)

| # | what | where | Pagefile | forget / break |
|---|---|---|---|---|
| E1 | `Kind {Seg, Ord, Form}` and the stored suffixes `""`, `.o`, `.z`. `disk_parse` recognises exactly those three. The ordered index and the form share their segment's token and key. | vault.rs:468-484, 505-513, 520-538 | GONE (S3) | A new derived file kind (a bloom file, a second form) that is not added here is "kept" in memory and lost at the close. The vault's own doc says so, and CLAUDE.md: "safe before anyone has thought about it, and lost at the close until someone has". No test fails unless a cipher test reopens and reads the new file. For a regenerable file the loss is a silent rebuild; for a durable one, data loss. |
| E2 | `passes()`: `wal-`, `spare-` and `manifest*` go to the directory under their plain names; every other name is kept. | vault.rs:562-564 | STAYS until S6, then GONE | A new file the engine appends to or rewrites in place (a second log, a new slot file) is kept as a write-once memfd. `rewrite` refuses it once it has an envelope (vault.rs:1090-1102, PermissionDenied). A rename of the log family breaks `passes` silently. |
| E3 | `persist_one` calls `crate::db::Db::ord_name_for(seg)`: the vault calls a db naming function. `persist`/`persist_with` take `(seg, Option<ord>)` pairs built by `Db::with_ords`. | vault.rs:2129-2139, 1291-1380; db.rs:27256-27267 | GONE (S3) | A change to the ordered index's naming, or a second companion file, must be threaded here. If not, the companion stays a memfd and is lost at the close. The ordered index is recomposed at the next open, so only the cost shows. |
| E4 | `persist_form` and `companion`: the form file is written as an envelope under its segment's token, as a temp name renamed over (because a form is rewritten while a view holds it), with a `cold` mode that mirrors zfile's `drop_output`. | vault.rs:1395-1470, 1489-1510; called at zfile.rs:4373, db.rs:31411, 31455 | GONE (S3; `cold` becomes `writeback(Wait::Both)` and a drop) | A new form file kind, or a form rewritten in place, needs vault code. Two of `b4d555bc`'s 24 mutants outlived the first tests here (CLAUDE.md). |
| E5 | `remove` cascades a segment's removal to its `.z`; `sweep` counts a `.z` as held by its segment. | vault.rs:993-1007, 1612-1617 | GONE | A new companion is not removed with its segment. It leaks on disk until the sweep, and the sweep then removes it because nothing holds it. |
| E6 | Manifest contents: `seal_manifest`/`open_manifest` seal `(token, wrapped)` per segment the manifest names. `adopt_all` refuses a count mismatch against `with_ords(m.names)`. | vault.rs:1575-1590, 1633-1685; db.rs:11661-11690 | GONE at S3 (no tokens in the manifest). The slot's sealing stays until S6 (Replace class). | A manifest that names a file which is not a segment, or a segment with no envelope, is a vault change. "Every segment the manifest names has its envelope written here" ties the manifest's contents to the vault's durability. |
| E7 | `LogCipher`: the WAL sealed record by record, with AAD `id\|at\|len`. The engine's WAL carries an `enc` field, the `SUPDBWE` header, `WAL_SEALED_LEN`, cipher arms in `begin`/`resume`/`open_append`/`write`/`replay`, and the `recycles` exception. | vault.rs:1686-1700, 1773-1860; db.rs:4103-4140, 4203-4240, 4325-4340, 4447-4450, 4553-4580 | STAYS until S6, then GONE (decision 2a). Decision 2b keeps it for good. | Any change to the WAL's framing, rotation, header kinds or recycling has a cipher twin. `17c363a5` (recycled log) was 12 lines of vault and 73 of db.rs. Under a WAL write failure the cipher replay maps the length to a different place, and a second reopen refuses "wal sequence gap" (experiments E0 item 5). |
| E8 | `MappedFile`/`Mapped`/`open_mapped`: the vault holds the run's memfd because a writer maps it. `RunMap` holds the `Mapped` guard. | vault.rs:598-635, 1039-1060, 1977-2005; db.rs:11436-11470 | STAYS, renamed as the mapped backing (design §2: "cannot go") | A new form that reads a file it is still writing through its own mapping must use `open_mapped`. If it does not, the pager may give the memfd's pages back under the mapping, and it reads zeroes (wrong answer). Under Pagefile the same holds for the write-through backing. |
| E9 | Write-once rule: a kept file is refused for `open_write` once it has an envelope or a page, and `open` of a finished file is `Unsupported`. | vault.rs:1090-1125, 1113 | GONE (Once/Log/Replace classes allow patches) | A form that reopens a finished file, to patch it or read it with `File` I/O, errors under a cipher only: an error in whatever cipher test reaches it. |
| E10 | Pager geometry: `CHUNK = (1<<12) - TAG = 4080`; a unit is 256 chunks in 255 pages; the `Per` divide. | pager.rs:108-125, test at 1766 | CHANGES: 4096 for `Pg` files, 4080 kept for envelopes until S6 | This is an assumption against the forms' page size. `ZPAGE` is 4096 (zfile.rs), so every form page lies across two chunks. The compact form under a bound read 0.73x (`b4d555bc`/`81ca5d3e`), which is the whole reason the controller is off under a cipher (F20). Any future form with 4 KiB units hits the same straddle while envelopes exist. |
| E11 | `crypt.rs` reader: "a thread remembers eight chunks" because a value in a block straddles two chunks. The default chunk shift was chosen on a segment's fault count. | crypt.rs:530-560, 120-125 | STAYS (crypt v1 frozen) | A tuning tied to the segment format's block layout. Only `crypt` users (finished-file envelopes, the browser) are affected, not the live store. Low risk. |
| E12 | `Hold` and its docs speak of segments and merges: a hold is for threads with no reader slot. | pager.rs:141-180 | STAYS | – (vocabulary only) |
| E13 | `Vault::sweep` sweeps `x-*.tmp` and any unheld `x-` envelope at open. | vault.rs:1593-1630 | GONE. Replaced by the engine's own `.tmp` sweep by logical name (a plain behaviour change) and "never delete a name that fails to authenticate". | – |
| E14 | `CipherCounts::late` counts the envelopes a manifest call had to write. A test holds it at none for a run (form behaviour seen through an encryption counter). | vault.rs:1720-1750; tests/cipher.rs:1353 | GONE | – |

## 2. Form code that names or assumes encryption (F→E)

### 2a. Read side: the readers (blob, flatindex, bytes, ordindex)

| # | what | where | Pagefile | forget / break |
|---|---|---|---|---|
| F1 | `Bytes::piecewise`, `lend_at`, `ASK_PIECES`; `MmapBytes::paged` with `lend: 0`; `slice_slow`, `Paging` (`resident`, `forget`, `willneed`). | bytes.rs:79-105, 300-330, 365-420, 481-492, 505-632 | STAYS (the "readers that ask" are kept; WARM at S7 sets `lend = len` while the store fits) | A new byte-source method has to know about the `Paged` arm. Every advice method in bytes.rs has one (`advise_dontneed_range` 530, `resident_at` 561, `advise_dontneed` 580, `advise_willneed` 604). A new advice written on `self.lend`/`map()` alone is a silent no-op under a cipher. |
| F2 | `Sect` trait, with `&[u8]` and `Pieces` implementations: `Pieces::window`, `RECORD_WINDOW` 16 MiB, `image`. About 23 functions in flatindex are generic over `S: Sect` (readpath.md; not recounted). | flatindex.rs:611-760 | STAYS | A new index reader written against `&[u8]` compiles and works in the clear, and has no path under a cipher. The caller then gets `flat() == None`, see F3. Caught only if a cipher test or `ZFUZZ_CIPHER=1` reaches the path. `tests/blob.rs`'s exact-range source holds the existing readers. |
| F3 | `Sec::{Lent}` arms; `Sec::get`/`not_lent`; `Blob::flat` against `Blob::pieces`; the `indexed!` macro (flat first, then pieces under `cold_path`, 8 sites). | blob.rs:475-560, 941-1000, 1018-1040 | STAYS | A new point-read accessor that calls `flat()` only answers `None` under a cipher. That is an absent key, a wrong answer with nothing raised, unless the fuzz reaches it. |
| F4 | The scan's two walks: `scan_either` → `scan_whole` → `Err(by_the_piece())` → `scan_refused` → `scan_pieces` (with `walk_fixed!`) → `scan_records`. `scan_at`'s `B::ASK_PIECES && piecewise()`. Callers at db.rs:19759, 19813, 19944. | blob.rs:2291-2530 | STAYS | A new walk needs a by-piece twin and has to be named so it costs the plain loop nothing. CLAUDE.md built and counted six shapes of `scan_at` before one read level; naming the second callee inside the first cost 3-4 instructions an entry. Forgetting the twin gives `Unsupported` at runtime under a cipher. Forgetting the plain cost is a slowdown no pairing resolves; only callgrind by function sees it. |
| F5 | `record_regions` against `record_pieces`; `record_region_span` against `record_region_place`; `read_exts_inline` (an inlining split forced by the second fetch path). | blob.rs:1379-1430, 1918 | STAYS | `record_region_span` returns `None` under a cipher. Every caller needs a cipher branch, as `z_cross` has (db.rs:40381-40393). |
| F6 | **Silent no-ops under a cipher:** `willneed_ranks` and `willneed_dir` start with `self.flat()?` and return 0. | blob.rs:1239-1290 | STAYS (not addressed by the design) | `Options::pieces_willneed`, `blocks_willneed` and `pieces_dir_willneed` (db.rs:3149-3285) do nothing for a store under a cipher, and no test or counter says so. This is the "slowdown nobody sees" case, present today. `OrdIndex::willneed_heads` (ordindex.rs:730-740) does go through `resident_at`/`advise_willneed`, so it forwards. |
| F7 | Chunk CRC by the piece in the block table check. | blob.rs:1742-1757 | STAYS | – |
| F8 | `OrdIndex::open` skips the whole-file CRC when `piecewise()`, and its top level is deferred to the first seek. `head_absent` answers 0 for a head that did not read; the store's `check` is relied on to replace the answer. | ordindex.rs:404-430, 505-520 | CHANGES: the CRC goes through the pager at open (Regen class verify) | A new ordered-index field read at open from `as_slice()` reads an empty slice under a cipher. A new reader of heads that does not go through `head_absent` takes 0 as a real head: a wrong rank, covered only if the op ends in `end_slow`. |

### 2b. Read side: the engine (db.rs)

| # | what | where | Pagefile | forget / break |
|---|---|---|---|---|
| F9 | `ATTN_PAGED` in the census byte; `paged_bits`, `paged_begin`, `begin_slow`, `Entered::finish`, `end_slow`, `leave_slow`, `page_snap`. | db.rs:15601-15624, 16344-16356, 18123-18290, 18573-18622 | STAYS | A new public read operation that does not go through `Entered`/`leave` never has its answer replaced by `Pager::check`. A failed tag then answers "absent" (wrong answer). It never waits at `swamped` either (memory past budget). |
| F10 | Pager pins in the reader table: `PagerGrace`, `Slot.page`, `oldest_lent`, `page_pin`/`page_set`/`page_idle`/`page_back`, `debug_pinned`. | db.rs:9652-10160 | STAYS | – (infrastructure, not per form) |
| F11 | `unlent(..)` and `unlend()`: about 25 call sites. Examples: `sync_wal` 27058-27063; merge and seal joins 37645, 38333, 41680; parks 29741, 30970, 33880-33935, 37119, 37609; flush and settle 37898, 38377, 38877, 38886. | db.rs:20606, 20696 | STAYS | A new wait (a join, a park, a device sync) made under a pin holds the cache shut. The evictor waits, the cache overshoots, and readers past `swamped` stall; in a ring it is a timeout. CLAUDE.md: a durable commit's fdatasync held the cache shut. Nothing fails. `PagerCounts::longest_wait_us` shows it if someone looks, and the tests that hold peak to budget may fail. |
| F12 | Rests: `SCAN_REST = 1024` with `scan_rested` (20709-20775); `MERGE_REST = 256` with `MergeRest`/`NoRest`/`&pager::Hold` (11979-12008; `merge_ranks` 12291 re-asks keys after a rest; `merge_keys` 12368; `compact_run`/`tier_walk` generic 12619, 12949); `build_rest`/`build_rest_unless` (20640-20690); `REST_HERE`, `writer_rests`, `not_asked` (15633-15640, 18215-18230); `look_mask` (18237); `AHEAD_REST`/`ahead_look` (15642, 30808-30860); `abandon_asked` (21701-21760, used at 21930, 23509, 23651, 23679, 24205, 33467); `same_blocks` rest (30630-30640). | db.rs as listed | STAYS | A new long walk (job, builder, scan, comparison) with no rest fills the cache past budget and other threads wait. A rest placed where a borrow is still held reads given-back pages, which read as zeroes: a wrong answer. `SUPDB_PAGER_GUARD` (debug) turns that into a fault, and the fuzz under a 16 KiB cache finds it if it reaches the path. A rest under a caller's callback rests the scan that lent the callback its bytes; `WRITER_OP` (16367) is the guard for the writer's fill. |
| F13 | `vault::hold` for threads with no slot: merges (11970, 12943), the open (27614-27615, rests at 27813/27844, not verified), the sampler (39666-39680), zfile attach (zfile.rs:2003), zfile build (zfile.rs:4309). | as listed | STAYS | A new background thread that reads a store's files without a hold panics in debug (`Paged::ensure`, `PINS>0`, pager.rs:1316). In release it reads given-back pages, a wrong answer. A mutant of exactly this hung a test, because the writer waits in `land_seal` for a dead thread (CLAUDE.md, ROADMAP.md). |
| F14 | `vault::check` before naming a merge's outputs. | db.rs:12865, 13033 | STAYS | A new job that writes outputs from reads and deletes its inputs, without `check`, writes keys without values and deletes the originals. That is silent data loss; `tests/cipher.rs`'s changed-byte property catches it only for jobs it drives. |
| F15 | `vault::map` for every segment open and for form files (8308, 12918, 40389). | db.rs | STAYS | A new reader that maps a file with `memmap2` directly is caught by the door test in db.rs only (F27). |

### 2c. Read side: the compact form (zfile.rs)

| # | what | where | Pagefile | forget / break |
|---|---|---|---|---|
| F16 | The attach takes a hold, maps through `vault::map`, and has `file = vault::open(path).ok()`, which is `None` under a cipher, so `drop_cache` does only the `madvise` arm. | zfile.rs:1995-2030, 3248-3260 | STAYS (hold, map); CHANGES (fd advice goes through `File::advise`) | – |
| F17 | `warm_file` reads `map.as_slice().len()`, which is 0 under a cipher, so the warm is a no-op. | zfile.rs:3774-3810 | STAYS until WARM. The S7 note says `warm_file` must use `ensure` (experiments E0). | Silent no-op: `supdb-zwarm` does nothing under a cipher. |
| F18 | `SegSrc::{File, Kept}` and `open_seg` via `vault::kept`; the `Kept` build arm; `PieceImages`; the `Walked` trait with `Mapped` (plain) and `Held` (cipher: willneed ahead, dontneed behind, `hold.rest()` per page). | zfile.rs:4193-4210, 4296-4360, 4393-4600 | STAYS, selected by `vault::paged(path)` (design §6) | A change to how the build reads records (a new image source, a second pass, a read-back of a written page) needs both arms. The held arm must rest only where nothing lent is reread. Omitting the `Held` arm's rest makes a big partition overshoot the cache. |
| F19 | The sampler's two arms: `record_regions` against `PieceImages`. | zfile.rs:5055-5090 | STAYS | as F18 |
| F20 | `persist_form` called after the rename; `plan.sync` via `vault::open(out)`; the build's `File` sinks. | zfile.rs:4360-4390, 4607 | GONE: `persist_form` goes. `Regen-lazy` is chosen at create. | – |
| F21 | Plain-only fd advice: `drop_written_range` (`sync_file_range` + `fadvise`), `swap_ahead` (bare `File::open(form)` at 3972, outside the door test), `drop_behind`. | zfile.rs:3915-4046 | CHANGES: routed through `File::writeback`/`advise`; the door test is widened to zfile.rs | Under a cipher these are not reached (`z_cross` branches), or they receive a `File` that `vault::open` could not give (`Unsupported`) and do nothing. A new advice written this way is a silent no-op under a cipher. A bare `File::open` in the decoy directory fails "not a directory" and, if the error is swallowed, the optimisation is silently off. The test bars this only in db.rs. |

### 2d. Read side: the engine's advice sites

| # | what | where | Pagefile | forget / break |
|---|---|---|---|---|
| F22 | `records_resident`: `resident_share` over a whole slice, else `paged_resident` (an explicit second arm). | db.rs:8219-8233 | STAYS (residency over the ciphertext mapping) | – |
| F23 | `z_cross`: `record_region_span` is `None` under a cipher, so an explicit cipher branch drops the ciphertext and willneeds the form file's envelope. | db.rs:40360-40400 | STAYS | A new swap or crossing written on the plain branch alone is silently off under a cipher. |
| F24 | `drop_raw_pages`: `vault::open` + `fadvise_dontneed` (no-op under a cipher), plus `blob.advise_dontneed` (forwarded to `forget`). Merge drop-behind: `vault::open` → `None` under a cipher, plus `blobs[i].drop_records` (forwarded). `start_writeback` and the WAL drop use raw fds (6071, 4475). | db.rs:8188-8197, 12755-12790, 6071-6085, 14904-14931 | CHANGES: through `vault::File` | The fd half is silently dead under a cipher today; the mapping half works only because bytes.rs forwards. |

### 2e. Write side (db.rs)

| # | what | where | Pagefile | forget / break |
|---|---|---|---|---|
| F25 | The direct run under a cipher: `RUN_LOGGED`, `MemTable::logged()`, the `self.vault.is_some()` choice, `log_run` (commits written to the WAL as well), and the envelope at the run's close via `vault::persist`. | db.rs:10236-10238, 11518-11520, 28219-28221, 28834-28881, 28999 | GONE at S4 (Log class + `finish`). S4 is gated on S3's pairing; decision 1(c) keeps `log_run`. | A change to the run's commit path has a cipher twin. The rotation-under-a-run and close-envelope mutants passed every test because another mechanism covered for them (CLAUDE.md). |
| F26 | `vault::persist` where a plain store fsyncs: the seal (29525), the run close (28999), the merge (12866, 13034), the merge piece (`persist_one`, 12574), and `manifest_image` (11669-11673, persist then seal). | db.rs as listed | GONE (S3): the fsync is real, and `FINAL` is written at `finish`/sync | A new kind of output file made durable by an fsync is not durable under a cipher (a memfd fsync syncs nothing) unless `persist` is called. CLAUDE.md: "A fourth fsync of a kept file is a durability that is not there, and the list is the thing to check a change against." It is silent until a crash; the fuzz's drop catches it if the file is needed after reopen. |
| F27 | The door: every file op of db.rs goes through `vault::` (128 calls). The decoy directory makes a bypass fail "not a directory". The test `the_engine_names_no_file_but_through_the_door` reads db.rs and ordindex's `open`. | vault.rs:2876-2905 | STAYS, widened to zfile.rs, ordindex.rs, bytes.rs and pager.rs, and also barring `as_raw_fd`, `posix_fadvise` and `sync_file_range` | A bypass in db.rs is a test failure (good: mechanical). A bypass in zfile.rs is not caught; it fails at runtime under a cipher, or silently if the error is swallowed. |
| F28 | Open: `refuse_marked`, the decoy `Held::dir`, the pager installed before threads, the `opening` hold, `sweep`, `cipher_controller`. | db.rs:27296-27310, 27372, 27589-27640, 27980 | mostly STAYS; `cipher_controller` GONE at S5 if E1c/S3 say so | – |
| F29 | `cipher_controller` and `zpages_under_cipher`: `Auto` becomes `Off` under a cipher. | db.rs:3782, 4055, 11522-11533, 27305, 27604 | GONE at S5, conditional on pairings | A form feature switched off under a mode and written down as a property: exactly what the owner objects to. |
| F30 | `vault::real(&self.wal.path)`, used for a diagnostic of where the file is. | db.rs:31342-31345 | GONE at S6 | – |
| F31 | `vault::companion` at attach: the store learns of a form file left by an earlier open. | db.rs:31405-31411, 31453-31455 | GONE | Without it a form file built in an earlier open is not adopted; the store rebuilds it. A mutant passed because the attach learns too (CLAUDE.md). |

`zpages.rs` and `zselect.rs` name no encryption (grep: no `vault`, `pager`, `cipher` or `Hold`; `zpages.rs:543,613`'s `.check()` is a form check). They are already clean in the code; they are coupled only through the models of §4.

## 3. Disciplines with no name in the code

These are what a form author must do for the form to work under a cipher. None is enforced by a type.

| # | discipline | enforced today by | forget → |
|---|---|---|---|
| D1 | Ask before every read: read through `slice_at`/`Sect`/`Pieces`, never `as_slice()`, `flat()` alone or a slice held whole. | nothing at compile time; `tests/blob.rs`'s copy-exact source for the existing readers; the cipher fuzz for reached paths | an absent key (wrong answer, nothing raised), an `Unsupported` at runtime, or a silent no-op for advice (F6, F17) |
| D2 | Never hold a borrow across a rest, or across any call that may rest. | nothing. `SUPDB_PAGER_GUARD` under debug faults on given-back pages; `ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384` | zeroes read as data: a wrong answer, a merge writing a wrong key or value, a tiling refusal far from the cause (CLAUDE.md's "a chunk lent out of a unit marked twice") |
| D3 | Rest in any walk longer than a bound (1024 keys for a scan, 256 merge steps, a block for a builder, a page for a form build), and only where every local is a copy, a position or a table. | nothing; tests that hold the cache's peak to its budget (`a_store_larger_than_its_cache...`, tests/cipher.rs:372) | overshoot, waits, the builders giving up their forms (a slowdown found only by the suite's counters) |
| D4 | A thread with no reader slot takes `vault::hold`. | debug assertion `Paged::ensure` (pager.rs:1316) | debug panic; in release, zeroes; a hung test when the panicking thread is the segment work's |
| D5 | Every wait under a pin is `unlent`/`unlend` (joins, parks, device syncs, waiting on another thread). | nothing; `longest_wait_us` is a counter | cache shut while waiting; a three-way wait ring ended by a timeout (`settle`'s join) |
| D6 | A job checks `vault::check` before it names outputs built from reads. | nothing | silent loss: keys with no values, inputs deleted |
| D7 | A reader's `Option` must not turn a failed read into "absent": the operation ends in `Entered::finish`/`end_slow`, or the job checks. | `end_slow` for ops through `Reader`; nothing for a new entry point | a wrong answer from a failed tag |
| D8 | Advice and residency go through the mapping's methods (`MmapBytes::advise_*`, `resident_at`, `paged_resident`), never through a raw fd or `mincore` over `as_slice`. | nothing | silent no-op under a cipher (F21, F24) |
| D9 | No file is opened, mapped, renamed or listed except through `vault::`. | the door test (db.rs only) and the decoy directory | an error at runtime under a cipher ("not a directory"); a silently skipped optimisation if swallowed |
| D10 | A new file kind is classed in the vault (`Kind`, `persist*`, `companion`, `remove`'s cascade, `sweep`). | nothing | lost at the close |
| D11 | Wherever a plain store fsyncs a non-log file, the cipher path calls `vault::persist`. | `CipherCounts::late`, for the run only | durability that is not there |
| D12 | Nothing rewrites a file in place after it is finished, and nothing reopens one by `File`. | vault refusals at runtime | an error under a cipher |
| D13 | A file read while it is written is mapped via `open_mapped`. | nothing | given-back pages under the mapping: zeroes |
| D14 | Form units that are not chunk-sized straddle (4096 against 4080). | nothing; found by pricing a pairing | a slowdown out of core (0.73x) |
| D15 | Cipher branches on a per-key or per-entry path are priced by callgrind against `origin/main`, by function. | `bench/examples/*` probes by hand | plain stores pay instructions no pairing resolves (the 370-instruction form scan regression in `eb377fe4`, found four commits later) |
| D16 | Rests only from the writer's own op (`WRITER_OP`) or from a thread's own loop, never under a caller's callback. | nothing | a rest under the scan that lent the callback its bytes: zeroes |
| D17 | Every operation that reads ends at `pager.check` before acknowledging. A background read that discards its result (the builder ahead, db.rs:30828) is covered only by the poison's latch. | partial | a failed read believed once; then every read fails |

## 4. Models and policies that assume the kernel's page cache

| # | what | where | assumption | Pagefile | forget / break |
|---|---|---|---|---|---|
| M1 | The controller's budget: `read_budget`/`budget_from` is the cgroup's limit (or `MemTotal`) less `anon` (v2) or `rss+rss_huge` (v1), held under `MemAvailable`. | zfile.rs:5199-5270 | Memory not anonymous is page cache the form can have. The pager's memfd plaintext is shmem: it is neither anon nor reclaimable, and nothing in the budget path subtracts it. Grep found no use of the pager's `held` in zfile.rs, zselect.rs or the budget path; not verified exhaustively. | STAYS (design §11 keeps the memfd backing for exactly this) | The budget overstates by the plaintext cache under a cipher. E7: about 337 MiB of memfd beside about 240 MiB of ciphertext page cache at 5M keys under 640 MiB. |
| M2 | Census: thread major faults (`getrusage RUSAGE_THREAD`), the group's `workingset_refault`. | zfile.rs:4935-4955; zselect.rs:594, 2315-2395 | A miss is a major fault. Under a cipher a miss of the pager is a fresh memfd page fault (minor) plus a copy out of the ciphertext mapping (major only if the ciphertext is not cached). | STAYS on purpose (the copy from the mapping keeps `ru_majflt` meaningful); a `pread` arm would blind it | The model reads the kernel's cache and sees nothing of the store's own cache (CLAUDE.md: "a store that fits memory and not its own cache of plaintext ... the controller ... sees none of it"). |
| M3 | `model_raw`/`model_z`, `choose_coding(budget, ..)`, the fit rules. | zselect.rs:440-460 and the model; `Seg::foot` (db.rs:8199-8217) | One cache, sized by `budget`. Under a cipher it is two caches in series, never held to a measurement. | STAYS (open item §12) | the wrong form bought, or the right one never (0.29 faults predicted against 0.60 measured, CLAUDE.md) |
| M4 | The sampler's price of a fault: wall less CPU over its touches. | zfile.rs:4955-5100 | A touch that faults is a device read. Under a cipher a touch of a paged mapping is a decrypt and a copy. | STAYS | mis-priced attach against build |
| M5 | Builds ahead, drop-behind (`plan.drop_behind`), will-need ahead (`plan.ahead`), `drop_output`/cold, `zpages_build_sync`, the crossing swap (`z_cross`), `zpages_drop_raw`. | zfile.rs:3915-4046, 4416-4590; db.rs:8188-8197, 40360-40400 | Advice to the page cache saves what the kernel would hold. Under a cipher it is advice to the cache of the ciphertext (forwarded), while the plaintext is the pager's to give back; the two are evicted independently. | CHANGES (through `vault::File` and the Handle's mapping) | E7 measured the faults term (about 69% of the out-of-core gap) as the bound split between the two caches, and the design keeps the split. |
| M6 | The block cache's budget (`scan_cache_bytes`/`auto_cache_bytes`, `Db::resolve_auto_cache`). | db.rs:23822-23840, 27449, 28057 | Sized against the process's memory and not against the pager. Whether `resolve_auto_cache` sees `cache_bytes` is not verified. | STAYS | – |
| M7 | Merges' drop-behind and verification reads (`merge_drop_behind`, `merge_verifies_inputs`) are priced as page-cache reads. | db.rs:12755-12790 | the page cache | CHANGES (fd through `vault::File`) | – |
| M8 | `pieces_willneed`/`blocks_willneed`/`heads_willneed`, gated on `zpages` being out of core. | db.rs:3149-3295; blob.rs:1239-1290 | The fault is a device read. Under a cipher two of the three are no-ops (F6). | STAYS (not addressed) | silent |

## 5. Couplings the Pagefile design adds (NEW)

| # | what | form author must | forget → |
|---|---|---|---|
| N1 | One of five classes per file, given at create (`SegmentOptions` field; `vault::create(path, Class)`). | choose a class for every new file kind | wrong durability or trust semantics. This is a generic contract, not form vocabulary, and it is a compile error if `create` takes the class. |
| N2 | `finish()` for a Log file that becomes Once. | call it at a run-like close | the file stays Log: damage reads as a truncation, and its ring is consulted forever |
| N3 | At most 4 overrides (patches below the written length) per Log file: today the run's pages 0 and 1, and the WAL's chain byte. | keep patches of an in-progress log-class file to a handful of pages | the counting test fails. A form change that patches more of a run (a bigger head reserve, a trailer patched in place) breaks it. |
| N4 | NAME_MAX: the longest logical name (`par-`, 115 bytes) seals to 253 of 255 characters. | keep logical names short | an open or create failure: a longer fence or a new prefix of three more bytes breaks it. Held by an S2 test. |
| N5 | The `.tmp` sweep: logical names ending in `.tmp`, other than `direct-*.tmp`, are removed at open. | not rely on a `.tmp` surviving an open | a new temp meant to survive is deleted |
| N6 | Regen-lazy files: every site reading a form page treats `page_bytes → None` as `ZFile::fail` (E0 item 6: holds today, no zfile change). | the same for a new form-page reader | a read through a failed form page answered as absent |
| N7 | Rename over an existing logical name costs a `sync_dir`. The manifest's rename shape is refused under `Pg`. | avoid a hot rename-over | a slowdown, or a refusal |
| N8 | `as_os()` plain-only sites. | not use them on cipher paths | runtime `None` |
| N9 | Write-through into the plaintext backing, and `open_mapped` always written through. | a file read while written still goes through `open_mapped` | as D13 |
| N10 | `vault::paged(path)` as the selector for the held build (in place of `kept`). | – | – |

## 6. Assessment against the owner's requirement

The requirement: "make form changes easily without constant encryption changes".

**Encryption code that has to change when a form changes (E→F).** Today the vault knows the forms through E1-E9 and E13-E14: which files exist, how they are named, which derive from which, which are durable when, how the log is framed, and that the run maps its file. Pagefile removes all of that except `Mapped` (E8) and the 4080/4096 geometry while envelopes exist (E10). After S6 a new file kind needs no vault code, only a class (N1).

**Form code that has to think about encryption (F→E, D).** The write side mostly goes: F20, F25, F26, F29, F30 and F31, and D10-D12 and D14. The read side does not go. Pagefile keeps the explicit-ask pager, so F1-F5, F9-F19 and D1-D9 and D13-D17 stay exactly as they are. These are the disciplines behind most of this branch's bugs: the borrow across a rest, the chunk lent out of a unit marked twice, the waits under a pin, the builders giving up, and the hang in `land_seal`. Every new reader, walk, background job or wait still has to know them. The owner's sentence is about "form changes" in general, and a new reader or job is a form change. On the read side the design therefore does not meet the requirement; it meets it on the write side.

**Failure modes.** None of the 17 disciplines is a compile-time property; the door is a test, not a type. Three are runtime refusals under a cipher (D9, D12, D1 for scans), two are debug assertions (D4, and D2 under `SUPDB_PAGER_GUARD`), and the rest are wrong answers or silent slowdowns. The silent ones are the dangerous ones: F6 (willneeds dead under a cipher, present today), F17 (warm), F21, F24 and D11.

**Levers that would move the read side.** These are not designed and not priced; they are offered as data.
1. **A plain test mode that lends nothing.** `MmapBytes` with `lend = 0` over a plain mapping (a pager stand-in that decrypts nothing), under a test flag, so the whole plain suite runs every reader through the ask path. D1, D2 (with the guard) and F6 then fail in ordinary tests, not only in the cipher fuzz. This needs no product change.
2. **Type-enforced pins and rests.** Borrows tied to the lifetime of a `&Guard`, with `rest(&mut Guard)`. A borrow held across a rest is then a compile error, not a wrong answer. It is a large refactor of the readers' lifetimes, and its plain cost is unknown.
3. **Run the whole suite under a cipher in CI** (an env var that wraps `Options`), as the fuzz does for its range. Coverage then follows tests, not the cipher tests that someone remembered to write.
4. **Widen the door test** to every module (the design does this for zfile.rs, ordindex.rs, bytes.rs and pager.rs), and bar `as_slice()` on an `MmapBytes` outside bytes.rs.
5. **A transparent mapping** (userfaultfd or PROT_NONE). It deletes D1-D5 and D13-D17 outright, and was rejected in §11 for container seccomp and a 7-11 µs fault. Its rejection is the price of the read-side coupling, and that trade should be said to the owner in those terms.

**What was not verified:**
- the exact count of `Sect`-generic functions;
- the open's rest lines 27813/27844;
- whether `resolve_auto_cache` sees the pager;
- whether any budget path subtracts the pager's `held` (grep only);
- the 8 sites of `indexed!` (from readpath.md, not recounted).
