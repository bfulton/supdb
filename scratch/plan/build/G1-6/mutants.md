# G1-6 mutants

Runner: `mutants.py` (each mutant one text replacement in one file, its tests run, the verdict
written to `mutants-results.txt` as it comes, the file put back from `keep/` and checked by its
sum after each). Logs: `mut-<key>.log`. Every kill below was read in its log to be the
assertion it names, not a compile failure.

The plan's four (g1.md, G1-6 "Mutants"), then nine of my own on the lines I wrote.

| key | mutant | killed by | how |
|---|---|---|---|
| M1 | the paged arm of `MmapBytes::resident_share` answers `Some(0.0)` | lib `a_paged_mapping_and_a_mapped_file_say_alike_what_is_resident` / `the_heads_resident_are_asked_of_the_source_for_either_kind` | "a source that holds all of it" |
| M2 | `warm_file` touches a paged mapping (`slice_at` for `lend_at`) | zpages `a_warm_under_a_cipher_asks_for_the_envelope_and_decrypts_nothing` | the debug assertion `Paged::ensure` in the warm thread: "read with nothing pinned" (it would decrypt into the cache; the test's fill count is behind it) |
| M3 | `z_cross` takes the paged branch for a mapped file | zpages `builds_ahead_at_landing_start_past_the_percent_and_only_from_resident_records` | "crossing, drop past true, near false: past: records 676/676": the records not dropped |
| M4 | the record region's place without `index_offset()` (what `swap_ahead`, the merge's ahead, the drop and the build are placed by) | blob `the_advice_by_rank_answers_the_same_for_a_source_by_the_piece` | "the region starts at rank 0", Some(192) against the record's offset |
| O1 | `heads_all_resident` true on any answer (`.is_some()`) | lib `the_heads_resident_are_asked_of_the_source_for_either_kind` | "one that holds most of it" (the InMemory source given a share for this) |
| O2 | the warm skips a chunk with any page resident (`share > 0.0`) | zpages `an_attached_form_file_is_warmed_once_per_process_unless_the_warm_is_off` | warm_bytes 0 where the attach left pages cold |
| O3 | the warm counts a chunk whole whatever was resident | same test | warm_bytes the whole file |
| O4 | the mapped arm of `resident_share` ignores its offset | lib `residency_counts_every_page_a_range_touches_and_none_past_the_end` | "past the mapping": Some(1.0) |
| O5 | `MergeAhead::new` takes a whole reader only (the base's shape) | cipher `a_merge_under_a_cipher_asks_ahead_for_its_cold_inputs` | "cold inputs: 0 asks of 0 bytes" -- this is the base's behaviour at that site, so it is the test's red-before as well |
| O6 | the build's drop behind without its mapping's pages (`advise_dontneed_range` taken out of `drop_behind`) | zpages `the_build_drops_the_raw_records_it_passed_when_asked_to_and_not_otherwise` | "the build left 235 of 235 pages of the records in the cache" |
| O7 | `Seg::records_by_descriptor` answers for a reader by the piece | zpages `form_files_an_earlier_open_left_under_a_cipher_are_dormant_and_then_attached` | "the form files on disk were not found": the crossing under a cipher swapped nothing (vault::open refuses the envelope and the site skipped) |
| O8 | `Blob::record_dir_place` places the record region | lib `a_stride_walk_that_declines_alone_gives_back_what_it_read` | the seen drops against the pointer oracle: `[(4288, 120504), (4288, 12288)]` |
| O9 | `Blob::raw_read_span` answers none for a reader by the piece (the base's shape) | blob `the_advice_by_rank_answers_the_same_for_a_source_by_the_piece` | "a raw read's span" |

O9 survived its first run (the zpages cipher tests do not reach the hot-raw policy's ask under
a cipher): the test was missing. `holds_to_the_file` in tests/blob.rs now holds the span a
raw read touches to the mapped reader's and to the record region and the file, and O9 is
killed by it (run again alone, `mutants-run2.log`).

Not mutated, and why:
- `ask_ahead`'s `from & !(ZPAGE - 1)`: taking it out changes no answer a test can read (the
  kernel reads the same pages), only the syscall's arguments (memmap2 would then align the
  address itself and lengthen the first piece by the remainder). It is held by the strace
  comparison alone: every build ask in it starts on a page (`4096 131072 MADV_WILLNEED` ...),
  the base's shapes exactly. Said in the notes as what the strace holds and no test does.
- `warm_file`'s `.round()`/`.min(pages)` on a paged share: for a mapped file the share is a
  ratio of integers and round-trips exactly; for a paged one the bytes are an estimate the
  tests hold only to be positive and under the file.

## Repair round 1

Runner: `mutants_r1.py` (the same runner, the reviewers' list), verdicts in
`mutants-results-r1.txt`, logs `mut-<key>.log` (overwrite the round-0 log of no key: the keys
are new). Each kill read in its log to be the assertion named. Tree checked against keep/ by
sum after the run.

| key | mutant | killed by | how |
|---|---|---|---|
| R1 | `warm_file` asks nothing (`map.advise_willneed(off, span)` deleted) | zpages `a_warm_under_a_cipher_asks_for_the_envelope_and_decrypts_nothing` (assertion added this round) | "the warm asked for the envelope: 0.081 of it in the cache" -- the envelope's own residency, by mincore over a mapping of the `x-*.z` file, polled to 20 s. This is the blocking item's red: with the line in, the share reaches >= 0.9 |
| R2 | `heads_all_resident` asks from offset 0 (`HEADER` dropped) | lib `the_heads_resident_are_asked_of_the_source_for_either_kind` (cases added this round) | "a source whose header alone is out": the InMemory source now answers by range (cold range given), a cold header and a cold last head |
| R3 | `ask_file` opens with `std::fs::File::open` past the door | SURVIVED (equivalent on every reachable state; see below) | |
| R4 | the non-Linux `ask_ahead` no-op deleted | source tests (14 of them through the door's table) | "cfgout src/db/zfile.rs <file>: 1 < 2 listed: lower tests/data/door.txt": the door list holds the repaired count |
| R5 | `resident_share`'s mapped arm `off as usize` for `usize::try_from(off).ok()?` | not run | differs only for an offset past `usize::MAX`, which a 64-bit target cannot have and a 32-bit (wasm) build never reaches with a mapping; recorded rather than run, as the reviewer asked |
| R6 | `Seg::records_resident` answers a whole reader only (`record_regions()` first) | zpages `builds_ahead_under_a_cipher_seal_their_form_files_and_keep_them_cold` | "built ahead": `ahead_cold 7`, `form_files 0` -- under a cipher the census read 0 and no build ahead was made |
| R7 | `OrdIndex::resident_share` answers 0 | zpages `a_served_partitions_ordered_index_is_dropped_again_a_second_on` | "what the cache held went: 0 -> 0" |
| R8a | `ZFile::pages_resident` answers 0 | SURVIVED its first run; killed by zpages `scans_after_a_quiet_stretch_have_the_cold_form_files_asked_for` (phase added this round) | "no rewarm over files in the cache": rewarms 2, rewarm_files 14 -- every file read back whole, a quiet stretch and a scan, and the rewarm asked for them all |
| R8b | `ZFile::pages_resident` answers 1 | zpages `a_rewarm_is_made_beside_a_merge_in_flight_after_a_quarter_second`, `scans_after_a_quiet_stretch_...` | no rewarm over files evicted |
| R9a | the paged arm of `MmapBytes::resident_share` ignores its offset (`p.resident(0, len)`) | lib `a_paged_mapping_and_a_mapped_file_say_alike_what_is_resident` (and the heads test) | the paged mapping and the mapped file each evicted whole and their first quarter asked back (added this round): a range past the quarter answers 1.0 |
| R9b | the paged arm swaps offset and length | same | same |
| E10 | = R2 (listed twice by the reviewers) | | |

R3, read: `ask_file`'s two callers (the dropped-file asks at a conversion's and a build's
collect, `src/db.rs`) run once the build's outcome is `Built`, and the build has persisted the
form file's envelope before it returns (`zfile.rs`, `vault::persist_form` in the build). For
a plain store `vault::open` is one load of a counter and `File::open`: the same openat (the
strace). Under a cipher, with the envelope written, `vault::open` refuses a form file and
`File::open` fails at the decoy directory: both answer 0. The memfd case (a form file kept in
memory with no envelope yet) is not reachable at either caller. So the mutant is equivalent on
every reachable state, and the change is held by nothing but the door's rule (S1: every file
operation through vault): the function's own `#[allow(clippy::disallowed_methods)]` (for its
`posix_fadvise`) covers a `File::open` too, so the door's lint cannot see it either. A
`posix_fadvise` on a door handle is S1's typed handle; what a cipher store should ask here --
the envelope, through the pager -- is S5's, and is not asked today.

Also run this round (for the repair's own lines): the lint and lists groups with the door's
`cfgout src/db/zfile.rs <file>` back at 2 (lint-lists-r1.log).

## Repair round 2

Runner: `mutants_r2.py` (every command of a mutant run, not stopping at the first red, so a
mutant is shown red at each test it is listed against; verdicts with each command's status in
`mutants-results-r2.txt`; logs `mut-<key>.log`; the file put back from `keep/` by its sum after
each). The strace gate is `strace/gate.sh BIN OUT` (the build-drop test of the zpages binary under
strace, normalized, held by `strace/gate.py` to the three base runs' shapes, exit 1 on any shape
no base run made or any shape every base run made and this run did not). C0, the control, runs
the gate over the tree's own binary: green (rc 0). Each kill read in its log.

| key | mutant | killed by | how |
|---|---|---|---|
| C0 | none (the gate's control) | -- | gate green over the tree's binary: VERDICT same |
| E1 | `drop_behind`: `advise_dontneed_range(a as u64, b)` (an end as a length) | lib `a_drop_behind_unmaps_the_window_and_no_page_past_it` (added this round) and the strace gate | the test: pages 24..32 unmapped past a window of 9..24 (`/proc/self/pagemap`'s present bit over the mapping); the gate: `8192 962560 MADV_DONTNEED` and `974848 159744` NEW, the base's `8192 954368`, `974848 28672` GONE |
| E2 | `Seg::records_resident` asks `record_dir_place()` | zpages `builds_ahead_at_landing_read_the_records_residency_and_not_the_directorys` (added this round) | "built ahead from the resident records": `ahead_cold 1, prebuilt 0` -- the records warm and the directory cold, the look read the directory. SURVIVED its first run: both listed tests (the plain census and the cipher one) have records and directory in one residency state. The plain kind only: the line is one path for both kinds, and the paged arm's answer by range is held by the pager test (R9a/R9b) |
| E3 | `warm_file`: `let len = map.as_slice().len()` | zpages `a_warm_under_a_cipher_asks_for_the_envelope_and_decrypts_nothing` | its first assertion (tests/zpages.rs, "the warm measured the file"): `warm_bytes + warm_skipped` 0, before the envelope's residency is read |
| E4 | `build_file_seen`: `recs_at` and `dir_at` swapped | lib `a_form_file_is_the_same_bytes_whatever_the_piece_and_the_window` (block form, the `Mapped` walk: "the directory is dropped last", `(4288, 28288)` against `(755792, 779792)`), lib `a_stride_walk_that_declines_alone_gives_back_what_it_read` and `a_stride_file_is_the_same_bytes_whatever_the_piece_and_the_window` (`write_stride`), zpages `the_build_drops_the_raw_records_it_passed_when_asked_to_and_not_otherwise` ("235 of 235 pages of the records in the cache") | all four red, each run on its own |
| E5 | `ask_ahead` (Linux) passes `from` unaligned | the strace gate only (every lib zfile test green, rc 0) | `4096 131264 MADV_WILLNEED` ... NEW (memmap2 aligning each piece's start and lengthening it by the remainder), the base's `131072` pieces GONE. No test can see it: the kernel reads the same pages either way. Held by `strace/gate.sh`, a script with an exit status, whose control C0 is green |
| E6 | `warm_file`: `share >= 0.5` skips a chunk | lib `a_warm_reads_the_cold_file_once_and_skips_a_resident_one` (phase added this round) | "a chunk 42 of 57 pages resident: Warm { bytes: 0, skipped: 233472 }" against `(61440, 0)`. SURVIVED its first run: every warm in the tests met a chunk wholly cold or wholly resident; the phase reads three quarters of the file back (FADV_RANDOM, no readahead) and warms |
| E7 | `Seg::records_resident`: `.unwrap_or(1.0)` | SURVIVED (equivalent on every reachable state) | see below |
| E8 | `Seg::records_by_descriptor` without `raw.record_regions()?` | zpages `form_files_an_earlier_open_left_under_a_cipher_are_dormant_and_then_attached` | "the form files on disk were not found", as O7 |
| E9 | `check_stride`: `read_back_residency(..).is_some_and(\|s\| s < 0.5)` | lib `a_stride_build_is_trusted_only_where_its_read_back_read_the_device` | "drop_output true, a page held true": `stride_device` true where one page of the file is held mapped (the share small and not none) |

E7, read: the `unwrap_or` answers only where `record_region_place` or `resident_share` is `None`.
`record_region_place` is `None` through `indexed!`'s third arm, a reader with neither `flat()` nor
`pieces()`, which is `Index::Sparse`, made only by `SparseBlob` (src/blob.rs), never for an
engine segment, which `Blob::open_with` opens with a flat index or refuses; or where `regions`
cannot cut the header's ranges from the section, which `FlatIndex::parse`'s header checks keep
from an open. `MmapBytes::resident_share` is `None` only for a mapped file whose `mincore` fails
(`ENOMEM` for a range not mapped, which a range inside the mapping is not; `EAGAIN` under kernel
memory pressure) or an offset past `usize`; the paged arm always answers. So no reachable state
reaches the default, and a test cannot hand one to `Seg` (a private type of a live store). The
default is the base's (`unwrap_or(0.0)` in the base's paged arm): a source that cannot say reads
as not resident, the side that builds nothing ahead from the device. Kept; not a check, a total
answer.
