# G1-4 mutants

Runner: `/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/mutants.py` (each applied to the keep copies, `cargo test --profile checked --test blob the_advice_by_rank`, verdict appended to results.txt, files put back by sha256 after each and at the end; tree checked against keep/SHA256SUMS after). Log: mutants.log. All 18 killed, none survived.

| mutant | verdict |
|---|---|
| M1 ranks by piece answers 0 | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (26088) panicked at tests/blob.rs:529:17: |
| M2 dir by piece answers 0 | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (26320) panicked at tests/blob.rs:603:14: |
| M3 record_offset by piece answers None | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (26555) panicked at tests/blob.rs:521:17: / assertion `left == right` failed: plain, pieces of 24: record offset at 0 |
| M4 ranks offset without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (26802) panicked at tests/blob.rs:593:13: |
| M5 record offset without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (27039) panicked at tests/blob.rs:570:9: |
| M6 dir offset without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (27274) panicked at tests/blob.rs:613:9: / assertion `left == right` failed: plain, pieces of 24: the word at 40280 names  |
| M7a record offset without the region start | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (27508) panicked at tests/blob.rs:570:9: |
| M7b ranks offset at the directory start | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (27743) panicked at tests/blob.rs:593:13: |
| M8a piece arm only: rank + 1 | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (27976) panicked at tests/blob.rs:621:5: / assertion `left == right` failed: plain, pieces of 24: the ask for rank 0 |
| M8b M8a with the equality assertion dropped | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (28210) panicked at tests/blob.rs:621:5: / assertion `left == right` failed: plain, pieces of 24: the ask for rank 0 |
| M8c piece arm only: record offset of rank + 1, equality dropped | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (28443) panicked at tests/blob.rs:521:17: / assertion `left == right` failed: plain, pieces of 24: record offset at 599 |
| O1 region place without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (28683) panicked at tests/blob.rs:577:5: / assertion `left == right` failed: plain, pieces of 24: the region's place is it |
| O2 dir ask without the word past the last | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (28916) panicked at tests/blob.rs:606:22: |
| O3 ranks span not cut at the region end | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (29151) panicked at tests/blob.rs:593:13: |
| O4 whole arm sent to the piece arm | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (29386) panicked at tests/blob.rs:574:9: / assertion `left == right` failed: plain, pieces of 24: mapped at 0 |
| O5 region place length short by one | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (29622) panicked at tests/blob.rs:577:5: / assertion `left == right` failed: plain, pieces of 24: the region's place is it |
| O6 region place by the piece only (the old shape) | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (29870) panicked at tests/blob.rs:542:13: / assertion `left == right` failed: plain, pieces of 24: the piece reader agains |

Plan's list: piece arm 0/None = M1-M3; offset without index_offset = M4-M6; region start off = M7a (start dropped), M7b (the directory's start); the equality assertion dropped = M8b, M8c (each with a piece-arm-only code mutant, killed by the file checks the piece trace is now held to as well, so the equality is not the only check of the piece arm). Own: O1-O6 on the lines written (region place's offset, its length, the old piece-only shape; the directory ask's extra word; the span's cut at the region end; the whole arm routed through the piece arm).


## Repair round 1

Runner `mutants_r1.py` (round 0's mutants rewritten for round 1's text, and the reviewers' extra
mutants as X1-X8, and O7-O9 on the lines this round wrote), then `mutants_r1b.py` after the test
gained the copying reader, the resident range, the asks of no bytes and the ask with no room
(`(keys/2, 4, 0)`) and the directory's dead guard was taken out: M2 M6 O2 X1 X4a X5a X7 X8 and
Y2-Y4 again or new. Same test, the same put-back by sum (keep/SHA256SUMS). Logs mutants-r1.log,
mutants-r1b.log; verdicts results-r1.txt, results-r1b.txt.

### First pass (results-r1.txt)

| mutant | verdict |
|---|---|
| M1 ranks by piece answers 0 | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (3157) panicked at tests/blob.rs:541:17: |
| M2 dir by piece answers 0 | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (3392) panicked at tests/blob.rs:629:14: |
| M3 record_offset by piece answers None | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (3631) panicked at tests/blob.rs:533:17: / assertion `left == right` fai |
| M4 ranks offset without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (3893) panicked at tests/blob.rs:606:13: |
| M5 record offset without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (4148) panicked at tests/blob.rs:582:9: |
| M6 dir offset without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (4384) panicked at tests/blob.rs:639:9: / assertion `left == right` fail |
| M7a record offset without the region start | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (4615) panicked at tests/blob.rs:582:9: |
| M7b ranks offset at the directory start | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (4847) panicked at tests/blob.rs:606:13: |
| M8a piece arm only: rank + 1 | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (5080) panicked at tests/blob.rs:647:5: / assertion `left == right` fail |
| M8b M8a with the equality assertion dropped | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (5312) panicked at tests/blob.rs:647:5: / assertion `left == right` fail |
| M8c piece arm only: record offset of rank + 1, equality dropped | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (5546) panicked at tests/blob.rs:533:17: / assertion `left == right` fai |
| O1 region place without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (5786) panicked at tests/blob.rs:589:5: / assertion `left == right` fail |
| O2 dir ask without the word past the last | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (6018) panicked at tests/blob.rs:632:22: |
| O3 ranks span not cut at the region end | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (6249) panicked at tests/blob.rs:606:13: |
| O4 whole arm sent to the piece arm | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (6479) panicked at tests/blob.rs:586:9: / assertion `left == right` fail |
| O5 region place length short by one | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (6711) panicked at tests/blob.rs:589:5: / assertion `left == right` fail |
| O6 region place by the piece only (the old shape) | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (6956) panicked at tests/blob.rs:554:13: / assertion `left == right` fai |
| O7 Pieces::start_in off by one | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (7199) panicked at tests/blob.rs:586:9: / assertion `left == right` fail |
| O8 slice start_in answers 0 | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (7431) panicked at tests/blob.rs:586:9: / assertion `left == right` fail |
| O9 flat_lent answers None for a whole reader too | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (7664) panicked at tests/blob.rs:586:9: / assertion `left == right` fail |
| X1 dir ask placed at the record region start, both arms | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (7892) panicked at tests/blob.rs:639:9: / assertion `left == right` fail |
| X2 region place through flat() alone | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (8133) panicked at tests/blob.rs:539:13: |
| X3a piece arm: ranks span cut at the section end | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (8376) panicked at tests/blob.rs:606:13: |
| X3b piece arm: dir span cut at the section end | SURVIVED |
| X4a dir: residency skip deleted | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (8848) panicked at tests/blob.rs:674:5: / assertion `left == right` fail |
| X4b ranks: residency skip deleted | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (9080) panicked at tests/blob.rs:663:5: / assertion `left == right` fail |
| X5a ranks: a >= b to a > b | SURVIVED |
| X5b dir: a >= b to a > b | SURVIVED |
| X6 piece arm: record offset by the directory start | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (9784) panicked at tests/blob.rs:582:9: |
| X7 ranks by piece not cold, inlinable | SURVIVED |
| X8 region place by the piece answered by Pieces::offset | SURVIVED |

### Second pass, against the final test and code (results-r1b.txt)

| mutant | verdict |
|---|---|
| M2 dir by piece answers 0 | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (11002) panicked at tests/blob.rs:645:14: |
| M6 dir offset without index_offset | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (11246) panicked at tests/blob.rs:655:9: / assertion `left == right` fai |
| O2 dir ask without the word past the last | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (11479) panicked at tests/blob.rs:635:13: |
| X1 dir ask placed at the record region start, both arms | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (11735) panicked at tests/blob.rs:655:9: / assertion `left == right` fai |
| X4a dir: residency skip deleted | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (11979) panicked at tests/blob.rs:690:5: / assertion `left == right` fai |
| X5a ranks: a >= b to a > b | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (12222) panicked at tests/blob.rs:621:13: |
| X7 ranks by piece not cold, inlinable | SURVIVED |
| X8 region place by the piece answered by Pieces::offset | SURVIVED |
| Y2 Sec::lend answers None for a copied section | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (12956) panicked at tests/blob.rs:595:30: |
| Y3 dir ask from the next rank, both arms | KILLED thread 'the_advice_by_rank_answers_the_same_for_a_source_by_the_piece' (13187) panicked at tests/blob.rs:655:9: / assertion `left == right` fai |
| Y4 flat_lent through flat (the error made) -- equivalent in answers | SURVIVED |

### The survivors, each read

- X3b, X5b (first pass): the lines they mutated were a check nothing reaches -- a directory
  span from `dir_span` is never empty and never past an index's directory -- and were taken out
  (notes.md, "The directory ask's guard taken out"). Not mutants of the tree any more.
- X5a (first pass): the ask with no room (`cap` 0) was in no call; the trace makes it now, and
  every ask is required to ask for more than nothing. Killed in the second pass.
- X7 (`#[cold] #[inline(never)]` dropped from `willneed_ranks_by_piece`) and Y4 (`flat_lent`
  through `flat`): no answer changes; each is a cost, and the scratch counts tell both
  (notes.md: X7 75,689 instructions more over r1/gate4's run, Y4 the 57-against-55 a call of
  item 1). Held by a count no gate runs; the cost job's probe for them is owed.
- X8: equivalent (`Pieces::offset` of a part is `index_offset()` plus its start).

The reviewers' extra mutant "record_offset's whole arm back to the pointer subtraction" is the
tree itself now (`Sect::start_in` is the subtraction for a slice): green, nothing to run; its
companion "the piece arm placed at the directory's start" is X6, killed.


## Repair round 2

Runner `mutants_r2.py` (each mutant applied to keep/, its tests run under `--profile checked`,
the verdict to results-r2.txt as it comes, every file put back by sum after each and at the
end). Tests: `--test blob the_advice_by_rank` for the reader's mutants; `--test fences drop` and
`--test zpages a_merge_drops` for the merge's. First pass mutants-r2.log (before the two new
tests), second pass mutants-r2b.log (after them).

| mutant (the reviewers' number) | verdict | killed by |
|---|---|---|
| Z1 (1) `dir_span(rank, n)` | KILLED | tests/blob.rs:635, `dir_at`'s ask of 8 bytes |
| Z2 (2) region place `sec.start_in(recs)` | KILLED | the subtraction's overflow at src/flatindex.rs:755 (slice arm, whole reader first) |
| Z3 (3) ranks by piece with `usize::MAX` for `cap` | KILLED | "an ask of no bytes" (calls[6]), pieces of 24 |
| Z4 (4) `Sec::lend`'s Lent arm `None` | KILLED | "mapped at 0" (the copying reader's offsets) |
| Z5 (5) `compact_run`'s first drop from `record_offset(1)` | SURVIVED, then KILLED | new test `a_merges_drops_behind_begin_at_each_inputs_first_record` (tests/fences.rs:810) |
| Z6 (6) `Sec::lend` by `slice_at` | SURVIVED, then KILLED | new assertion: the piece reader's bytes asked across the trace under a quarter of the region (tests/blob.rs:554) |
| Z7 (7) `Pieces::start_in` the absolute offset | KILLED | tests/blob.rs:597, "the region's place is its span" |
| (8) `#[cold] #[inline(never)]` off `willneed_dir_by_piece`, `record_offset_by_piece` | survives the tests (no answer moves); counted | r2/count8.sh: `record_offset` +13 a call over 120,006 calls in the drop probe; `overlay_runs` 12,194 fewer over the gate probe's run (notes.md, round 2) |
| (9) `#[inline(never)]` off `record_offset` | RED by exit status | r2/red9.sh: cost.sh 869a4fee merge exit 1, `cost: RED`, `compact_run +4.003 an op`; control exit 0 `cost: green` |
| Z10 (10) the `a >= b` guard out of `willneed_ranks_in` | KILLED | tests/blob.rs:621, every ask asks for more than nothing |
| Z10s the guard out of the slice arm alone | KILLED | tests/blob.rs:621 |
| Z10p the guard out of the piece arm alone | KILLED | tests/blob.rs:621 |
| Z11 (own) a drop to the record's page, not a page behind | KILLED | the new fences test |
| Z12 (own) `dropped_to` not moved | KILLED | the new fences test |
| Z13 (own) the first drop from the file's start | KILLED | the new fences test |

Z5 and Z6 were missing tests, each added: Z6's reader answers alike either way and over a paged
mapping makes the section present whole at every call; Z5's drop is the base's mechanism, which
no test held to where it begins. The zpages test passes under Z5, Z11-Z13 (it holds only that
some bytes were dropped).
