# R2 and R3: what becomes private, who calls it, what each caller becomes

Export read: `scratchpad/plan/tree` (commit 682b2c09). All paths below are
relative to that root. Nothing was built or run. Line numbers are this
export's. "fn" is the enclosing function, found by
`plan/borrows/encl.py` (a backwards scan for the nearest `fn`/`macro_rules!`
line; checked by eye at every site quoted here).

## 0. Searches run

Each was run from the export root. `--include=*.rs` unless said.

| # | pattern | where |
|---|---|---|
| S1 | `as_slice\b\|as_slice(` | `.` (rs, mjs, js) |
| S2 | `lend_at` | `.` (rs, md) |
| S3 | `\.flat()\|\.pieces()` | `src/blob.rs`, then `src` |
| S4 | `indexed!(` | `src/blob.rs` |
| S5 | `Paging\|MmapBytes::paged\|fn base\|\.base()` | `.` |
| S6 | `from_raw_parts` | `src` (and `grep -c` per file) |
| S7 | `record_regions\|record_pieces\|record_region_span\|record_region_place\|record_offset\|willneed_ranks\|willneed_dir\|prefetch_ranks\|prefetch_ranks_est\|prefetch_dir\|drop_records\|zero_copy\|scan_either\|scan_whole\|resident_pages\|paged_resident\|record_region_bytes\|hash_region_bytes\|z_source_facts` (one grep each, `\b` after) | `.` |
| S8 | `\.source()` | `.` |
| S9 | `impl.*Bytes for` | `.`; then `fn lend_at\|fn piecewise\|const ASK_PIECES\|fn piece_end\|fn piece_at` in `tests examples bench/src bench/examples experiments` |
| S10 | `MmapBytes` | `tests examples bench/src bench/examples experiments README.md docs web` |
| S11 | `: MmapBytes\|: &MmapBytes\|<MmapBytes>\|-> MmapBytes\|Result<MmapBytes>\|&'a MmapBytes\|Arc<MmapBytes>\|Option<MmapBytes>\|= MmapBytes::\|vault::map(\|super::vault::map(` | `src` (every binding of the type, to find method calls a name grep cannot tie to a receiver) |
| S12 | `piecewise()\|ASK_PIECES\|\.piece_at(\|\.piece_end(` | `src` |
| S13 | `libc::mincore\|libc::madvise\|libc::posix_fadvise\|\.advise(memmap2\|advise_range\|sync_file_range` | `src/*.rs src/db/*.rs` minus pager.rs, vault.rs |
| S14 | `fadvise_dontneed_range(\|fadvise_dontneed(\|resident_share(\|swap_ahead(\|ask_ahead(\|drop_behind(\|drop_written_range(\|warm_file(` | `src/db.rs src/db/zfile.rs` |
| S15 | `memmap2::\|Mmap::map\|MmapMut` | `src/db.rs src/db/*.rs src/ordindex.rs src/blob.rs` |
| S16 | `vault::open(` | `src/db.rs src/db/zfile.rs` |
| S17 | `advise_dontneed_range\|advise_willneed\|resident_at\|advise_dontneed\b\|advise_random\|advise_normal` | outside dirs |

How the hidden forms were covered:

- **Method-call syntax.** A name grep finds `x.as_slice()` for every type.
  S11 lists every binding whose type is `MmapBytes` in `src`; each S1 hit was
  read and kept only where its receiver is one of those (or a field of type
  `MmapBytes`: `OrdIndex::map` ordindex.rs:365, `ZFile::map` zfile.rs:1863,
  `Held::map` zfile.rs:4523). The other S1 hits are `Vec`/`Extents` and are
  not listed.
- **Renamed imports.** `grep -rn "as_slice as\|lend_at as\|Bytes as\|Paging as\|MmapBytes as"`
  found only `use crate::bytes::Bytes as _;` (db.rs:42435). No renamed import
  of any item in this note.
- **Macros.** The two macros that reach a whole borrow are `indexed!`
  (blob.rs:941) and `walk_fixed!`/`emit_record!` (blob.rs:779, 882). Their
  bodies were read; `indexed!` calls `$self.flat()` and `$self.pieces()`,
  and its eight call sites are S4. The other two take a slice they are given.
- **Trait impls.** S9 lists every `impl Bytes for`. `lend_at` is overridden
  only by `MmapBytes` (bytes.rs:481). `Paging` is implemented only by
  `pager::Paged` (pager.rs:1302).
- **Deref.** `SegBlob` (db.rs:5457) has no `Deref`; `Seg::raw` (db.rs:8969)
  and `SegBlob::for_zfile` (db.rs:5491) hand out `&Blob<MmapBytes>`, so every
  `Blob` method in `db.rs` is reached through `.raw().` or `.for_zfile().`,
  which S7 matches by method name.

Not covered: `web/*.mjs` was searched by S1 and found nothing; JS cannot
name a Rust item, so it is outside this question except through the wasm
module's size.

## 1. The items, their visibility, and every use

Kinds: **D** dispatch (chooses the whole walk or the walk by the piece);
**W** a reader or walk that would have to ask by the piece; **N** silently
does nothing or answers short for a source that lends by the piece; **P**
public API; **T** a test.

### 1.1 `MmapBytes::as_slice` — `pub fn`, bytes.rs:376

Returns `from_raw_parts(ptr, lend)`: the whole of a mapped file, an empty
slice for a paged mapping (`lend == 0`). Reachable outside: yes
(`supdb::MmapBytes` is re-exported, lib.rs:107).

| site | fn | kind | under a paged source |
|---|---|---|---|
| ordindex.rs:419 | `OrdIndex::open` | N | `whole` is empty; the CRC (425-428) and the top level (460-470) are skipped under `if !paged` |
| ordindex.rs:466 | `OrdIndex::open` | (same `whole`) | top level deferred to `top_late` |
| ordindex.rs:678 | `OrdIndex::head` | D | fast path; empty slice sends every head to `head_absent` (513), which asks with `slice_at` |
| ordindex.rs:940 | `OrdIndex::seek_exact_hint` | N (speed only) | `get(at..)` is `None`, so no `prefetch_lines` of the stride |
| zfile.rs:3784 | `warm_file` | N | `len = 0`: warms nothing, returns `Warm{0,0,us}` |
| zfile.rs:3802 | `warm_file` | N | the touch loop; never runs |
| zfile.rs:4266 | `build_file_seen` (`SegSrc::File` arm) | W | base pointer for the address-based advice of `Mapped` (only plain files reach this arm, `open_seg` zfile.rs:4203) |
| bytes.rs:659 | test `resident_pages_counts_every_page_a_range_touches_and_none_past_the_end` | T | |
| vault.rs:2409 | test `a_kept_file_lives_in_memory_and_reaches_the_directory_as_an_envelope` | T | asserts a paged mapping's `as_slice` is empty |
| zfile.rs:7482, 7483, 7492, 7494, 7498, 7501, 7507 | test `an_ask_ahead_brings_the_cold_pages_in` | T | feeds `ask_ahead` a base pointer |
| zfile.rs:7529, 7530 | test `a_warm_reads_the_cold_file_once_and_skips_a_resident_one` | T | |

Outside `src`: **none** (S1 over tests, examples, bench, experiments; every
hit there is a `Vec` or a byte literal).

### 1.2 `Bytes::lend_at` — trait method of `pub trait Bytes`, bytes.rs:79 (default), override bytes.rs:481

Default: `None` if `piecewise()`, else `slice_at`. `MmapBytes`: one range
check against `lend`, so `None` for any range of a paged mapping without a
test of the kind.

| site | fn | kind |
|---|---|---|
| blob.rs:507 | `Sec::get` | D (the only caller) |

Outside: no call, no override (S2, S9).

### 1.3 `Sec` and `Sec::get` — private `enum Sec`, blob.rs:480; `fn get` blob.rs:504

Already module-private. `Sec::get` is the whole borrow; `Sec::pieces`
(blob.rs:515) is the other arm.

| site | fn | kind | note |
|---|---|---|---|
| blob.rs:967 | `Blob::open_with` | D | `parse_in(pieces)` else `parse(get)` |
| blob.rs:980-982 | `Blob::open_with` | **N** | key-section checksum row verified only when `key.pieces(&src).is_none()`: under a paged source the row is never checked. Same kind as the ordered index's CRC skip; not in forms.md's list |
| blob.rs:991 | `Blob::open_with` | D | block table |
| blob.rs:1020 | `Blob::flat` | D | |
| blob.rs:1160 | `Blob::blk_sec` | D | callers fall to `blk_pieces` (blob.rs:1630-1635, 1741-1747) |
| blob.rs:2871 | `SparseBlob::open_with` | loud, not N | `blk.get(&src)?` errors (`Unsupported`) for a piecewise source; the engine never opens a `SparseBlob` |

### 1.4 `Blob::flat` (blob.rs:1018) and `Blob::pieces` (blob.rs:1035) — both private `fn` already

forms.md says they "become private". They are private to `blob` today. The
work is in their callers inside `blob.rs` (S3):

| site | fn | kind | under a paged source |
|---|---|---|---|
| blob.rs:943, 945 | `indexed!` | D | |
| blob.rs:1212 | `Blob::prefetch_ranks` (pub) | N (speed) | no CPU prefetch |
| blob.rs:1240 | `Blob::willneed_ranks` (pub) | **N** | returns 0, asks the kernel nothing (forms.md "~1239") |
| blob.rs:1277 | `Blob::willneed_dir` (pub) | **N** | returns 0 (forms.md "~1276") |
| blob.rs:1299 | `Blob::record_offset` (pub) | **N** | `None`; its caller's drop-behind does nothing (1.7) |
| blob.rs:1326 | `Blob::prefetch_dir` (pub) | N (speed) | no CPU prefetch |
| blob.rs:1352 | `Blob::prefetch_ranks_est` (pub) | N (speed) | no CPU prefetch |
| blob.rs:1380 | `Blob::record_regions` (pub, `#[doc(hidden)]`) | P/D | `None`; the whole-section accessor (1.6) |
| blob.rs:1391 | `Blob::record_pieces` (pub, doc hidden) | D | |
| blob.rs:1427 | `Blob::record_region_place` (pub, doc hidden) | D | `None` for a whole reader |
| blob.rs:1502 | `Blob::values_at` | D | falls to `values_at_pieces` |
| blob.rs:1521 | `Blob::values_at_pieces` | D | |
| blob.rs:2159 | `Blob::scan_counts` | D | `None` walk falls to `exts_at` (indexed) |
| blob.rs:2206 | `Blob::scan_counts_fixed` | D | same |
| blob.rs:2311 | `Blob::scan_pieces` | D | |
| blob.rs:2463 | `Blob::scan_whole` | D | refuses with `by_the_piece` at 2524 |

`indexed!` call sites (S4): `lookup` 1167, `lookup_full` 1177,
`with_lookup` 1188, `exts_at_full` 1197, `seek` 1202, `z_source_facts`
1450, `key_at` 1481, `exts_at` 1487. All D.

### 1.5 `Paging` and `Paging::base` — `pub trait Paging`, `#[doc(hidden)]`, bytes.rs:300; `fn base` bytes.rs:302

| site | fn | kind |
|---|---|---|
| bytes.rs:367 | `MmapBytes::paged` (`pub(crate)`) | the only caller of `base()` |
| pager.rs:1303 | `impl Paging for Paged :: base` | the only impl |
| vault.rs:156 | `use crate::bytes::Paging;` | brings `size()` into scope (vault.rs:968) |
| pager.rs:79 | `use crate::bytes::Paging;` | the impl |

Outside: none (S5). An outside caller can name `supdb::bytes::Paging` and
implement it, but cannot build a `MmapBytes` from it (`paged` is
`pub(crate)`), so the trait is dead outside.

### 1.6 Other accessors that hand out a whole section or a base pointer

| item | vis | site(s) of use | kind |
|---|---|---|---|
| `Blob::record_regions` blob.rs:1379 | pub, doc hidden | zfile.rs:4273 (`build_file_seen`, W), zfile.rs:5065 (`sample`, D), db.rs:8956 (`Seg::records_resident`, D), blob.rs:1413 (`record_region_span`), zfile.rs:7434, 8375 (tests) | whole section |
| `Blob::record_region_span` blob.rs:1412 | pub, doc hidden | db.rs:42429 (`Db::z_cross`, D) | takes `slice_at(0,1).as_ptr()` as the mapping's base and subtracts (blob.rs:1414-1415): a raw base pointer |
| `Blob::source` blob.rs:1026 | pub | db.rs:8960 (`records_resident`), db.rs:42436 (`z_cross`), zfile.rs:4334 (`build_file_seen` `Held`); outside: tests/blob.rs:399, tests/segwriter.rs:780, 788, 1052, 1060, tests/dict.rs:536, 540, 568, 579, 641, all on their own test types | harmless once `as_slice` is private |
| `MmapBytes::resident_pages` bytes.rs:422 | pub | zfile.rs:3795 (`warm_file`, **N**: clamps to `lend`, 0 for paged); tests bytes.rs:662-676, zfile.rs:7483-7529, 8203; **outside: tests/zpages.rs:5325** (`an_attached_form_file_is_warmed_once_per_process_unless_the_warm_is_off`) | address-based residency |
| `MmapBytes::paged_resident` bytes.rs:415 | pub | db.rs:8960 (`records_resident`), pager.rs:1754 (test) | paged-only answer; `None` for a mapped file |
| `SegImages::new(recs, dir, n)` (zpages) | pub, doc hidden | zfile.rs:4284, 5067; zpages.rs:7526, 7559 (tests); outside bench/examples/zsize.rs (after `slice_at` of the whole section, line 289), tests/zpages.rs:84 | W: whole-slice images. Outside callers obtain the slice by asking (`slice_at`), which R2 allows |
| `RunMap::map` (`memmap2::Mmap`, db.rs:12167) | private | db.rs:11184, 11629, 11647, 12019 | whole borrow of the run's own file mapping. Correct today: under a cipher the run's file is a memory file held whole (`vault::Mapped`). Out of R2 as forms.md writes it; it becomes R2 if the run ever goes behind a paged layer (S4) |

### 1.7 Advice sites that take an address or a descriptor (R3)

From S13, S14, S16. Each is where a byte source should answer instead.

| site | fn | what it does | under a paged source |
|---|---|---|---|
| db.rs:15796-15821 | `resident_share` | `mincore` over a slice's address | not reached (caller dispatches) |
| db.rs:8954-8962 | `Seg::records_resident` | D: `resident_share(recs)` or `source().paged_resident(...)` | right |
| db.rs:13579-13608 | `compact_run` (merge drop-behind) | `record_offset` + `drop_records` + `fadvise_dontneed_range` on `vault::open` of each input | **N**: `record_offset` is `None`, nothing dropped; and `vault::open` of a kept file is `reopen_written` (vault.rs:1897), a memory file, not the envelope (not verified what fadvise does there) |
| db.rs:8919-8929 | `Seg::drop_raw_pages` | `blob.advise_dontneed()` (forwards to `Paging::forget`) then `fadvise_dontneed` of `vault::open(name)` | first half right; second half on a memory file (not verified) |
| db.rs:42411-42452 | `Db::z_cross` | D: `record_region_span` + `swap_ahead` (fd, zfile.rs:3957) or `record_region_place` + `advise_dontneed_range` + `advise_willneed` | right, but two paths |
| db.rs:4771-4778 | `drop_synced` | fadvise on a writer's own file | write side; out of R3's read question |
| zfile.rs:3926-3945 | `drop_written_range` | `sync_file_range` + fadvise of the form file being written | write side; out of scope |
| zfile.rs:4002-4024 | `drop_behind` | `madvise(base+a)` + fadvise | plain build only |
| zfile.rs:4035-4049 | `ask_ahead` (free fn) | `madvise(base+a, WILLNEED)` | plain build only |
| zfile.rs:4436-4505 | `Mapped` (`Walked` impl) | carries `base`, `file`, `recs_at`, `dir_at` from pointer subtraction (4294-4295) | plain build only |
| zfile.rs:4521-4590 | `Held` (`Walked` impl) | the same walk through `map.advise_willneed` / `advise_dontneed_range` | the paged twin |
| zfile.rs:3778-3810 | `warm_file` | `as_slice`, `resident_pages`, `advise_willneed`, page touch | **N** |
| bytes.rs:513-638 | `impl Bytes for MmapBytes` advice | forwards each to `Paging` for a paged mapping | right; `advise_dontneed_range` for a mapped file does `madvise` only, no fadvise (the fd is not kept) |

`ZFile`'s own advice (`ask_ahead` method zfile.rs:2857, `drop_cache`
3255, the willneed at 3267) already goes through `self.map`, except
`drop_cache`'s `fadvise_dontneed(f)` on `self.file` (zfile.rs:3257-3258).

### 1.8 Every `from_raw_parts` / `from_raw_parts_mut` in `src`

S6 counts 44: bytes.rs 5, blob.rs 1, pager.rs 5, blocks.rs 2, zpages.rs 6,
crypt.rs 3, db.rs 6, flatindex.rs 2, wasmapi.rs 14.

| site | fn | memory |
|---|---|---|
| bytes.rs:378 | `MmapBytes::as_slice` | file mapping (`lend` bytes; empty for paged) |
| bytes.rs:403 | `MmapBytes::slice_slow` | the pager's memory file (paged mapping), after `ensure` |
| bytes.rs:475 | `MmapBytes::slice_at` | file mapping |
| bytes.rs:487 | `MmapBytes::lend_at` | file mapping |
| bytes.rs:509 | `MmapBytes::piece_at` (Paged arm) | the pager's memory file, after `piece_from` |
| blob.rs:3153 | `SparseBlob::dictionary_walk` | a heap `Vec<u64>` |
| pager.rs:1003 | `Paged::fill` | the pager's memory file (the decrypt target) |
| pager.rs:1694, 1866, 1892, 2055 | tests (`read`, `nothing_is_given_back_under_a_hold`, `a_rest_gives_way_and_a_range_asked_again_is_the_same_bytes`, `a_changed_chunk_fails_its_read_and_poisons_the_store`) | the pager's memory file |
| blocks.rs:216, 243 | tests | anonymous blocks |
| zpages.rs:781, 795, 812 | `GroupBuf::bytes_for`, `bytes_with_slack`, `bytes` | heap `Vec<u64>` |
| zpages.rs:3579, 3589 | test helper `Al::new`, `Al::bytes` | heap |
| zpages.rs:4450 | test `a_stored_page_is_parsed_in_place_where_aligned_and_copied_where_not` | heap |
| crypt.rs:752 | `Kept::lend` | anonymous mapping of an envelope's plaintext |
| crypt.rs:976 | `Encrypted::keep` | the same anonymous mapping |
| crypt.rs:1555 | test | the same |
| db.rs:9718 | `Slots::deref` | anonymous block (`blocks::alloc`, heap below 128 KiB or `mmap` anonymous) |
| db.rs:9972 | `ByteArena::try_slice` | anonymous block |
| db.rs:10120 | `ByteArena::write_with` | anonymous block |
| db.rs:10200 | `ByteArena::slice` | anonymous block |
| db.rs:17778 | `SnapArena::reserve_bytes` | anonymous block |
| db.rs:17790 | `SnapArena::slice` | anonymous block |
| flatindex.rs:584 | `parse_record_tail` (free fn) | reinterprets a slice it is handed as `&[Ext]`; the memory is whatever the caller lent (file mapping, pager window, or heap copy) |
| flatindex.rs:3152 | `encode_blocks` | a stack `BlockRec` |
| wasmapi.rs (14) | the C ABI | wasm linear memory handed in by the host |

The six db.rs sites forms.md left unclassified (its 8988, 9242, 9390, 9470,
16848, 16860) are these six: all anonymous blocks from `blocks::alloc`
(db.rs:9708, 9994, 10086, 17701). None addresses a file mapping. No
`from_raw_parts` outside bytes.rs and pager.rs addresses a file or the
pager's memory.

## 2. The N sites, found and rewritten (R3)

forms.md's list, located in this export:

- "blob.rs ~1239" = `Blob::willneed_ranks`, blob.rs:1239-1266.
- "blob.rs ~1276" = `Blob::willneed_dir`, blob.rs:1276-1296.
- "zfile.rs ~3783" = `warm_file`, zfile.rs:3778-3810.
- "ordindex.rs ~413-428" = `OrdIndex::open`, ordindex.rs:403-480.
- "the zero footprint": **not present**. `Seg::foot` (db.rs:8941) reads
  `hash_region_bytes` and `record_region_bytes`, which match
  `Index::Flat { idx, .. }` and so answer for a paged reader too. Only the
  doc comment at blob.rs:1432-1433 ("0 for a reader that does not hold the
  section whole") is stale; the code answers 0 for `Index::Sparse` alone.

More of the kind, found by S3, S7, S13, S14:

- `Blob::record_offset` blob.rs:1298, and its caller `compact_run`
  db.rs:13591-13608: no drop-behind of merge inputs under a cipher.
- `Blob::open_with` blob.rs:980: the key section's checksum row is never
  verified for a paged source.
- `MmapBytes::resident_pages` bytes.rs:422: 0 for a paged source (only
  `warm_file` uses it in non-test code).
- Speed-only, no wrong answer: `prefetch_ranks` 1211, `prefetch_ranks_est`
  1351, `prefetch_dir` 1325, `OrdIndex::seek_exact_hint` 940 (CPU-cache
  prefetches by address).
- Advice on a descriptor that is a memory file under a cipher:
  `drop_raw_pages` db.rs:8924, `compact_run` db.rs:13584,
  `ZFile::drop_cache` zfile.rs:3258 (via `self.file`, opened by
  `vault::open` at zfile.rs:2023). What `vault::open` returns for each of
  those files under a cipher, and what fadvise then does, is not verified.

Rewrites. The byte source gains three methods, defaulted so no outside
impl breaks (11 outside impls, S9): `resident(off, len) -> Option<f64>`,
`forget(off, len)` (mapping pages and the file's page cache behind them),
`willneed(off, len) -> usize` (today's `advise_willneed`, kept by name if
wanted). `MmapBytes` answers them for a mapped file by `mincore`/`madvise`
plus `posix_fadvise`, which means `Own::Map` keeps the `File` (or a dup of
its fd) beside the `Mmap`; for a paged mapping by `Paging::resident`,
`forget`, `willneed`, which exist (bytes.rs:321-329). Offsets come from the
index header (`FlatIndex`'s `recs`/`dir` ranges plus `index_offset()`),
never from pointer subtraction.

1. **`willneed_ranks`, `willneed_dir`, `record_offset`.** Write each over
   `indexed!` (or a Sect-generic helper it calls): `FlatIndex::record_span`,
   `record_off` and `dir_span` are already generic over `Sect`
   (flatindex.rs:2198, 2234, 2222). The file offset is
   `index_offset() + region start from the header + a`. Then
   `self.src.resident_at` / `advise_willneed` / `forget`, which already
   forward to the pager. Under a paged source this asks one directory word
   by the piece and advises the ciphertext.
2. **`compact_run`'s drop-behind.** With `record_offset` answering for both
   kinds, `drop_records` becomes `src.forget(from, len)`, and the
   `files: Vec<Option<File>>` and `fadvise_dontneed_range` go (the source
   does the fadvise). Same for `drop_raw_pages` (db.rs:8919): one
   `blob.forget_all()` and `ord.forget_all()`.
3. **`OrdIndex::open`.** Take the CRC by the piece: walk the body in
   chunks of a few pieces with `map.piece_at`/`slice_at`, folding with
   `block::crc32_resume` (block.rs:123), under the opener's hold. Build
   `top` through `head(i)` for both kinds, or keep the deferral but for both
   kinds. A plain open pays one loop over the same bytes it reads today; the
   instruction count at open is row 6 of G-cost (not verified).
4. **`Blob::open_with`'s row check (blob.rs:980).** Verify the row by the
   piece: either the whole row at open through `Pieces` (each piece asked
   once), or per piece on first use with the `verified` bits `SparseBlob`
   already keeps (blob.rs:2946 `verify_span`). Paged-only cost; the plain
   path is unchanged.
5. **`warm_file`.** Use `map.len()`, `map.resident(off, span)` and
   `map.willneed(off, span)`. The per-page touch that makes the reads
   synchronous becomes a byte-source method (`populate(off, len)`): a touch
   or `MADV_POPULATE_READ` for a mapped file, the willneed alone for a
   paged one (a touch there would decrypt into the store's cache, which the
   warm does not want). `resident_pages` then has no non-test caller.
6. **`records_resident` (db.rs:8954) and `z_cross` (db.rs:42411).** One
   path each: offset and length from the header, `source().resident(..)`;
   `source().forget(recs)` and `vault::map(form).willneed(0, len)`.
   `resident_share`, `swap_ahead`'s fd half, `record_region_span` and
   `paged_resident` are deleted.
7. **The plain build (`Mapped`, zfile.rs:4436).** Same walk as `Held`:
   region offsets from the header, `map.willneed` ahead and `map.forget`
   behind; `drop_behind` and the free `ask_ahead` are deleted. The images
   stay `SegImages` over whole slices for the plain build (per-record
   path), obtained from the dispatch accessor of section 3.3.
8. **CPU prefetches** (`prefetch_ranks*`, `prefetch_dir`, ordindex 940).
   Either leave them as no-ops for a paged source and say so in the doc,
   or move the address into a byte-source method (`hint_lines(off, len)`,
   a no-op for paged). They are on the scan's hot path (`prefetch_block`
   db.rs:14400, `scan_seek` db.rs:20968); a trait method inlined over
   concrete `MmapBytes` should compile to the same code (predicted, not
   verified; G-cost row 2 decides).

## 3. The public API

### 3.1 What an outside caller can name today

From lib.rs:97-116 and the `pub mod` lines.

| item | outside can | used outside (S1, S2, S7-S10, S17) |
|---|---|---|
| `Bytes` (trait, every method, `ASK_PIECES`) | call and override every method | 11 impls; `tests/blob.rs:268,271` override `piecewise` and `piece_end` (the `Piecewise` source that holds readers to asking); `examples/crypt_cost.rs:208` calls `Blob::advise_random` |
| `Bytes::lend_at` | call and override | none |
| `MmapBytes::{open, from_map, as_slice, paged_resident, resident_pages}` | call | `open` widely; `resident_pages` at tests/zpages.rs:5325; `from_map`, `as_slice`, `paged_resident` none |
| `bytes::Paging` (doc hidden) | name, implement | none |
| `Blob::{record_regions, record_pieces, record_region_span, record_region_place, record_offset, willneed_*, prefetch_*, drop_records, source, zero_copy, z_source_facts}` | call | `zero_copy` (tests/blob.rs, segwriter.rs, crypt.rs), `z_source_facts` (tests/forged.rs:161), `source` (tests on their own types); the rest none |
| `flatindex::{Sect, Pieces, FlatIndex}` | all | `FlatIndex::parse`/`regions` over a slice from `slice_at` (bench/examples/zsize.rs:291-292, tests/zpages.rs:83-84) |
| `Blob::flat`, `Blob::pieces`, `Sec` | nothing (private) | — |
| wasm: `Blob<VecBytes>`, `Blob<HostBytes>`, `SparseBlob<HostBytes>` (wasmapi.rs:97-102) | through the C ABI | neither source overrides `lend_at` or `piecewise` |

### 3.2 What Rust can express, and what each costs

`pub(in crate::bytes)` on a trait method does not exist: a trait's methods
share the trait's visibility. And `pub(in path)` takes only an ancestor of
the item's own module, so nothing in `crate::bytes` can be made visible to
`crate::blob` and not to the rest of the crate. The options:

- **A sealed supertrait** (`pub trait Bytes: sealed::Sealed`). Every outside
  `impl Bytes` stops compiling: 11 impls, including the `Piecewise` test
  source and `examples/crypt_cost.rs`'s `Copying`. A blanket
  `impl<T> Sealed for T` keeps them compiling but then `MmapBytes` cannot
  have its own `lend_at` (no specialization on stable). Rejected.
- **A sealed method by an unnameable token.** Keep `lend_at` on `Bytes` with
  a third parameter of a type outside callers cannot name:
  `fn lend_at(&self, off: u64, len: usize, _: seal::Token) -> Option<&[u8]>`,
  where `mod seal { pub struct Token(()); }` is a private module of
  `bytes` and the constructor is `pub(crate)`. Outside callers cannot call
  it (no `Token`) and cannot override it (cannot write the signature), so
  every outside impl keeps the default. `MmapBytes` overrides it inside the
  crate. The token is a zero-sized value and the call is inlined over a
  concrete or monomorphised `B`, so the plain lookup and scan should
  compile to the same code (predicted, not verified: G-cost rows 1, 2, 3
  and 8). Whether rustc's private-in-public lints accept a `pub` type in a
  private module in a public trait's signature without a warning under
  `-D warnings` is not verified.
- **`pub(crate)` inherent methods.** Right for `MmapBytes::as_slice`,
  `resident_pages`, `paged_resident` and the `Blob` accessors. Wrong for
  `lend_at`: `Sec::get` is generic over `B: Bytes` and cannot reach an
  inherent method.
- **Scoping inside the crate.** Neither choice confines `lend_at` to
  `blob`: the token constructor is `pub(crate)` at most. What confines it is
  clippy `disallowed_methods` naming `supdb::bytes::Bytes::lend_at` and the
  token constructor, with `#[allow(clippy::disallowed_methods)]` on
  `Sec::get` and `OrdIndex::head`, which is the same mechanism forms.md's
  G-door uses. Whether `disallowed_methods` matches a trait method called
  through a generic `B` is not verified.
- **An enum that must be matched.** Replace the pairs that answer `None`
  for one kind (`flat`/`pieces`, `record_regions`/`record_pieces`,
  `record_region_span`/`record_region_place`) with one accessor returning
  `enum Held<'a, B> { Whole(&'a [u8], ...), Pieces(Pieces<'a, B>, ...) }`.
  A caller has to name the piece arm, so a silent `None => return 0` for a
  paged source cannot be written by accident. It can still be written on
  purpose (`_ => 0`); clippy's `wildcard_enum_match_arm` is a restriction
  lint over every enum and is not scoped to one. Over an inlined accessor
  the enum is two registers and a tag (predicted, not verified; CLAUDE.md's
  "a value built a field at a time and handed back whole" says to check
  the annotation, not the count).

Monomorphisation: `indexed!` writes each body twice and `scan_either` keeps
the piece walk out of line (`scan_refused`, `#[cold] #[inline(never)]`).
None of the choices above changes that. A dispatch written as a generic
function taking two closures was rejected in CLAUDE.md for the same cost
(a callback with a second taker cost the slice loop 3-5 instructions an
entry), so keep the macros.

### 3.3 Recommended visibility, item by item

| item | now | becomes | outside callers broken |
|---|---|---|---|
| `MmapBytes::as_slice` | pub | deleted (or private to `bytes`) | none |
| `Bytes::lend_at` | pub trait method | token-sealed method; clippy-confined to `Sec::get`, `OrdIndex::head` | none |
| `Paging` | pub, doc hidden | `pub(crate) trait`; `base` clippy-confined to `MmapBytes::paged` | none |
| `MmapBytes::resident_pages` | pub | `pub(crate)`, or deleted once `warm_file` and the tests use `Bytes::resident` | tests/zpages.rs:5325 (switch to the new `resident`) |
| `MmapBytes::paged_resident` | pub | deleted (`Bytes::resident`) | none |
| `Blob::record_regions` + `record_pieces` | pub, doc hidden | one `pub(crate)` accessor returning the two-armed enum | none |
| `Blob::record_region_span` + `record_region_place` | pub, doc hidden | one `pub(crate)` offset accessor from the header | none |
| `Blob::record_offset`, `willneed_ranks`, `willneed_dir`, `drop_records`, `prefetch_*` | pub | stay pub (or `pub(crate)`: no outside caller), rewritten per section 2 | none |
| `Blob::flat`, `Blob::pieces`, `Sec` | private | unchanged | — |
| `Bytes::piecewise`, `piece_end`, `piece_at`, `ASK_PIECES` | pub trait | unchanged: an outside source declares its kind with them (tests/blob.rs `Piecewise`). An outside `ASK_PIECES = false` with `piecewise() = true` gets `Unsupported` from `scan_at`, not a wrong answer | — |
| `Blob::zero_copy`, `source`, `SparseBlob` | pub | unchanged | — |

The wasm module: `VecBytes` and `HostBytes` take the default `lend_at`; the
token adds a zero-sized argument. Its size is held by `web/` size control
in the wasm group; a change is expected to be none (not verified).

## 4. Order of landing, and a red test for each

Each step compiles alone. Each names a test that fails on today's code (the
known-red input) before the step lands.

1. **Byte-source advice methods (R3 groundwork).** Add `resident`,
   `forget`, `populate` (and `hint_lines` if chosen) to `Bytes`, defaulted;
   `Own::Map` keeps the file's descriptor; `MmapBytes` answers both kinds.
   No caller changes. Red test: a unit test in bytes.rs that a mapped
   file's `forget` empties the page cache under it (by `mincore` after an
   fadvise-free drop, today's `advise_dontneed_range` leaves the cache
   resident since it only `madvise`s). Rows: none hot.
2. **Blob's span helpers (N1-N3) and the merge's drop-behind.** Rewrite
   `willneed_ranks`, `willneed_dir`, `record_offset` over both arms; make
   `compact_run` and `drop_raw_pages` call `forget`. Red test, in
   tests/blob.rs beside `Piecewise`: give `Piecewise` an `advise_willneed`
   and `advise_dontneed_range` that count; assert `willneed_ranks(0, n,
   cap) > 0`, `willneed_dir(0, n) > 0` and `record_offset(r).is_some()`
   for a `Blob<Piecewise>`. All three fail today (0, 0, `None`). Rows: 1, 2
   (the scan's `blocks_willneed` path is out of core only, but the function
   bodies are shared).
3. **The checks skipped by the piece.** `OrdIndex::open` CRC by the piece;
   `Blob::open_with`'s row by the piece. Red tests: (a) a vault unit test
   that `put`s a kept ordered index with a wrong CRC (vault.rs has the
   `put`/`map` helpers, vault.rs:2400-2411) and asserts `OrdIndex::open`
   errs "checksum does not match"; passes today (wrongly opens). (b) in
   tests/blob.rs, a segment with one byte of the key section flipped and
   its block CRCs left whole, opened over `Piecewise`: must fail at open
   or at the read of that piece; today opens and may answer wrong. Row 6
   (open) and the load row for the ordered index's open.
4. **Advice sites to the source (R3 proper).** `warm_file`,
   `records_resident`, `z_cross`, and the plain build's `Mapped` all go
   through the source; delete `resident_share`, `swap_ahead`'s fd half,
   `drop_behind`, the free `ask_ahead`, `record_region_span`,
   `paged_resident`. Red test: a vault-backed unit test (zfile.rs tests)
   that `warm_file` on a kept form file reports `bytes + skipped ==` the
   file's length; today both are 0. The existing
   `an_ask_ahead_brings_the_cold_pages_in` (zfile.rs:7470s) is rewritten to
   ask through the source. Rows: 3 (form scan), build timing is not a row.
5. **The CPU prefetches.** Either documented as slice-only or moved behind
   `hint_lines`. No red test (speed only); G-cost rows 2 and 3 must read
   zero moved instructions in `prefetch_block` and `scan_seek`.
6. **Visibility.** Delete `MmapBytes::as_slice` (ordindex `head` takes
   `lend_at` with a token; the tests at zfile.rs:7482-7530 use `len()` and
   `resident`); token-seal `lend_at`; `Paging` to `pub(crate)`;
   `resident_pages` to `pub(crate)` with tests/zpages.rs:5325 moved to
   `Bytes::resident`; the enum accessor replaces
   `record_regions`/`record_pieces`. Add the clippy entries with their
   `#[allow]` sites. Red tests: (a) a `compile_fail` doctest in lib.rs or
   bytes.rs that calls `MmapBytes::as_slice` and one that calls
   `Bytes::lend_at` from outside; each passes today and fails to compile
   after. A plain `compile_fail` doctest passes for any error; error-code
   checking (`compile_fail,E0624`) is honoured on nightly only (not
   verified for this toolchain). Doctests run under `cargo test --profile
   checked` for an `rlib` lib (Cargo.toml:11), and none exist today in
   lib.rs, bytes.rs or blob.rs (searched `` ``` ``), so this would be the
   first. (b) clippy red input: a `lend_at` call added to zfile.rs fails
   `cargo clippy -- -D warnings`. Rows: 1, 2, 3, 6, 8, with the
   `OrdIndex::head` fast path read by function (it runs at every seek
   step).
7. **G2 (the conform pass)** is where "a reader that borrows whole fails a
   test in the clear" is held for code written later: under conform every
   `Encryption::None` store is paged with `lend = 0`, so any new caller of
   a whole accessor that answers `None` for a paged source is exercised by
   the ordinary tests. Steps 1-6 are what make the existing tests pass
   under it.

## 5. Not verified

- That a `pub struct` in a private module, used in a public trait method's
  signature, compiles under `-D warnings` with no private-in-public lint.
- That clippy `disallowed_methods` matches `Bytes::lend_at` called through
  a generic `B: Bytes` and through `MmapBytes`.
- Instruction counts of every rewrite on the plain path (G-cost rows).
- What `vault::open` returns for a kept segment, ordered index or form
  file under a cipher, and what `posix_fadvise` does on it.
- Whether `merge_drop_behind`'s dropped bytes are exposed by a counter a
  test under a cipher can read.
- That the wasm module's size does not move with the token parameter.
- That `compile_fail` doctests with error codes are checked on this
  toolchain.
