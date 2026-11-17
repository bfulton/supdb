# formsites: every site that reads a form page

Tree: `plan/tree` at 682b2c09 (read-only export). Paths are under `plan/tree/src`.
Line numbers are that tree's. Produced and reconciled by `sites.py` in this
directory (the two searches and the hand classification; `found.txt` is its
`--list` output). The design reference is forms.md R8 and design.md's
paragraph "A Regen-lazy (form) file".

## What a failed read is, today

There is one accessor of a form page, `ZFile::page_bytes` (zfile.rs:2141),
which is `self.map.slice_at(p * ZPAGE, ZPAGE)`, plus two more `slice_at`s
in `ZFile::attach` (the trailer, 2029; the resident part, 2051). The mapping
is a `MmapBytes`:

- **Plain store.** A mapped file. `slice_at` answers `None` only for a
  range past the end. A device error under a mapping is a SIGBUS, not a
  `None` (true of every mapped read in the engine; not this item's).
  Damage reaches the reader as bytes that fail the page CRC, the directory
  parse or a record parse, never as `None`.
- **Cipher store.** A paged mapping. `slice_at` -> `slice_slow` ->
  `Paging::ensure` -> `Paged::slow` -> `fill`. A chunk that fails its tag
  (or an envelope shorter than its header) zeroes the chunk, sets it
  `EMPTY`, **calls `Pager::poison`** (pager.rs:1100-1104) and returns false,
  so `slice_at` answers `None`. The zfile code then does what it does for a
  plain store (below: `ZFile::fail` and the raw form), so the read itself
  answers right; but `Entered::finish` (db.rs:17281) -> `end_slow` ->
  `Pager::check` (db.rs:19070) replaces the answer with the poison, and
  every operation of the store after it errs. A page of a form file spans
  two chunks (4096 against 4080), so one failed chunk fails two pages.

So under a cipher the `None` arms of every site below are the paths a
failed tag takes, and they are reachable **only** under a cipher (and by
the unit test's `skewed` seam with an empty buffer, zfile.rs:8567, which
drives `count` alone). Under a plain store no input reaches them.

Legend for "today on a failed read":
**F** = `ZFile::fail(p)` (page bad, file out of service) and an answer
that sends the caller to the raw form; **F-** = the same answer without
`fail` (form stays in service); **R** = re-asks the page at the next step,
which then takes F; **N/A** = holds bytes already read, asks nothing.
"Wrong answer possible" means: with the store-wide poison removed, could a
failed read of a form page produce an answer the raw form would not give.
**Ambiguous Option** marks a return type in which "did not read" and "not
there / cannot say" are one value.

## Layer 1: the mapping (zfile.rs)

| # | function | what it reads | today on a failed read | under R8 | wrong answer possible | ambiguous Option |
|---|---|---|---|---|---|---|
| 1 | `ZFile::page_bytes` 2141 | a page, `map.slice_at` (test: `skewed` buffer) | returns `None` (also `None` past the mapping's end; the `p < n_pages` filter is the caller's, and `check_sample`, `group_of_index`, `group_of_key`, `owner_span_from` and `locate` (2510) do not apply it) | `Result<&[u8], Damaged>`; past-the-end is `Damaged` too | no, by itself | **yes**: `None` = out of range or did not read |
| 2 | `ZFile::attach` 2029 | the trailer | `None` -> `Reject::Damage(ZError::Length)` | `Damaged` -> `Reject::Damage` (name the cause: not `Length`) | no | n/a (Result) |
| 3 | `ZFile::attach` 2051 | the resident part (fence, filter) copied to the heap | `None` -> `Reject::Damage(Length)` | same | no | n/a |
| 4 | `ZFile::check_sample` 2109/2129 | 16 sampled pages at attach | `page_bytes` `None` -> `Err(ZError::Length)` -> `Reject::Damage` | same | no | n/a |
| 5 | `warm_file` 3778 | one byte a page, `map.as_slice()[at]` | plain: no check (SIGBUS); cipher: `as_slice()` is empty, the loop reads nothing | nothing to answer; must stay a no-op or ignore `Damaged`, never poison | no | n/a |
| - | `ZFile::ask_ahead` 2857, `prefetch_all` 3266, `drop_cache` 3255, `file_bytes` 3245; `z_cross` (db.rs:42424, `vault::map(form)` + `advise_willneed`); `swap_ahead` (zfile.rs:3957, plain only) | advice and length; no byte read | `Paging::willneed`/`forget`/`resident` decrypt nothing and cannot fail | infallible advice on the source (forms.md R3) | no | n/a |

The `attach` mapping itself: `vault::map(path)` Err other than NotFound
(an envelope header `checked_header` refuses, a key that does not unwrap)
-> `Reject::Damage`; it does not poison today (no fill is made).

## Layer 2: pages and groups to answers (zfile.rs)

ZFile methods (each entered only while `!damaged`, else `Out`):

| # | function | reads | today on a failed read | under R8 | wrong answer possible | ambiguous Option |
|---|---|---|---|---|---|---|
| 6 | `open_group` 2178 | `page_bytes` (2188), then view, memo, `first_decode`/`first_checks`/`verified_group_in` | F, `None`; `Fail::Misaligned` -> F- `None` | `Result<Opened, Damaged>`; Misaligned stays its own variant | no | **yes**: `None` = failed, misaligned |
| 7 | `view_of` 2362 | `page_bytes` + `view` | F, `None` | `Result<PageView, Damaged>` | no | yes (failed read = failed parse; both damage) |
| 8 | `group_of_index` 2376 | `page_bytes` + `view` | F, `None` | Result | no | yes (same) |
| 9 | `group_of_key` 2392 | `page_bytes` + `view` | F, `None` | Result | no | yes (same) |
| 10 | `locate_rank` 2421 | `view_of`, `open_group` | `Locate::Fallback` (F inside) | from the Results | no: `Absent` only for `rank >= nkeys` | no (enum) |
| 11 | `locate` 2463 | `view_of`; coded path: `page_bytes` again (2510) + `first_checks`, `view.payload`; else `open_group` | `Fallback` (F); `Misaligned` F- | from the Results | no: `Absent` only from the filter, the fence, a `ZNOPAGE` entry, or a seek over a page that read | no (enum) |
| 12 | `read_all` 2583 | `locate` + `parse_record` | `ZRead::Fallback` | `Fallback` | no | no |
| 13 | `count` 2618 | `locate` | `Fallback` | same | no | no |
| 14 | `scan_at` 2661 / `scan_in` 2690 | `view_of` 2743, `open_group` 2753, payload, `record_key_next`/`parse_record` | `ScanEnd::Fallback` at a record boundary with `seen` exact (F) | same | no: records before the stop were emitted whole, the caller walks raw from `seen` | no |
| 15 | `values_at` 2877 | `locate_rank` | `Fallback` | same | no | no |
| 16 | `key_at_into` 2924 | heap fence for a `ZNOPAGE` entry; else `locate_rank` | `Fallback`; `out` untouched | same | no | no |
| 17 | `seek` 2974 | `view_of` 3007; coded: `page_bytes` 3022 again; else `open_group` | `ZSeek::Fallback`. At 3021-3022 a `None` from `page_bytes` (or `payload`) is **not** taken as failure: it falls through to `open_group`, which asks again and takes F | `Damaged` at 3022 -> `Fallback` directly | no (R: the re-ask decides) | no |
| 18 | `cut_in` 3133 | `seek` | as `seek` | same | no | no |
| 19 | `owner_span_from` 3100 (and `owner_span` 3089) | `page_bytes` + `view` 3114 | F, `None`. `v.group(j)?` (3119) answers `None` **without** `fail` (a directory index that does not resolve on a page that read) | `Result<Option<span>, Damaged>`, or `Damaged` folded into "cannot say" explicitly | no: `None` sends callers to the exact resolve (`lazy_block`) or the seek (`scan_blocks`) | **yes**: `None` = did not read, out of service, a several-record `ZNOPAGE` entry, an unresolved group |

`ZCursor` (the merge path, the overlay walk, the block cache's reader).
It holds the open group across calls as `Group::Page(&'a [u8])` or
`Group::Coded(&'a [u8], _)`, borrows of the mapping, so its later reads of
the group ask nothing:

| # | function | reads | today on a failed read | under R8 | wrong answer possible | ambiguous Option |
|---|---|---|---|---|---|---|
| 20 | `ZCursor::seek` 457 | `group_of_key` 483, `open_at`, `record_group_seek`/`group_seek` over the held group | `Err(ZSeek::Fallback)` (F); caller seeks raw | same | no | no (Result) |
| 21 | `ZCursor::at` 429 -> `load` 667 | `entry_at_rank` (heap), `group_of_index` 686, `open_at` | `fall()`: the rest of the walk is the raw form's | same | no | no |
| 22 | `open_at` 551 | `page_bytes` + `view` (572), `open_group` (575) | `page_bytes` `None` gives `view` `None`, then `open_group` re-asks (R) and takes F; `fall()`, false | `Damaged` -> fall | no | bool: false = fell (any cause) |
| 23 | `next_group` 622, `resume` 1005, `advance` 786 | `open_at`, `place` | fall | same | no | no |
| 24 | `place` 637 | the held group (N/A) | parse failure -> F, fall | unchanged (no ask) | no | no |
| 25 | `group` 530 | the held group (N/A) | - | unchanged | no | no |
| 26 | `key` 715 | held group / `raw.key_at` | N/A: `group().get(at..at+len)` is over bytes `place` parsed | unchanged | no **while the borrow is live** (see note A) | **yes**: `None` = past the end; over bytes already read it cannot mean "did not read" |
| 27 | `values` 732 | held group, `parse_record`/`coded_record_to`; raw values | record that does not parse -> F, raw `values_at` | unchanged | no | no |
| 28 | `walk_until` 816 / `walk_run` 838 / `walk_impl` 847 | held groups, `next_group`, `ask_ahead` | damage -> F, `fall`, the walk goes on raw from the exact rank | same | no | no |
| 29 | `skip_to` 1035 / `advance_until` 1112 | `group_of_key` 1068, `open_at`, held groups | `None` after `fall` (F inside, or F- for a fence entry that does not decode) | same | no | **yes**: `None` = cannot say (fell) |

Not form reads, though a search finds them (OTHER in `sites.py`): the
builder (`build_file_seen`, `write_pages`: reads the **segment**; verifies
its own stack page with `verify_page_with`, never the file), the sampler
(`sample`, `sample_of`: reads the **segment**, parses its own stack page),
`Held::{ahead,passed,done}` and `Mapped` (the segment's mapping),
`Memo::touch` (its own `map` local), the pure checkers in zfile.rs and
zpages.rs (`first_decode`, `first_checks`, `verified_group_in`, `view`,
`parse_page`, `page_view`, `view_trusted`, `coded_*`), which only take bytes
handed to them. zpages.rs has no `MmapBytes`, no `slice_at` and no ZFile.
blob.rs: `read_exts_inline` reads the **segment's** blocks for a run a form
record names; the inline tail it is handed is form bytes already read.

## Layer 3: the dispatch that reads the failure (db.rs)

Every one of these sends the read to the raw form (or the ordered index)
on `Fallback`/`Out`/`NoPage`/`None`/a fallen cursor; under a cipher, the
raw read of the **segment** still poisons on a failed segment chunk.

| # | function | form call | on a form failure | ambiguous |
|---|---|---|---|---|
| 30 | `part_read_z` 20591 | `ZFile::read_all` | raw read | no |
| 31 | `part_count_z` 20634 | `count` | raw count | no |
| 32 | `part_scan_z` 20680 | `scan_at` | raw walk from `rank + r.seen` (`part_scan_rest`) | no |
| 33 | `part_seek_z` 20752 | `seek` | ordered index | no |
| 34 | `part_cut_z` 20780 | `cut_in` | `cut_raw` | no |
| 35 | `part_values_z` 20809 | `values_at` | raw values | no |
| 36 | `cache_seek_exact` 20931 | `seek` | heads + raw | no |
| 37 | `lazy_block` 20994 | `owner_span_from` | `None` -> exact resolve | yes (Option, all causes go raw) |
| 38 | `cache_key_at` 21040 | `key_at_into` | raw `key_at` | bool: false = no key at the rank, made by the raw form after a fallback |
| 39 | `pwalk_at` 21079 | `ZCursor::at` | the cursor falls inside | no |
| 40 | `pwalk_seek` 21099 | `ZCursor::seek` | ordered index | no |
| 41 | `scan_partition_cursor` 27413 | `walk_until` | cursor falls, walks raw | no |
| 42 | `emit_unsealed` 27470 | `PartVals::Cursor(c) => c.values`, else `part_values_z` | as 27 / 35 | no |
| 43 | `scan_blocks` 25965 (26118) | `owner_span` directly (`zpages_seek_span`) | `None` -> `scan_seek` | yes (as 37) |
| 44 | `PartReader::{cursor_at 38033, scan 38077, scan_until 38101, cut 38133, values_at 38180}` | `ZCursor::at`, `walk_run`, `skip_to`, `values` | `fell_back` -> close, `on = false`, dispatch (`cache_*`); `skip_to` `None` -> `cache_cut` | `cursor_at` `None` (no form / fell) |
| 45 | `PWalk::{key 38237, values 38245, advance 38254}` | `ZCursor::{key, values, advance}` | as 26-23 | `key` `None` = end |
| 46 | `z_attach` 33265, `z_give` 33332, `z_collect` 43131 (four `attach` calls: 33282, 33387, 43229, 43276, 43347) | `ZFile::attach` | `Reject::Damage` -> counted, `z_declined = REFUSED`, partition raw, a build replaces the file | no |
| 47 | `zpages_settle_with` 33495 (33578), `z_warm` 43423 (43437) | `warm_file` | result ignored or counted | no |

Above layer 3 (ABOVE in `sites.py`: `cache_scan`, `cache_values`,
`cache_cut`, `scan_seek`, `cache_owner_of`, `block_bounds`,
`scan_partitions`, `scan_merged`, `count_in`, `read_all_in`, `settle_each`,
`fold_piece_block`, `make_table`, `cuts_of`, `table_bounds`, `snap_bounds`,
`filed_cuts`, the block builders `walk_block`, `walk_deltas`,
`walk_deltas_over`, `emit_over`, `deltas_for`, `copy_from_deltas`,
`patch_block`, and the callers of `cache_owner_of`): they receive answers
the dispatch already took from the raw form where the form failed, never a
failure. No merge, seal or other job whose output is durable reads a form
file: the merges read `seg.raw()` and the pieces, so a form read never
reaches a manifest.

## Note A: borrows held across calls

`ZCursor` holds `Group::Page`/`Group::Coded` borrows of the mapping, and
`ZFile::open_group` hands out `Opened::Borrowed`/`Opened::Coded` borrows
for a stored or raw-tail group. Under a cipher those are pager memory: valid
while the reader's pin stands. Not a damage path (the bytes were checked
when asked), but a page layer must keep them valid for the cursor's life
or until a rest, and a cursor must not outlive a rest. `scan_rested`
(db.rs:21652) rests between `scan_body` calls, after the cursors (locals of
the body) are dropped; the block builders' `build_rest` was not traced here.

## Reconciliation

`python3 -I sites.py <tree>/src` (see notes.md for the exact command and
output): A finds 63 functions, B 123, union 124; every one is classified;
62 READS (the rows above, counting each ZCursor/PartReader/PWalk method);
61 found by both, one by B only (`ZCursor::group`, which no accessor name
reaches because it is a match over the held borrow; B's root is the
`Group::Page(`/`Group::Coded(` borrow of the mapping). Mutants of the tree
that add a reader of the form under a new name are caught (notes.md).

## The smallest sound change for a cipher store today (before the page layer)

**Yes: a failed tag on a form file's chunk can be answered as "that form
file failed", with the store reading the partition raw as a plain store
does, without weakening a failed tag anywhere else.** The read side needs
no change for soundness: every site in layers 2 and 3 already answers a
`None` from `page_bytes` by `ZFile::fail` and the raw form (the table,
rows 6-47; the two places that do not `fail` at once, 17 and 22, re-ask
and fail at the next step). What makes the store refuse today is only the
latch the pager sets before `slice_at` returns `None`.

Where the pager tells the two apart: **at the fill's failure in
`Paged::slow`** (pager.rs:1099-1104), the one call of `Pager::poison`.
The class belongs with the envelope the chunk is decrypted from, so it is
set where the envelope is attached:

1. `Back` (pager.rs:719) gets a field `fails: Fails` (`Store` or
   `File`). `Vault::back_of` (vault.rs:907) sets `File` exactly when
   `disk.kind == Kind::Form` and `Store` for every other kind. A fill with
   no `Back` ("a chunk is gone and the file has no envelope") stays
   `Store`: it is an engine invariant broken, not damage.
2. In `Paged::slow`'s `Err(e)` arm: with `Store`, as today; with `File`,
   set the chunk `EMPTY`, do **not** poison, bump a per-file failure count
   (a `PagerCounts::form_failures`, and a flag on the `Paged`) and return
   false. `ensure`/`piece_from` then answer false, `slice_at` `None`, and
   the zfile code takes the form out of service. A second ask decrypts
   again and fails again; nothing is cached as `HOT`.
3. Nothing in zfile.rs or db.rs changes. The controller sees
   `damaged()` and rebuilds (`ZKind::Rebuild`, db.rs:42841); a rebuild is
   written under a temp name and renamed over (`Vault::rename` gives the
   new name a fresh entry, so a fresh `Paged` and a fresh `Back`), while a
   reader still holding the old `ZFile` keeps the old view.

Why this does not weaken any other file's failure: the class is read from
the vault's own `Disk.kind` for the envelope a chunk comes from, and a
`Paged` is shared between names only for one `Disk` of one kind
(`Disk::same` compares token and kind, vault.rs:500). The raw reads a form
fallback makes go to the segment's own `Paged` (`Kind::Seg`), which still
poisons; so do the ordered index, the log and the manifest. No job whose
output is durable reads a form file (layer 3 note), so a form failure can
never reach a merge's outputs or a manifest, and the merge's `vault::check`
keeps its meaning. An attacker who renames a segment's envelope to the
form's name (`x-T` to `x-T.z`, same key, since the form's `Disk` copies
the segment's wrapped key and the AAD does not bind the kind) gets a form
whose trailer the attach refuses, which is what a plain store does with a
file of the wrong content; the segment's reads still go through `Kind::Seg`.

What the store gives up: today a changed form chunk under a cipher stops
the store, which is louder than the plain store. After the change it is
counted (`bad_pages`, `zfallbacks`, the new per-file count) and repaired,
as the plain store does. Whether a failed tag on a form file should also be
surfaced to the caller is a policy question, not a soundness one; this
change makes it the plain store's policy.

### The tests

- **T1** (rewrite of `a_changed_byte_in_a_form_file_is_refused_or_the_form_is`,
  tests/cipher.rs:1941): every flipped byte of every `.z` envelope opens
  (`refused == 0`), reads the whole store as `want` (`unread == 0`), then
  writes, commits, flushes and reads again `Ok` (no latch); summed over
  the flips, form files refused at attach > 0 **and** reads that met a
  failed page > 0 (`zpage_counts().bad_pages`/`zfallbacks`, and the
  pager's per-file count > 0), so a run whose flips never reached a page
  read fails.
- **T2** (the control, a store with form files in service): flips in the
  segment envelopes and the `.o` files still give only refused / "did not
  read" / a state the store acknowledged, with "did not read" > 0 for the
  segments. The existing `a_changed_byte_is_refused_or_is_where_the_log_ends`
  (tests/cipher.rs:888) runs `tiny(3)`; whether that store has form files
  was not checked, so T2 is a separate test with `ZPages::Always`.
- **T3** (pager unit, beside `a_changed_chunk_fails_its_read_and_poisons_the_store`,
  pager.rs:2034): a `Paged` whose `Back` is `File`: a changed chunk fails
  `ensure` twice, `Pager::check` stays `Ok`, the chunk is not `HOT`, the
  count is 1; the same chunk under `Store` poisons.
- **T4** (zfile unit, the R8 per-site test the tree lacks): a seam that
  makes `page_bytes` answer `None` for one page, before and after that page
  is verified (memo off and on), driving `read_all`, `count`, `scan_at`,
  `values_at`, `key_at_into`, `seek`, `cut_in`, `owner_span`,
  `ZCursor::{at, seek, skip_to, walk_until, walk_run, values}`: each answers
  `Fallback`/`Out`/`None`/fell, never `Served` with an absence, and the file
  is `damaged()`. Today only `count` meets an empty source (zfile.rs:8567).
  This is the test that holds the `None` arms, which no plain-store input
  reaches.

Mutants each must catch: classifier `File` for every kind (T2 and the
existing tamper test red); classifier `Store` for every kind, which is
today (T1 red); the `File` arm returning true over the zeroed chunk (T3
red; T1 may pass, since a zero page fails its CRC unless the page was
verified and trusted earlier, so T3 is the one that holds it); `Back` built
for a form under `Store` because the class was taken from the name at
`adopt` before the envelope existed (T1 red for a form built in this open);
any one `None` arm in rows 6-29 answering an absence instead of failing
(T4 red). None of these mutants was run: no cargo here.

## What the page layer's interface must offer these sites

Only what the sites need, not its design:

1. A read of a page of a Regen-lazy file answers `Result<&[u8], Damaged>`
   (`#[must_use]`), where `Damaged` covers a failed tag, a short envelope
   and a range past the end, and is the file's alone: never latched
   store-wide, counted per file.
2. The class (store-fatal against file-local failure) fixed when the file
   is opened by its kind, never by a name the engine can rename, and
   carried to every view of that file.
3. Borrows that stay valid for as long as the caller's pin, since
   `ZCursor` and `Opened` hold page borrows across calls (note A).
4. Infallible advice and length on the same handle (`willneed`, `forget`,
   `resident`, `len`), since `ask_ahead`, `prefetch_all`, `drop_cache`,
   `file_bytes`, `z_cross` call them on a file that may be damaged.
5. A per-file failure count and the out-of-service state readable by the
   controller (today `ZFile::damaged`, `bad_pages`), so a rebuild is
   scheduled; and a rename over the file that gives the new file a fresh
   handle while the old one's readers finish.
6. The attach's three reads (trailer, resident part, sampled pages)
   answering `Damaged` the same way, so an attach refuses (`Reject::Damage`)
   and the store goes on.
