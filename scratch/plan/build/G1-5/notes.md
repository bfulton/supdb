# G1-5 `authenticated` -- notes

Base (HEAD at the unit's start): ea0962c2 (the launch-args commit on top of
97412a31 G1-4). `git status --porcelain` was empty at the start.

## Step: (kept current)

Repair round 3 (15:2xZ on): the reviewers' blocking item (no test reached
the compare of the first or the last piece). tests/blob.rs only: a flip at
the last byte of every piece in the new test. The finished copies are the
working tree's and keep4/ (SHA256SUMS; src identical to keep3/). Runner
mutants7.py: "old" (keep3's test) then "new" (keep4's), results5.txt,
logs mutants7-old.log and mutants7-new.log, read by their EXIT lines.
Done: R1-R12 (mutants.md "Repair round 3"): R4, R5, R9, R10, R11 survive
the old test (the red input) and are killed by the new; R3 survived and is
killed by an assertion added for it (an empty mapped file answers `authenticated`
 false). Neighbours (blob, cipher, segwriter, source) in
neighbours3.log, lint and lists in lint4.log. src/ unchanged since round 2
(keep4's src files equal keep3's by cmp), so no callgrind row is counted
again: cost2.log stands for the src as it is.

(Round 2's step, for the record:)

Repair round 2 (14:25Z on): the finished copies are the working tree's and
keep3/ (SHA256SUMS checked after every runner). Done: the extra mutants
(mutants4.py, mutants5.py; mutants.md), one survivor (E8) answered with a
test (tests/blob.rs
`a_piece_whose_crc_moves_in_its_top_byte_alone_is_refused_by_the_piece`), a
clippy fix in it (`sort_by_key`), the full test run (after2.log), the
neighbours on the final copy (neighbours2.log), lint lists (lint3.log),
wasm (wasm2.log). The bench group (bench.log, EXIT 0, after removing
bench/target/release/examples, 1.2 GB) and the count (cost2.log, `cost:
green`, EXIT 0, base target removed by the script) are done. Round 2 changed
tests/blob.rs only; nothing under src/. The private TMPDIR is removed. The
unit is ready for the commit step.

## What the tree has that the plan read at 682b2c09

- The skip in `Blob::open_with` is now spelled `key.pieces(&src).is_none()`
  (the plan's blob.rs:975-980), with a comment "A section held by the piece
  is not asked for whole, here or anywhere: the row is not checked, and what
  such a source lends has passed the source's own check". The verify arm
  has grown a read-ahead hook (`BlobOptions::read_ahead`,
  `flatindex::verify_pieces_with`, `open_ahead`) since 682b2c09.
- `Bytes` has `lend_at` (default `piecewise() -> None`), `piece_end`,
  `piece_at`, `ASK_PIECES` since; `MmapBytes` has a private `map()` that
  answers `Some` for `Own::Map` alone. `MmapBytes::paged` is `pub(crate)`:
  only the engine makes a paged mapping, and the only `Paging` impls in
  `src` are the pager (`db::pager::Paged`) and a test `InMemory` in
  ordindex's tests (which never reaches `Blob::open`).
- Implementations of `Bytes` outside src: 15 now (tests 14, examples 1),
  the plan's 11. Two answer `piecewise` true: tests/blob.rs `Piecewise` and
  `Advised<B>` (forwards). None outside tests.
- The plan's open bound: tests/cipher.rs
  `a_store_larger_than_its_cache_reads_as_itself_within_it` (reopen `fills
  <= 6 * files`) -- found by name; form-file bounds are ZFile's, which does
  not go through `Blob::open_with`.
- tests/blob.rs `a_source_that_lends_by_the_piece_answers_the_same` holds a
  piece reader's open to asking under 4096 bytes. With the row verified at
  the open of a source that checks nothing, that bound is the bound of an
  authenticated piece source: the test opens `Piecewise::checked` for it
  (departure, below).

## Verified (rule 10)

- Every source that answers `piecewise` true in the library is a paged
  mapping, made only by `MmapBytes::paged` (pub(crate)) over `Arc<dyn
  Paging>`; in src the only non-test `Paging` is the pager, whose chunks are
  each under an AEAD tag checked before a chunk is lent (pager.rs
  `Paged::slow`/`check`). So `authenticated == piecewise` for every source
  the engine makes, and the change is no behaviour change for any store.
- The wall: `authenticated` matches no token class of tests/source.rs
  (cipher_tokens_in: cipher, crypt, envelope, pager*, Paged/Paging/paged_*/
  ::paged, the attn words, unlent, MergeRest, piecewise/ASK_PIECES). The
  MmapBytes answer is written `self.map().is_none()` so that bytes.rs
  gains no `Own::Paged` (a `paged` token) and no `piecewise`: no count moves.

## What was built

- `Bytes::authenticated` (bytes.rs), defaulted `false`; `MmapBytes` answers
  `self.map().is_none()` (true for a paged mapping, false for a mapped file).
- `Blob::open_with`: where the row is to be verified and the section is held
  by the piece, `verify_row_by_piece` (`#[cold] #[inline(never)]`, blob.rs)
  skips it for an authenticated source and otherwise verifies it through
  `flatindex::verify_pieces_in` (new, `Sect`-generic: the row asked once,
  each piece asked on its own, never the section whole). The whole-section
  arm is the code it was, re-indented under `None =>`.
- tests/blob.rs: `Piecewise` gains `checks` (`Piecewise::checked`) and
  `longest` (the longest single ask); `Advised` forwards `authenticated`;
  new test `a_source_by_the_piece_that_checks_nothing_has_its_row_verified_at_open`.

## Departures

- `a_source_that_lends_by_the_piece_answers_the_same` held any piece
  reader's open to < 4096 bytes asked. That bound is now the bound of an
  authenticated piece source (the one a cipher store has); the test opens
  `Piecewise::checked` for it, and the new test holds the unchecked source
  to being asked every byte, no ask longer than a piece.
- The read-ahead hook (`BlobOptions::read_ahead`) is not given to the walk
  by the piece: no source that reaches that walk in the library (none: every
  library piece source is authenticated) and the hook advises a mapping.
- `Encrypted` (crypt.rs) authenticates what it lends but is never piecewise,
  so it keeps the default `false`; answering `true` would change nothing.

## Red / green

- before.log: the new test red at "a flip at section offset 18471 opened over
  a source by the piece that checks nothing" (the trait method in, the blob
  change not yet), EXIT 101.
- neighbours: blob 20/20, source 25/25, cipher 75/75 (the open bound
  `a_store_larger_than_its_cache_reads_as_itself_within_it` among them),
  segwriter -- neighbours.log EXIT 0.

## Green

- after.log: `cargo test --profile checked --no-fail-fast`, EXIT 0, 1528
  tests passed, no FAILED (started 13:29Z).
- stride.log: `sh scripts/check.sh stride`, EXIT 0.
- lint.log: `sh scripts/check.sh lint lists` (clippy -D warnings, fmt check,
  the door, the ratchet's red cases and the lists), EXIT 0.
- wasm.log: `sh scripts/check.sh wasm`, EXIT 0.
- bench group not run (rule: disk 9.8 GB, the bench target 11 GB; the change
  adds a defaulted trait method, no bench source names it, and cost.sh built
  every probe binary of bench/ against the tree, which is that crate's code
  compiled).

## Count (rule 1)

cost1.log: `COST_DIR=$B/cost1 sh scripts/cost.sh ea0962c2 load_durable
load_buffered commit_durable commit_buffered seal scan_at merge land`, ending
`cost: green`, EXIT 0 (base target removed by the script at its end). Rows 6
and 8 and, beyond them, seal, merge and land (the unit touches flatindex.rs
and Blob::open_with, which a merge's and a landing's opens call).
`Blob::open_with`'s own instructions, by function, base -> head:
merge 5732 -> 5732 and land 1694 -> 1694 on its own line, with 79 of
core::result's lines in the base read as memmap2's in the head (inlining
of `MmapBytes::piecewise`'s match into the `Some`/`None` dispatch, same
total); load_durable and load_buffered 265 -> 268 summed over its lines for
the run's one open (+3 instructions a segment opened, not a key or a step;
under the probes' slack). `verify_pieces_with` is the same to the
instruction in both. No scan, commit or seal function moved. The new
functions (`verify_row_by_piece`, `verify_pieces_in`) run in no probe: no
plain store's source lends by the piece.

## What this holds, and what it cannot see

- Holds: a source by the piece that does not say it authenticates has its
  key section row verified at `Blob::open`, each piece asked once and none
  longer than a piece; a flip in the records fails that open. A paged
  mapping (the cipher store's) still skips: the cipher open bound test is
  what fails if it does not (mutant O1).
- Cannot see: an outside `Bytes` that answers `authenticated` true and
  checks nothing (the method is the caller's word, documented as such);
  `SparseBlob` is untouched (it verifies per piece already). The ordered
  index's skip (`OrdIndex::open`) is left, as the plan says.
- Not changed for any store: every piece source the engine makes is a paged
  mapping, so the behaviour change is for outside piece sources alone. No
  mechanism starts to work under a cipher; nothing owed to a pricing stage.

## Not verified

- That no third party implements `Paging` (it is `#[doc(hidden)] pub`; only
  `pub(crate) MmapBytes::paged` builds an `Own::Paged`, so an outside
  `Paging` cannot reach `MmapBytes` today).
- The bench group (above).
- No timing of any kind was taken; none is owed (no behaviour change for a
  store).

## Files

The finished copies are the working tree's; keep/ holds them with sums and
diff.patch.

## Repair round 2 (results)

- Mutants: X3-X12 and the reviewers' E1-E10 run (mutants4.log EXIT 0,
  results3.txt), every one killed but E8 (three bytes of the word
  compared): no one-byte flip can tell it, the CRC moving in every byte but
  one time in 2^24. Answered with a forged input: a set of bits of one piece
  whose CRC32C deltas cancel in the low three bytes, found by elimination
  over the deltas' linear part; the test asserts that property of its own
  input, the whole reader's refusal naming the piece, and the piece reader's
  refusal equal to it. E8 is killed by it (results4.txt, mutants6.log). X7
  (base 0) is red by "key index checksum mismatch in piece 0" at the open in
  all six tests it fails (out-X7.log). E5 shows the Advised line does reach
  the piece arm (its line-511 assert fails when the wrapper says
  `piecewise` false). mutants.md has the table.
- after2.log: `cargo test --profile checked --no-fail-fast`, EXIT 0, 1529
  passed (round 1 plus the forged test), the cipher open bound
  `a_store_larger_than_its_cache_reads_as_itself_within_it` among them. The
  blob binary in that run was built before the `sort_by_key` edit (made while
  the run was past its compile); neighbours2.log reruns blob, cipher,
  segwriter and source on the final copy, EXIT 0 (21, 25, 20, 75).
- lint2.log: red, EXIT 101, clippy `unnecessary_sort_by` in the new test;
  fixed; lint3.log `sh scripts/check.sh lint lists` EXIT 0 (clippy -D
  warnings, fmt, the door, the ratchet's red cases and the lists).
- wasm2.log: `sh scripts/check.sh wasm` EXIT 0 on round 1's flatindex.rs
  (unchanged since).
- bench.log: `sh scripts/check.sh bench` (build test lint figures), EXIT 0.
  Before it bench/target/release/examples (1.2 GB) was removed; disk 11 GB
  free before and after.
- cost2.log: `COST_DIR=$B/cost2 sh scripts/cost.sh ea0962c2 load_durable
  load_buffered commit_durable commit_buffered seal scan_at merge land`,
  `cost: green`, EXIT 0, on round 1's src (unchanged in round 2). By
  function, as round 0: `verify_pieces_with` the same to the instruction in
  merge (126,264 on its own line) and land (11,128); `Blob::open_with`
  merge 5732 -> 5732 and land 1694 -> 1694 on its own line, its
  core::result lines 79 and 14 lower in the head and the same 79 and 14 on
  memmap2's unix.rs line (the `map()` match of `MmapBytes::authenticated`'s
  sibling `piecewise` inlined differently; same total); load_durable and
  load_buffered 265 -> 268 over the run's one open (+3 a segment opened).
  `verify_pieces_in` and `verify_row_by_piece` appear in no probe: no plain
  store's source lends by the piece. Totals moved -1.16 to +0.62 an op, all
  under the slacks; no function flagged.

## Repair round 1: where it stopped (14:3xZ)

The launcher asked for the result before the round finished. The extra
mutant runner (mutants3.py) was stopped after X1 (killed); the four files
were put back from keep2/ and checked by sum. Not yet done this round: X2-X12,
the count again (cost.sh ea0962c2, rows as round 0) since flatindex.rs's
verify_pieces_in changed, the bench group (`sh scripts/check.sh bench`), the
whole test run, lint/lists/wasm, mutants.md.

## Repair round 3 (results)

- The blocking item: tests/blob.rs's new test now flips the last byte of
  every piece of the key section (15 in its fixture; piece 0 short, 0..12288
  of the section, past the 192-byte header; piece 14 ending at the row),
  each refused by the whole reader for that piece by name and the same by
  the unchecked piece reader. Red input: R4, R5, R9, R10, R11 survive the
  test as round 2 left it (results5.txt "old") and are killed by it now.
- R3 (an empty mapped file answering authenticated) survived: no test
  mapped an empty file, and `Blob::open` refuses one at the superblock. An
  assertion holds it now; killed.
- All eight extra mutants and four own (R9-R12) run; no survivor.
- neighbours3.log (blob 21, cipher 25 incl. the open bound, segwriter 20,
  source 75) EXIT 0; lint4.log (`sh scripts/check.sh lint lists`) EXIT 0.
  Format: the lint group's fmt check is in it.
- src/ unchanged this round (cmp against keep3/): no count again, no wasm
  or bench rerun; cost2.log is the count for this src. The full
  `cargo test` run (after2.log) predates round 3's test-only additions;
  the blob binary that holds them ran whole in neighbours3.log.
- Private TMPDIR removed.
