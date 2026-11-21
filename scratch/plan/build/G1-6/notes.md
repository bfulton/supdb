# G1-6 `advice`: notes

Base (HEAD when the unit began): f9e83900fb1ef8291406f7e09dc8ca8354dd4bbb.
Private TMPDIR: /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-6

## Step now
Repair round 2: the reviewers' nine mutants run (mutants.md, "Repair round 2"; runner
mutants_r2.py, verdicts mutants-results-r2.txt), two tests and a test phase added, the strace
comparison made a gate with an exit status (strace/gate.sh, gate.py), then lists + lint and the
full checked test run (r2-checks.log). This round changed tests only (zfile.rs's test module,
tests/zpages.rs): no compiled engine code, so the cost rows were not counted again (cost2.log
stands). The finished copy of every changed file is the working tree, identical to keep/
(keep/SHA256SUMS); keep-r1/ is round 1's, keep-r0/ round 0's.

## What the tree has that the plan read differently (682b2c09 -> f9e83900)

- `MmapBytes::resident_count` (pub, Option<usize>) exists beside `resident_pages`
  (its `unwrap_or(0)`); caller `zfile::read_back_residency` (stride read-back,
  new since the plan). It answers `Some(0)` for a paged mapping (lend = 0): a
  silent "none resident" -- the same class as `resident_pages`.
- `resident_share` callers beyond the plan's two: `Seg::redrop_ord_pages`
  (the ordered index's second drop, over `OrdIndex::mapped()`, which is
  `slice_at(0, len)`: under a cipher that decrypts the whole index and then
  mincores the memory file), `ZFile::pages_resident` (same shape over a form
  file's whole `slice_at`), and `map_resident_share` (whole mapping:
  `paged_resident` or `resident_share(as_slice())`) with three callers in the
  hot-raw/settle-asks paths (`54814`, `55450`, `55455` at the base).
- `record_region_span` has five callers (MergeAhead::new, drop_record_pages,
  the asked-files' settle (`Asked.records`), `z_cross`, the dropped-file ask
  at a build's collect) and one test caller (tests/blob.rs).
- `swap_ahead` has three callers (z_cross, the asked-files settle, the
  dropped-file ask); `ask_file` (File::open of a form file by path) two
  (swap_ahead, the dropped-file ask at an install).
- The plain build has two `Walked`-like address users: `Mapped` (block form)
  and `write_stride` (the stride form, new since the plan), each with
  `base`, `recs_at`, `dir_at` by pointer subtraction, `drop_behind(base..)`
  and `ask_ahead(base..)`.
- `ordindex::heads_all_resident` uses `resident_pages`.
- Tests: bytes.rs's resident_pages test, pager.rs's paged_resident line,
  zfile.rs tests (an_ask_ahead..., a_warm..., the drop tests computing
  `recs_at` from `as_slice().as_ptr()`), tests/zpages.rs (resident_pages),
  tests/blob.rs (record_region_span), tests/source.rs token line
  (`m.paged_resident(0)`).

## Site list (every residency or advice call on an address or a borrowed slice)

Found by `grep -n 'resident_share(\|map_resident_share(\|resident_pages(\|paged_resident(\|
record_region_span(\|resident_count(\|as_slice()\|\.mapped()\|as_ptr()' src/` and read
beside every `madvise`/`mincore`/`posix_fadvise` in src/db.rs, src/db/zfile.rs, src/blob.rs,
src/ordindex.rs. Each, and what it became:

| site | before | after |
|---|---|---|
| `bytes::MmapBytes::paged_resident` | Paging::resident for a paged map, None for a mapped file | deleted; `Bytes::resident_share` (one method, both kinds) |
| `bytes::MmapBytes::resident_pages` / `resident_count` (pub) | mincore by offset clamped to `lend`: 0 / Some(0) for a paged map | deleted / made the private `mapped_resident` (the mapped arm of `resident_share`) |
| `db::resident_share(&[u8])` | mincore over a borrowed slice's address | deleted (door allow removed) |
| `db::map_resident_share` (3 callers: hot-raw back ask, settle-asks resident sum and re-ask) | paged_resident or resident_share(as_slice()) | `Bytes::resident_share(map, 0, len)` |
| `Seg::records_resident` | two paths (record_regions slice / paged_resident) | one path: record_region_place + `resident_share` of the source |
| `Seg::redrop_ord_pages` | resident_share(ord.mapped()) -- under a cipher a whole slice_at of the index, i.e. the whole index decrypted, then mincore of the memory file | `OrdIndex::resident_share()` asking its source |
| `OrdIndex::mapped()` | whole slice_at | deleted; `OrdIndex::resident_share` |
| `OrdIndex::heads_all_resident` | resident_pages (0 for a paged map: always false) | `resident_share(HEADER, n*HEAD) == Some(1.0)` |
| `ZFile::pages_resident` | resident_share(slice_at(0, len)) -- under a cipher decrypts the whole form file | `resident_share(0, len)` of the mapping |
| `zfile::warm_file` | `as_slice().len()` (0 under a cipher), resident_pages, touch through as_slice | `map.len()`, `resident_share` per chunk, `advise_willneed`, touch through `lend_at` (a paged map lends nothing: no touch, no decrypt) |
| `zfile::read_back_residency` | resident_count (Some(0) for a paged map) | `resident_share(0, len)`, `== Some(0.0)` |
| `zfile::ask_ahead` (free, madvise(base+a)) | address | `ask_ahead(map, from, to)` = `map.advise_willneed(from & !(ZPAGE-1), ..)` on Linux, a no-op elsewhere (as at the base; repair round 1); no libc, door allow removed |
| `zfile::drop_behind` | madvise(base+a) + fadvise(fd) | `map.advise_dontneed_range(a, b-a)` + fadvise on the descriptor (S1) |
| `Mapped` / `write_stride` (`base`, `recs_at`/`dir_at` by pointer subtraction) | address | `recs_at` from `record_region_place`, `dir_at` from the new `pub(crate) Blob::record_dir_place`; `Placed` carries map+regions+offsets to write_stride |
| `build_file_seen`'s `base` | `map.as_slice().as_ptr()` | gone |
| `Blob::record_region_span` (base by slice_at(0,1)) | 5 callers + tests/blob.rs | deleted; callers: `MergeAhead::new` and `Db::drop_record_pages` take `record_region_place` (both kinds); the three descriptor sites (z_cross, the asked-files settle, the dropped-file ask) take `Seg::records_by_descriptor` (record_regions()? then record_region_place: Some only for a reader that holds its section whole, the one whose file has a descriptor) |
| `Blob::raw_read_span` | `self.flat()?` (None for a paged reader) | from the header words for both kinds |
| `zfile::ask_file` | `File::open(form)` | `vault::open(form)` (one load of VAULTS; same openat) |
| `zfile::swap_ahead` | descriptor half, offsets from record_region_span | unchanged body, offsets from record_region_place |

Left, by name:
- `Blob::{prefetch_ranks, prefetch_ranks_est, prefetch_dir}`, `OrdIndex::seek_exact_hint`, the
  db.rs `prefetch_lines` sites: CPU-cache prefetches by address, no advice to the kernel
  (borrows notes 2 item 8): speed only, no wrong answer; a no-op for a paged reader. G1-7/G2.
- `MmapBytes::{advise_dontneed, advise_dontneed_range, resident_at}`: by offset inside bytes.rs
  (the seam), already both kinds.
- `populate_ahead`'s madvise: the writer's anonymous buffer, no file (the launch note).
- descriptor-based advice: `compact_run`, `Seg::drop_raw_pages`, `Db::drop_record_pages`'s
  fadvise, `swap_ahead`'s first half, `drop_behind`'s fadvise, `ZFile::drop_cache`,
  `drop_written_range`, `drop_synced`: S1's typed handles (plan "Leaves").
- whole-borrow reads of a form file that are not advice: `zfile.rs` `as_slice()` at
  `file_facts`/`time_file_decodes`/`check_stride`/`StrideBody::open`/`ZFile::bytes` and a test:
  G1-7 (R2), with `OrdIndex::{head, open, mapped_heads}`'s `as_slice`.

## Step log
- base strace: zpages test binary at f9e83900 copied to zpages-base.bin; strace/run.sh
  (7 tests, -y, madvise/fadvise64/mincore/openat/mmap/munmap), normalize.py places each
  madvise/mincore in the file mapping it falls in. before1..3 = three runs of the base
  (they differ between themselves: background threads; see the comparison rule below).
- red at the base (before.log): pager `a_paged_mapping_and_a_mapped_file_say_alike_what_is_resident`
  (written against the API the tree had: `resident_pages`, 0 against 256), ordindex
  `the_heads_resident_are_asked_of_the_source_for_either_kind`, zpages
  `a_warm_under_a_cipher_asks_for_the_envelope_and_decrypts_nothing` (warms 1, warm_bytes 0,
  warm_skipped 0). The pager test's final text asks `resident_share`; its assertion is the same.
- change made; all targets compile under checked with no warning (norun.log); the red tests and
  the zfile/pager/ordindex/bytes neighbours green (after-unit.log, after-zp.log).
- zpages-after.bin copied; strace after1..3 running.
- strace: after1..3 of the head binary; compare.py over before1..3/after1..3 (strace/compare.txt).
  The normalizer first dropped anonymous-memory madvise by placing each call in the file mapping
  it falls in (mmap with -y), and had to join strace's unfinished/resumed lines and drop
  overlapped mappings before the three base runs agreed with each other (a stale mapping read
  a merge's madvise as another file's). With that, every test's set of call shapes is the same
  in all three base runs, and the head's is the same set but for one test (see Proof below).
- lists: couplings lowered (bytes.rs paged_resident 2 -> resident_share 1; db.rs's two G1 lines
  gone; ordindex's test scopes moved by the InMemory lift, totals the same), door.txt lowered
  (resident_share and ask_ahead allows gone, zfile <file> cfgout 2 -> 1 [undone in repair round 1: back at 2], two reasons
  restated), cost-entries.txt lowered (Blob::record_region_span gone). No raise anywhere.
  `sh scripts/check.sh lists lint`: green (lint-lists.log).
- cost: `COST_DIR=$B/cost1 COST_BASE_TARGET=$B/costbase-target sh scripts/cost.sh f9e83900 read
  read_z scan scan_long scan_z scan_long_z merge land` -> `cost: green`, EXIT 0 (cost1.log).
  Base target removed after (4.7 GB).
- Added since the cost run, tests only: tests/blob.rs Advised forwards resident_share;
  tests/cipher.rs a_merge_under_a_cipher_asks_ahead_for_its_cold_inputs; ordindex test
  InMemory given a share. keep/ refreshed (keep/SHA256SUMS); mutants.py running
  (mutants-results.txt, one line a mutant as it comes; mut-<key>.log each).
- mutants: 13, all killed (mutants.md, mutants-results.txt). O9 survived its first run; the
  missing test was added to tests/blob.rs (`holds_to_the_file`: a raw read's span) and O9 run
  again alone, killed. Tree put back and checked against keep/ by sum and byte for byte.
- after.log: `cargo test --profile checked --no-fail-fast`, 1533 passed, 0 failed, EXIT 0
  (started 17:16:59Z).
- stride.log: `sh scripts/check.sh stride` running (started 17:27Z).

## Proof that a plain store did not move (strace)

`strace/run.sh BIN OUT` runs seven tests of the zpages binary, each alone, under
`setarch -R strace -f -y -e trace=madvise,fadvise64,mincore,openat,mmap,munmap`:
the crossing (`builds_ahead_at_landing_start_past_the_percent_and_only_from_resident_records`,
which builds ahead, drops past the line and swaps at the crossing through `z_cross`), the build's
drop behind (`the_build_drops_the_raw_records_it_passed_when_asked_to_and_not_otherwise`), the
dropped file asked back at a build's collect and install (`a_build_in_flight_at_the_crossing_has_
the_file_it_dropped_asked_back`: `swap_ahead`, `ask_file`), the warm
(`an_attached_form_file_is_warmed_once_per_process_unless_the_warm_is_off`), the ordered index's
second drop (`a_served_partitions_ordered_index_is_dropped_again_a_second_on`:
`OrdIndex::resident_share`), a merge's drops (`a_merge_drops_the_raw_records_of_an_input_its_form_
serves`) and the stride switch (which builds nothing in this binary: one shape, the same).
`normalize.py` keeps each file advice as (call, file with digits folded, offset in the file,
length, advice, result); `compare.py` compares the three base runs with the three head runs.

Result (strace/compare.txt): in six tests of seven every call shape the head makes the base
makes, and every shape the base makes in all three runs the head makes; counts differ inside the
base's own run-to-run range or by a run or two (RANGE lines: background builds and drops whose
number depends on timing). In the merge test the head made four shapes in one run of three that no
base run made -- a build's asks (`4096 131072 MADV_WILLNEED`, `135168 25720 MADV_WILLNEED`) and
drops (`8192 151552 MADV_DONTNEED` and its fadvise) over a partition named with an open upper
fence (`par-N-N-NbNdN-.sup`) -- the same offsets and lengths the base makes over that test's
other partitions: which partition a background build took in that run, not a new call. GONE: none.
So the build's `ask_ahead` through `advise_willneed` issues exactly the base's madvise pieces
(page-aligned start, `WILLNEED_PIECE` pieces), `drop_behind` the base's madvise and fadvise,
`z_cross`/`swap_ahead`/`ask_file` the base's fadvise and openat, and the residency questions the
base's mincore ranges. The plan's "not verified" item (that advise_willneed and ask_ahead issue
the same calls) is answered yes, given the start aligned down by the caller (memmap2 would
otherwise widen the first piece by the remainder).

Not traced: the stride form's plain build (`write_stride`'s ask and drop), which no test of the
zpages binary builds; it calls the same `ask_ahead` and `drop_behind`, and its offsets are held by
`a_stride_file_is_the_same_bytes_whatever_the_piece_and_the_window` and
`a_stride_walk_that_declines_alone_gives_back_what_it_read` against offsets the tests take by
pointer subtraction (an oracle independent of the header-derived ones).
- stride.log: `sh scripts/check.sh stride`: 1537 passed, 0 failed, `all checks passed: stride`, EXIT 0.
- wasm.log: `sh scripts/check.sh wasm`: green (a public trait gained a method).
- bench.log: `sh scripts/check.sh bench`: green, EXIT 0.
- lint-lists2.log: `sh scripts/check.sh lists lint` on the finished copy: green, EXIT 0.

## What the change holds, and what it cannot see

- One span residency question, `Bytes::resident_share(off, len) -> Option<f64>` (default None),
  answered by `MmapBytes` for both kinds: a mapped file by mincore over the pages by offset
  (`mapped_resident`, the old `resident_count` made private), a paged mapping by
  `Paging::resident`. The three it replaces are deleted (`MmapBytes::paged_resident`,
  `MmapBytes::resident_pages`, `db::resident_share`), with `resident_count`'s public face and
  `OrdIndex::mapped()`. No residency or advice call outside bytes.rs takes an address now but the
  CPU prefetches (left, above).
- Held by tests: the two kinds agree (pager unit test, real envelope against a plain file); the
  heads' residency is asked of the source (ordindex, an InMemory source at 1.0, 0.99, 0.0); the
  warm under a cipher measures the file, asks for the envelope's cold pages and decrypts nothing
  (zpages, store level, Always); a merge under a cipher asks ahead (cipher); a raw read's span and
  the region's place are the header's for both kinds (blob); and the plain store's advice by the
  strace, above.
- Cannot see: the pager has no count of the advice it is given (Paging::willneed bytes asked,
  forget calls, residency asks), so what the crossing and the build ask of it under a cipher is
  held only by outcomes (O7's test: the crossing under a cipher swaps; the warm's own counts). The
  seam owed is a count of those three in `PagerCounts`: S5, by name.
- `ask_ahead`'s page alignment of the start is held by the strace alone (mutants.md).
- The warm's bytes for a paged mapping are an estimate (chunk share times pages); held only to be
  positive and under the file, and to be most of it after the envelopes are dropped.

## What starts to work under a cipher (decision 16, rule 2), on paths no default reaches there

Each is behind the compact form (`ZPages::Always`, or `Auto` with `zpages_under_cipher`, neither a
default under a cipher: `cipher_controller` turns `Auto` off) unless said:
- `warm_file`: asks the kernel for the envelope's cold pages (it measured the file as empty);
  priced at S5. Verified by reading: with the defaults a store under a cipher has `ZPages::Off`
  after `cipher_controller`, so `ZKind::Warm` is not reached (the plan's "not verified" item).
- `ZFile::pages_resident`, `Seg::redrop_ord_pages`: no longer decrypt the whole form file / the
  whole ordered index to ask whether it is resident; they ask the pager. S5.
- `OrdIndex::heads_all_resident`: answers by the pager where it answered "no" always (the settle's
  asks out of core, behind `pieces_willneed`, the gate away from raw). S5.
- `Blob::raw_read_span`: the hot-raw release's ask of the raw pages now asks the ciphertext. S5.
- `MergeAhead::new`: a merge's read-ahead (`Options::merge_read_ahead`, default 0, under no
  cipher default either) asks the inputs' ciphertext. Decision 16 names the compact form and
  `merge_drop_behind`; this is a third arm of the same kind, priced by whoever turns
  `merge_read_ahead` on (S5's pairing or that arm's own). Pairing owed (no timing here):
  `bench/scripts/pair.sh supdb-cipher64 supdb-cipher 1000000 --reps 4` with an arm that sets
  `merge_read_ahead`, at this commit and at f9e83900; and G1-4's owed
  `bench/scripts/pair.sh supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes read`.
- `Db::drop_record_pages` (a test seam): now drops and counts a paged reader's records.
- `zfile::read_back_residency`: answers for a paged mapping, but no stride file is built under a
  cipher (`StrideNo::Kept`): unreachable there.

## Departures from the plan, each with its reason

1. The method is `resident_share(off, len) -> Option<f64>`, a share, not a page count: the paged
   side has no pages to count (`Paging::resident` is a share of chunks), and a share is exact for
   the mapped side (a ratio of two page counts; `== Some(1.0)` iff all, `Some(0.0)` iff none).
   `resident_count` became the private `mapped_resident` (the mapped arm), not deleted.
2. The warm's red test is store level in tests/zpages.rs (`ZPages::Always` under a cipher), not a
   vault-backed unit test in zfile.rs: zfile.rs has no cipher token in its test region, and a test
   there naming a cipher raises the wall (couplings.txt) for nothing a store-level test cannot
   hold. The paged/mapped agreement test is in pager.rs's tests, not bytes.rs's: bytes.rs has no
   Paging to make a paged mapping from; pager.rs has the stand-in cipher and the envelope.
3. The three descriptor sites (z_cross, the asked-files settle, the dropped-file ask at a build's
   collect) need the reader's kind, which `record_region_span`'s None gave. They ask
   `Seg::records_by_descriptor` (record_regions()? then record_region_place): one place for G1-7's
   enum accessor to replace. A dispatch on `vault::open`'s answer was rejected: a kept file still
   in memory opens as its memory file, which would send the swap to it; `vault::kept` would raise
   the wall in db.rs.
4. The warm's page touch goes through `Bytes::lend_at` (lent on the bounds check alone, nothing for a
   paged mapping). G1-7 seals `lend_at` to blob: it has to give `warm_file` its touch another way
   (a token of zfile's, as it plans for `OrdIndex::head`), or a populate on the source. Left to
   G1-7 by name.
5. Sites the plan did not name, found by the list: `OrdIndex::mapped()` and its caller
   `redrop_ord_pages`, `ZFile::pages_resident`, `read_back_residency`, `heads_all_resident`,
   `write_stride`'s base (the stride form, new since the plan), `raw_read_span`.
6. `ask_ahead` is kept as a function over the mapping (aligning the start, then
   `advise_willneed`), not inlined at its two callers: the alignment is what keeps the plain build's
   syscalls the base's, and it is written once.
7. A new crate-private accessor, `Blob::record_dir_place` (the build needs the directory's file
   offset, which only pointer subtraction gave); `pub(crate)`, so no cost-entries line.
8. `tests/source.rs`: the door test that moved `ask_ahead`'s allow now moves `drop_behind`'s
   (`ask_ahead` has no libc call left and no allow); the lexer test's synthetic `paged_resident`
   word became `paged_view` (it tests the `paged_` pattern, not the method).

## Not verified

- The pager's advice under a cipher by count (no seam; S5, above).
- The stride form's plain build under strace (no test of the zpages binary builds one).
- macOS/aarch64 (round 1: `ask_ahead` is Linux-only again, so the plain build asks nothing
  there, as at the base): `mapped_resident` keeps the old `resident_count`'s cfg (none: libc's mincore
  exists there); `drop_behind`'s madvise now goes through `advise_dontneed_range`, which is
  Linux-only in bytes.rs, as `drop_behind`'s body was. Not built for those targets here (the
  cross-arm group was not run).
- No timing of any kind (rule 7).

## Candidate rule for CLAUDE.md (left for G1-8)

A syscall trace of a multi-threaded run is compared by call shape only after its
`<unfinished ...>`/`<... resumed>` pairs are joined and each madvise is placed in the file
mapping it falls in (mmap traced with -y, overlapping maps dropped): before that, three runs of
one binary disagreed with each other on a third of their shapes, a merge's advice read as another
file's, and the comparison of two binaries would have been noise.

## Repair round 1

Blocking 1 (a new syscall off Linux): `zfile::ask_ahead` is under `#[cfg(target_os = "linux")]`
again, with the non-Linux no-op restored (`fn ask_ahead(_map: &MmapBytes, _from, _to) {}`), so
on macOS and the other unix targets the plain build asks nothing, as at the base (memmap2's
`advise_range` is `cfg(unix)`, and `MmapBytes::advise_willneed` is not Linux-gated: the
reviewer was right that round 0 made a call there the base never made). The door's
`cfgout src/db/zfile.rs <file>` is back at 2 with the base's reason ("drop_behind and ask_ahead
do nothing"): no raise, the line as at f9e83900 (`git diff f9e83900 -- tests/data/door.txt`
shows no cfgout line). Held: mutant R4 (the no-op deleted) is red in tests/source.rs by the
cfgout count. On Linux the function is the same text, so the round-0 strace stands for it
(not re-traced: no Linux code changed). Not built for a non-Linux target here (as round 0).

Blocking 2 (the warm's ask held by nothing): `a_warm_under_a_cipher_asks_for_the_envelope_and_
decrypts_nothing` now reads, after the reopen's warm, the form file's envelope (`x-*.z`) by
`MmapBytes::resident_share` over a mapping of its own (mincore), polled to 20 s, and requires
>= 0.9 of it in the cache. Red input: mutant R1 (the `map.advise_willneed(off, span)` deleted)
-> "0.081 of it in the cache" (mut-R1.log); green with the line. Rule 4: this assertion is
new this round and was shown red only against the mutant, not against the base (at the base
the warm measured the file as empty and asked nothing: the test was red there already, on its
earlier assertion, before.log).

Reviewers' mutants (all run but R5, by their own note): R2 and R9a/R9b needed tests made
stronger -- the ordindex InMemory source answers by range now (a cold range: a cold header and
a cold last head), and the pager test evicts the envelope and the plain file whole and asks
their first quarter back, then holds ranges in the quarter at 1 and past it at 0 for both kinds
(a partial drop did not work: the cache holds a file in folios the size of its writes, and a
drop of part of one leaves it; the envelope's last chunk ends in a page the file does not
fill). R8a survived (pages_resident answering 0 was read by nobody who could tell): the rewarm
test now reads every form file back whole, makes a quiet stretch and a scan, and requires no
rewarm; R8a is red there ("rewarms 2, rewarm_files 14"). R3 (ask_file by `File::open`) is
equivalent on every reachable state (mutants.md says why) and is held by the door's rule
alone. The wall: the heads test first named `MmapBytes::paged` in its own body and raised
`src/ordindex.rs paged test` 2 -> 3; the cold range went into `in_memory`'s signature instead,
so the count is the base's (tests/source.rs green, after-r1-source.log).

New tests run five times each alone, green every time (r1-repeat.log).

Round 1's checks: after-r1.log (`cargo test --profile checked --no-fail-fast`: 1527 passed and
6 failed, all six in tests/source.rs from the wall's count that the heads test raised. The
fix moved the cold range into `in_memory`. after-r1-source.log then has source 75/75 and the
lib's ordindex:: and pager:: 37/37, EXIT 0). lint-lists-r1.log: `sh scripts/check.sh lists
lint` then `wasm`, both "all checks passed", door ok, EXIT 0. cost2.log:
`COST_DIR=$B/cost2 COST_BASE_TARGET=$B/costbase-target sh scripts/cost.sh f9e83900 read read_z
scan scan_long scan_z scan_long_z merge land` ends `cost: green`, EXIT 0. The toggled probes
moved 0 functions, and land moved 3, inside their bounds. merge (no toggle) moved 36 for
+9.8 an op. They are libc's memcpy, and `Reader::sweep_retired_states` and
`Readers::oldest_for` within their `~merge` bounds (cost-hot.txt: the epoch sweep's retries,
which depend on timing). No function this unit touched is among them. Base target removed
after. Private TMPDIRs removed. Nothing runs in the background.

## Repair round 2

No blocking items; nine extra mutants (mutants.md, "Repair round 2"). Seven killed, one
equivalent (E7, read there), one held by a script (E5). What was added for them, all tests:

- E1 (`drop_behind` given an end for a length): `a_drop_behind_unmaps_the_window_and_no_page_past_it`
  (src/db/zfile.rs tests, Linux and not qemu-user): a file of 64 pages, every page touched
  through the mapping, a drop from mid-page 8 to mid-page 24, then the mapping's page table read
  (`/proc/self/pagemap`'s present bit; `mincore` cannot see a mapping): 9..24 gone, every other
  page mapped. The door exempts a test's /proc literal (tests/source.rs, "a test's /proc
  literal"), so no list moves. Red only against the mutant (the line was right at the base).
- E5 (the alignment in `ask_ahead`) and E1 again: the round-0 strace comparison was a script
  that printed and exited 0. `strace/gate.sh BIN OUT` runs the build-drop test of a zpages binary
  under strace and `gate.py` holds its shapes to the three base runs, exit 1 on a NEW or GONE
  shape. Control C0 (the tree's own binary): green, "VERDICT same". E1 and E5: red. The gate
  covers the one test whose shapes are the same in all three base runs (67 of 67); the other six
  of round 0's comparison stay as compare.txt read them.
- E2 (`records_resident` asking the directory): `builds_ahead_at_landing_read_the_records_
  residency_and_not_the_directorys` (tests/zpages.rs): each partition's mappings zapped and the
  file evicted, then its record region read back with readahead off (`regions_of` for the place,
  `cached_pages` asserting records >= 0.9 and directory < 0.5 in the cache), then the budget set
  under the selector's line: every partition built ahead, none found cold. Under the mutant the
  look read the directory and marked the partition cold. Plain kind only: the line is shared by
  both kinds, the paged arm's answer by range is the pager test's (R9a/R9b); a cipher store with
  records and directory in different residency states is not built (its records' chunks would
  have to be out of the store's cache and the kernel's while the directory's are in one of them).
- E6 (the warm skipping a chunk half resident): a phase of
  `a_warm_reads_the_cold_file_once_and_skips_a_resident_one`: three quarters of the file read back
  with FADV_RANDOM, the warm then reads the cold quarter and counts it, and the file is whole
  after.
- E3, E4, E8, E9 were killed by the tests the reviewers named; E4 by each of four tests run on its
  own, the block form's `Mapped` walk and `write_stride` both.
- E7 (`.unwrap_or(1.0)`): survived; no reachable state answers None (mutants.md says which lines
  make that so). The default is the base's.

Checks: r2-newtests.log (the three new or changed tests green, before the mutants were rerun);
r2-checks.log (`sh scripts/check.sh lists lint`, then `cargo test --profile checked
--no-fail-fast`). `cargo fmt -- --check` clean. Not run this round: wasm, bench, stride and the
cost count, since nothing compiled into the library or a release build changed (both edits to
src/db/zfile.rs are inside `mod tests`).

Not verified this round: E2's mutant under a cipher (above); the gate's other tests.
- r2-checks.log: lists + lint "all checks passed: lists lint"; then the full checked test run,
  1535 passed, 0 failed, EXIT 0 (started 19:2xZ). Private TMPDIRs removed; nothing runs in the
  background; the tree equals keep/ byte for byte.
