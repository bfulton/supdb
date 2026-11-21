# G1-5 mutants

Runner: mutants.py (over keep/, put back by sum after each; verdicts in
results.txt, logs mutants.log and mutants2.log -- O4's first try did not apply,
its edit text predated rustfmt, and was rerun with O5). Tests: tests/blob.rs
whole for every mutant but O1, which runs tests/cipher.rs
`a_store_larger_than_its_cache_reads_as_itself_within_it` (the guard).

| mutant | killed by |
|---|---|
| P1 `Bytes::authenticated` true by default (plan) | new test: `!Piecewise::new(..).authenticated()` (line 446); the flip would too |
| P2 the skip on `piecewise` alone (plan) | new test: "a flip ... opened over a source by the piece that checks nothing" |
| P3 the verify through `Pieces` asking for the section whole (plan) | new test: longest ask 240072 bytes > a piece |
| P4 `MmapBytes` answering true for a mapped file (plan) | new test: `!MmapBytes::open(..).authenticated()` |
| O1 `MmapBytes` answering false for a paged mapping | the guard: tests/cipher.rs open bound (cache peak 21 MB of a 2 MB budget: the open verifies every key section whole) |
| O2 a piece mismatch passed over (`continue` for `return Err(i)`) | new test: the flip opens |
| O3 the verdict dropped in `verify_row_by_piece` | new test: the flip opens |
| O4 the arm by the piece in `open_with` verifies nothing | new test: the flip opens |
| O5 the first piece of the row never asked (`.skip(1)`) | new test: asked 228056 of 240132 bytes (the flips all land past piece 0, so the ask count is what holds the first piece) |

No survivor. The tree was put back exactly (sha256 of the four files against keep/SHA256SUMS: OK).

## Repair rounds 1 and 2

Runners: mutants3.py (X1, X2, over keep2/, results2.txt), mutants4.py (X3-X12,
E1-E10, over keep2/, results3.txt, log mutants4.log, EXIT 0), mutants5.py (E8
and X7 again over keep3/, results4.txt, full outputs out-E8.log and
out-X7.log, log mutants5.log, EXIT 0). Each runs tests/blob.rs whole and puts
the four files back by sum after each mutant. The tree after the last run
matches keep3/SHA256SUMS. "the new test" is
`a_source_by_the_piece_that_checks_nothing_has_its_row_verified_at_open`;
"the forged test" is
`a_piece_whose_crc_moves_in_its_top_byte_alone_is_refused_by_the_piece`
(round 2, for E8).

| mutant | killed by |
|---|---|
| X1 `Advised::authenticated` false (test wrapper) | new test: the flip opened through the wrapper of a checked source, line 460 |
| X2 each piece held to the next word (`.skip(1)` on the row) | five tests: every piece-source open refused |
| X3 / extra 6: the skip on `src.piecewise()` in `verify_row_by_piece` | new test: flip at 18471 opened over the unchecked piece source (assert_eq, line 498) |
| X4 the last piece dropped (`take(n/4-1)` on the pieces) | new test: asked-bytes bound, 225552 of 240132 (no flip lands in the last piece; the bound is what holds it) |
| E7 / extra 7: the last word never compared (`chunks_exact(4).take(len/4-1)`) | the same bound, 225552 of 240132 |
| X5 a piece not lent passed over (`continue`) | new test: "a piece of the records not lent opened" (refusing source) |
| X6 a row not lent passes (`return Ok(())`) | new test: "the row not lent opened" (refusing source) |
| E10 / extra 10: a row not lent read as no words (`unwrap_or(&[])`) | new test: "the row not lent opened" |
| X7 / extra 9: base 0 at the open | six tests, every one by "key index checksum mismatch in piece 0" at the open (out-X7.log): the red is the row's at open, not another failure |
| E3 base `sb.key_off + 1` | five tests: every piece-source open refused at the row (clean `expect` fails) |
| X8 the piece arm reached with `verify_index` off | new test: "was checked with the row's verification off" |
| X9 own: the row range cut short (`saturating_sub`) | five tests: the row not lent, every piece-source open refused |
| X10 / E2: a piece not lent named `usize::MAX` | new test: "not lent: ... piece 18446744073709551615, not piece 7" |
| X11 own: a mismatch named piece 0 | new test: assert_eq on the refusal's words (flip at 18471 named another piece) |
| X12 own: the word compared big-endian | five tests: every piece-source open refused |
| E1 the row range empty (zip verifies nothing) | new test: the flip at 18471 opened |
| E4 `authenticated() || piecewise()` at the callee | new test: the flip at 18471 opened |
| E5 `Advised::piecewise` false, `authenticated` forwarded | new test (line 511: the flip was checked through the wrapper, the whole arm having verified it) and `the_advice_by_rank_answers_the_same_for_a_source_by_the_piece` (`piecewise()` assert): the Advised line does run the piece arm |
| E8 / extra 8: three bytes of the word compared | SURVIVED round 2's first run (results3.txt): a one-byte flip moves all four CRC bytes but one time in 2^24, so no fixed flip set can tell. Test added: the forged test (a set of bits in one piece, found by elimination over the CRC's linear deltas, whose CRC moves in the top byte alone; the test asserts that of its own input). Killed by it at line 684 (results4.txt, out-E8.log) |

No survivor after round 2.

## Repair round 3

Runner mutants7.py over keep4/ (src identical to keep3/; tests/blob.rs with
round 3's additions), results5.txt, logs mutants7-old.log, mutants7-new.log,
mutants7-r3.log (each EXIT 0; the four files checked against
keep4/SHA256SUMS after the last). "old" runs the mutant with keep3's
tests/blob.rs (the test as round 2 left it), "new" with round 3's. The
reviewers' blocking item: no test reached the compare of the first piece
(O5's note) or the last (X4's). Round 3 adds to the new test a flip at the
last byte of every piece (15 in the fixture: piece 0 is short, 0..12288 of
the section, its last byte past the 192-byte header; piece 14 ends at the
row), each asserted refused by the whole reader *for that piece* and
refused the same by the unchecked piece reader; and an empty mapped file's
`authenticated` asserted false (for R3).

| mutant | old test | new test |
|---|---|---|
| R1 (extra 1, = X2) each piece held to the next word | -- | killed, five tests (every piece-source open refused), as X2's record says |
| R2 (extra 2) skip the row when `crc_off > sec.size()/2` | -- | killed, the new test ("flip at section offset 18471 opened") and the forged test: the row always lies in the second half |
| R3 (extra 3) `MmapBytes::authenticated` true at length zero | -- | SURVIVED the first run (no test opens an empty mapped file; `Blob::open` refuses one at the superblock, so no reader reaches it); killed after the assertion `!MmapBytes::open(empty).authenticated()` was added (line 460). Nothing else depended on it |
| R4 (extra 4) piece 0 asked, never compared | SURVIVED (red input) | killed: "a flip at the end of piece 0 of 15 opened" |
| R5 (extra 5) the last piece asked, never compared | SURVIVED (red input) | killed: "a flip at the end of piece 14 of 15 opened" |
| R6 (extra 6) every other piece compared | killed (both parities already among the 12 flips and the forged piece) | killed |
| R7 (extra 7) base aligned to the piece | -- | killed by refusals, not the ask bound: the forged piece and the flip at 18471 opened, and every clean piece-source open refused "checksum mismatch in piece 0" (out5-new-R7.log) |
| R8 (extra 8) both first entries dropped | -- | killed: five tests, every piece-source open refused (each piece held to the next word, as R1) |
| R9 own: the piece ending at the row never compared | SURVIVED | killed (end of piece 14) |
| R10 own: a piece starting at the section start never compared | SURVIVED | killed (end of piece 0) |
| R11 own: a short piece never compared | SURVIVED | killed (piece 0 is the short one) |
| R12 own: the middle piece (count/2+1) skipped | killed | killed |

No survivor after round 3.
