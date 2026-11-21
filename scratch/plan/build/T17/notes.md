# T17 upstream-count: notes

Step: repair round 2 done. Finished copies: $U/keep-r2/ (keep-r2.sha256), the tree at the round's end;
keep-r1/ is round 1's (R1e), keep/ round 0's. CLAUDE.md edited in rounds 0 and 1.

## Base
- `git merge-base HEAD origin/main` = 5aad09dd (main's squash of PR #20, 5 Nov). It is an
  ancestor of 9bfc4362 (277 commits behind it); 9bfc4362 is upstream's branch head and an
  ancestor of HEAD (merged at bb365602). So the upstream this branch last merged is 9bfc4362,
  and counting against 5aad09dd would bill upstream's own 277 commits to this branch. Base used:
  9bfc4362, as the brief says. Departure from "use what merge-base says", for that reason.

## Runs (read each by its EXIT line)
- before: $U/cost-before.log (COST_DIR=$U/cw, COST_BASE_TARGET=$U/tb kept)

## Step 1: scan_at understood (by instruction, `ins/perins.py` over callgrind --dump-instr)
Probe run before: $U/cost-before.log (EXIT 1; scan_at +239.37, land +274.40), outputs kept in
$U/before-out/. Per instruction of `scan_at_probe::scans` (base 9bfc4362 vs head):
- The fast walk's loop (walk_fixed!, executed 100,000 times): 57 instructions an entry at the
  base, 59 at the head: no added step -- the same source, the loop's registers allocated
  otherwise (an `xor eax` and two extra `mov %rbp,%r9` copies of the records' length live into
  the loop's break edges). The compiler's, and the caller that grew is the probe's own `scans`:
  `Blob::seek` is inlined into it, and on this branch `seek` is `indexed!`, whose by-the-piece
  arm (`Blob::pieces`, `seek_with::<Pieces>`) is inlined beside the slice's; likewise
  `exts_at_full` (the general walk) brings `at_full::<Pieces>`, `bytes::take`, `slice_slow`.
- The fence's search loop (inlined from `seek_with`, 12.675 iterations a scan): 42 -> 45, one
  more spill reload across the memcmp call and a branch pair merged by setcc/or. Compiler's.
- `FlatIndex::search` (out of line, 4.056 iterations a call): +3 a call, one more value
  (the section's base) spilled across the memcmp call. Compiler's: source the same, now
  generic over `Sect`.

## Shapes counted for scan_at (q.sh TAG scan_at; each listing in $U/q/TAG/scan_at.self)
- s1 `seek`'s by-the-piece arm out of line (`seek_pieces`, cold, inline(never)): total +42.4
  a scan; the loop back at 57 an entry; fence loop still 45; search +3. KEPT.
- s2 s1 + fence_key reads its two offsets as one u64: +140.1. Refuted, reverted.
- s3 s1 + fence_window inline(never): +283.3 (fence_window +687, scans -406). Refuted.
- s4 s1 + exts_at_full's by-the-piece arm out of line: +419.3. Refuted (the loop's registers
  went the other way). The general walk's arm stays in `indexed!`.
- s5 s1 + walk_fixed! moves `$rank` once at the loop's end by what `$seen` moved (they move in
  lockstep, a record each): total -68.6; search +3 still red. KEPT.
- s6 s5 + `FlatIndex::search` resolves the records and the directory once before its loop:
  total -373.0 a scan (search now inlined into scans, -232.9; scans -140.1). GREEN. KEPT.

## Step 1: land understood (per function, per inlined callee via --dump-instr + addr2line -i;
## per source line via $U/fnlines.py over the out files; base sources archived at $U/cw/base)
Reading of G1-3 verified by function: a diagnostic build with `identity_of` answering any
claim (upstream's rule; $U/q/diag-upstream-identity) took `CachedBlock::lower_bound::{closure#0}`
(+193.4) to 0 and `splice_run` (+81.0) to level, and the probe to -6.77 an op. So 324a395a's
claim by partition is what made every patch into a copy carried across the land probe's
merges a search by key.
The other gainers of `land`, each:
- `BuildCtx::copy_block` +28.73: a MOVE. `BuildCtx::emit_over` (-40.44) is inlined into it on
  this branch (its call line in copy_block is gone, its lines are copy_block's); net -1.17M.
- `Db::op` +35.01 / `Reader::enter` -35.01, `WriterOp::drop` +37.01 / `Reader::leave` -49.03:
  MOVES (the bracket of an operation, 55c3e526 "a store in the clear pays nothing").
- `Blob::values_at_pieces::{closure#0}` +14.875 / `Blob::values_at::{closure#0}` -14.875: MOVE
  (a rename by inlining, same count).
- `Blob::key_at` +7.90, `Blob::scan_whole` +7.76 / `Blob::flat` -17.59: MOVE (G1-3's reading).
- `Reader::settle_each` +22.26 before: the compiler's, and two hint sites. Its source is
  upstream's but for the claim's test at `by_cut`; what grew is what is inlined into it from
  `ordindex` (eb377fe4, 55c3e526, 198ed453: `top()`'s empty test, `head()`'s bound against
  `lim` with a call that returns, `head_absent`): the seek's binary search over the stride's
  heads (525k probes) read 18 instructions a probe against 15, three more values spilled
  across the loop for the returning call; `plan_seek` +12 a call. With 324a395a's claim the
  hint sites skipped their prefetches for carried copies, which hid 15 of the 37.
- `CachedBlock::prefetch` +4.25: the hint site of a scan's start that 324a395a sent to the
  whole copy for a carried one (step of 324a395a; a store in the clear needs only a hint).
- `__memcmp_avx2_movbe` +13.2: the search's compares (324a395a).
- smaller: `Reader::patch_block` +3.4, `Blob::exts_at` +1.1, `PartReader::raw_walk` +0.46,
  `Reader::cut_raw` +0.30, `OrdIndex::whole_key_at` +0.52, `BuildCtx::give_offs` +0.21: present
  in G1-3's run too; not touched by this change (see step 3's table).

## Shapes counted for land (q.sh TAG land; listings $U/q/TAG/land.self)
- L1: splice_run takes the rank's entry under any claim where its key is the key in hand
  (key_cmp), the search where not: +8.33 (splice_run +22.25, memcmp +3.98). Kept, refined.
- L2: L1 + the hint sites (copy_entry_at, by_cut, prefetch_block) take any claim, as
  upstream's did, and a scan's start takes the rank under a claim of another partition where
  `starts_at` proves it by two compares: +18.99 (settle_each back to +37: the hints cost
  what they cost upstream). Kept: the hints are upstream's behaviour.
- L3: L2 + `seek_exact_planned` reads heads under `lim` with a bound that panics
  (`head_held`): +8.42, settle_each +26.6.
- L4: L3 with the heads as one slice loaded once (`heads_held`) and `plan_seek`'s empty-index
  test folded into its top level's: -1.13, settle_each +19.6.
- L5: L4 + splice_run counts a form's bytes again only where the patch could move them (an
  in-place run in a copy it did not clone moves none): -20.64; splice_run +0.24.
- L6: L5 + the patch's compare as `key_eq` (no call for a 16-byte key's empty tail):
  -33.88; splice_run -9.01, memcmp level. KEPT.
- L7/L8: exact_at given the head the search read (out of line: -9.28; inline(always):
  -32.95 with settle_each +20.5). Refuted, reverted to L6.

## Mutant passes (runner $U/mutants.py; verdicts $U/mutants.results; logs $U/mutant-*.log)
First pass left M4, M13 (the scan start's two-compare proof under a stale claim, `starts_at`),
M5 (the scan's piecewise refusal), M9, M14, M15, M16. Each answered:
- M4/M13: `starts_at` reached by no test. Taken out: the scan's start takes the rank under the
  claim of its own partition alone, as 324a395a has it, and searches otherwise. Its replacement
  mutant M4b (the scan start's claim of any partition, i.e. 324a395a taken out there) survives
  too: pre-existing, not this change's line. Why nothing reaches it: a rewrite kept by
  `same_blocks` moves a block's ranks only by a fold and a drop inside that block, the fold's
  write is filed into that block's copy as an insert (identity 0) by the settle every scan makes
  before it walks, so a scan's start never meets a copy whose claim is stale and whose ranks
  moved. That is an argument about histories, so the start keeps its search there.
- M5: pre-existing gap: the engine's record walk answers what the windowed walk answers, so no
  answer told them apart. New unit test in src/blob.rs
  (`the_whole_walk_refuses_a_source_by_the_piece_and_the_engines_entry_takes_its_walk`):
  `scan_whole` refuses a source by the piece with `Unsupported`, and `scan_either` answers what
  `scan_at` does. Killed.
- M9 (`FlatIndex::search` answering `hi` where its regions do not resolve): an equivalent
  mutant on a path nothing reaches: `FlatIndex::parse` refuses any region outside the section,
  and the value on the path is the one the loop before gave (every probe a key that will not
  read narrows `hi` to `lo`). Two shapes without the branch were counted: s7 (the regions as an
  `Option` pair matched in the loop) put `search` back out of line at +2 a call, red. Kept as is.
- M14/M15: `key_eq`'s words and tail: the test file's claim test now runs over keys of 4, 16 and
  19 bytes (`claim_across_rewrite`), with a record between the dropped one and the folded one
  written first in their batch, so its patch meets a carried copy whose rank names another key.
  Killed by the 16- and 19-byte tests.
- M16: new unit test `a_mapped_index_plans_from_the_top_level_read_at_its_open`. Killed.

## Step 4: the whole run against 9bfc4362 ($U/cost-whole.log, EXIT 1, `cost: RED` by function)
Every probe's total at or under upstream's (per operation; G1-3's run in brackets):
read -5.11 (-6.15); read_z -0.38 (-0.79); scan -355.57 (-52.60); scan_long -6008.33 (-52.92);
scan_z -566.32 (-566.32); scan_long_z -11491.05 (-11499.55); load_durable -8.62 (-5.53);
load_buffered -1.47 (-4.94); commit_durable -7.88 (-7.80); commit_buffered -7.94 (-7.79);
work -11.83 (-11.76); merge -210.67 (-212.28); land -28.81 (+276.99); seal -1.02 (-1.03);
scan_at -373.05 (+239.39). The engine's scan and scan_long gained from `FlatIndex::search`
hoisting its regions (`Blob::scan_at_in`, where the engine's seek inlines it, -301 and -5954).
The listing by probe is $U/pairs-whole.txt (every function over 0.05 an op, gained and lost).

## Step 3: the other probes' gains paired with their losers (net an op), from the whole run
| probe | gained | lost | net | reading |
|---|---|---|---|---|
| read | read_all_in::{closure#5} +189.36 | read_all::{closure#5} -187.35 | +2.01 | move (renamed by inlining) |
| read | leave_looked +35.00, supdb_get +8.00 | leave -51.00 | -8.00 | move: the read's bracket (55c3e526) |
| read | __memcmp_avx2_movbe +1.88 | read_exts -1.00 | | libc, by the addresses (G1-3 read +0.84) |
| read_z | each_value +64.00, read_exts_inline::{closure#2} +15.00, part_read_z +22.55 | read_exts -95.00 | +6.55 | move (read_exts_inline out of line for the form's reads, CLAUDE.md) with part_read_z +6.5 left |
| read_z | leave_looked +35.00, supdb_get +4.00 | leave -49.00 | -10.00 | move |
| read_z | zpages::view_with +3.05 | (none) | +3.05 | NO LOSER: a third cost, small, in the compact form's view; not this change's; reported |
| scan, scan_long | scan_whole +61, scan_seek +15, scan +5, raw_scan +1, key_at +0.06 | scan_at -69, flat -35.03, scan_blocks -16.80 | -38.8 | move (G1-3's reading: flat into key_at/scan_whole) |
| scan, scan_long | leave_looked +35 | leave -51 | -16 | move |
| scan_z | view_with +14.19, open_at +10.23, parse_page +7.26, ask_group_ahead +7.24, scan +12.00, pwalk_seek +1 | walk_impl -600.63, first_checks -4.36, load -3.61, group_of_key -1 | -556 | the compact form's helpers inlined differently (G1-3); the gainers have no one-to-one loser, walk_impl's loss covers them; NOT verified by instruction |
| scan_long_z | view_with +151.73, open_at +146.89, ask_group_ahead +110.67, _int_malloc +40.44 | walk_impl -11769.35, open_group_with -106.21, load -71.91 | -11491 | as scan_z; _int_malloc +40 a scan is the allocator, by addresses |
| load_durable, commit_* | pwritev_all +0.09, WriterOp::drop +0.08, Db::op +0.07, memset +1.25 (load) | append_batch -7.00, commit_direct -1.01, stream_hash -0.21, writev_all -0.10, leave/enter -0.18 | -8 | moves (writev_all to pwritev_all, the bracket) and the run's write path lighter |
| load_buffered | zselect::parse_mount +1.22, zfile::read_budget +1.09, CharSearcher::next_match +0.85, allocator +1 | (the same functions lose in load_durable: -0.49, -0.42, -0.32) | | the controller's reading of the memory group on a clock: noise of a probe with no toggle, both signs across the two loads |
| work | work_probe::main +23.32, WriterOp::drop +18.87, leave_looked +17.79, Db::op +16.93 | leave -50.62, enter -16.93, emit_unsealed -11.66, push_chunk -6.00, scan -3.17 | -11.8 | moves: the bracket, and the engine inlined into the probe's main |
| merge | key_at +252.02, values_at_pieces::{closure#0} +238, exts_at +27 | flat -315.01, values_at::{closure#0} -238 | -36 | move (G1-3) |
| merge | compact_run +112.49 | merge_ranks::{closure#1} -284.00, merge_keys::{closure#1} -4 | -175.5 | move by inlining (merge_ranks' closure into compact_run), NOT verified by instruction |
| merge | Wal::replay_frames +87 | Wal::replay -96 | -9 | move |
| merge | WriterOp::drop +37.76, Db::op +35 | leave -62.52, enter -35 | -24.8 | move |
| merge | sweep_retired_states +17.26, whole_key_at +10.89, seal_table::{closure#4} +5, append +4, Emitter::enter +3 | (none named one-to-one) | +40 | NO ONE-TO-ONE LOSER: inside a probe with no toggle, a hundredth an op being its slack; their sum is under the merge's other losses; reported, not decomposed |
| land | WriterOp::drop +37.01, Db::op +35.01 | leave -49.03, enter -35.01 | -12 | move |
| land | copy_block +28.73 | emit_over -40.44 | -11.7 | move (emit_over inlined into copy_block: its call line gone, its lines copy_block's) |
| land | values_at_pieces::{closure#0} +14.88 | values_at::{closure#0} -14.88 | 0 | move |
| land | key_at +7.89, scan_whole +7.76 | flat -17.59 | -1.9 | move |
| land | settle_each +19.57 | (none) | +19.57 | THE COMPILER'S: its source is upstream's but for the claim's hints; what grew is ordindex's code inlined into it (top()/top_late, head()'s bound against lim, plan_seek), from eb377fe4/55c3e526/198ed453. +37 before this change; heads_held and the folded empty test took 17 of it. Left, reported |
| land | patch_block +3.38, exts_at +1.12, whole_key_at +0.52, raw_walk +0.46 | overlay_runs -4.78, raw_scan -4.20, rank_before_publish -2.99, cuts_of -2.55 | -9 | no one-to-one pairing; present in G1-3's run; not this change's |
| seal | (none) | AlignedWriter::write_all -1.01 | -1 | |
| scan_at | (none) | FlatIndex::search -232.88 (inlined into the probe), scans -140.07 | -373 | this change |
The gate stays red by function on these moves, as G1-3 found: a move reads red with nothing to
say it is one (the declared group held to its net is the later change).

## The tree against HEAD ($U/cost-vs-HEAD.log): `cost: green`, EXIT 0
read +1.88 (`__memcmp_avx2_movbe` alone, libc, within its slack), read_z +0.29, scan -303.14,
scan_long -5955.49, scan_z +6.79 (memset +5.48, _int_malloc +3.09: libc and the allocator,
within their slack, no engine function moved), scan_long_z +0.45, load_durable +0.64 (the
controller's clock-driven cgroup reads), load_buffered -2.56, commits -0.06/-0.07, work -0.23,
merge -6.66, land -304.40, seal -0.03, scan_at -612.51.

## Step 5: under a cipher (HEAD's build against the tree's, the toggles of cost.sh;
## listings $U/cipher/{HEAD,tree}.{read,scan}.self)
- `read_only supdb-cipher 100000 1 n-none`: -2.24 an op, `__memcmp_avx2_movbe` alone (libc).
- `lag0_probe supdb-cipher 100000 1` (a scan by the piece, the windowed walk): -198.94 a scan:
  `Blob::scan_pieces` -200 (walk_fixed! moves the rank once, its windowed loop too), and
  `Blob::scan_at_in` +1 a scan (the record walk's entry after the refusal; the compiler's,
  1,000 over the run: red under the toggle's 64 had it been the plain store's; it is the
  cipher store's path, reported).
- The patch into a carried copy: no cipher twin of `land` exists. The code is the plain
  store's, shared: under a cipher the ordered index is read by the piece (`lim` 0), so the
  planned seek takes `head()` as before (`heads_held` is empty and never asked), `plan_seek`'s
  folded test reaches `top_late` as `top()` did, and the patch's compare is the same. Not
  counted.
- Nothing starts working under a cipher because of this change.

## What the change holds, and what it cannot see
- Holds: a patch into a copy carried across a rewrite of its partition takes the rank's entry
  only where `key_eq` proves the entry is the key's (or the claim names the partition in hand),
  red without it in three tests of tests/settle.rs (4-, 16- and 19-byte keys); the fuzz with
  the block cache on (ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1) over seeds 0..2500 plain and 0..450
  under a cipher, five and three processes at once on four cores, FAILURES 0 in every range
  ($U/fuzz-*.log, $U/fuzzc-*.log), and seed 1179 under ZFUZZ_SETTLELATE.
- Cannot see: the scan's start under a stale claim still searches (M4b, the start's 324a395a
  line, is held by no test; argued unreachable, notes above); the piecewise refusal of the
  whole walk (M5) is held by no test of the tree (a unit test that holds it needs a couplings
  raise, and scripts/ratchet-red.sh's l_deregistered case goes red under any real raise of
  tests/data/couplings.txt: $U/lists.log of that attempt shows it; reported for T18's owner);
  `settle_each` stays +19.57 an op over upstream by function (the compiler's, ordindex's
  cipher-era code inlined into it), so `cost.sh 9bfc4362` stays RED by function on moves and
  on that one, with every total under upstream.
  [Round 1 supersedes two of these: the piecewise refusal is held by tests/blob.rs now (R1_M5
  killed, an integration test, outside the couplings lists), and settle_each reads -8.71 under
  upstream. M4b is still held by no test.]

## Departures
- Base 9bfc4362, not `git merge-base HEAD origin/main` (5aad09dd, 277 commits older than the
  upstream this branch last merged): counting against it would bill upstream's own commits.
- Shapes were counted with $U/q.sh (the tree's probe built by cargo in bench/, run under the
  probe's callgrind toggle, compared by scripts/cost-diff.py against the base's output of
  $U/cost-before.log) between the before run and the whole runs, not by cost.sh each time:
  the same comparator and the same base output, without rebuilding the base worktree.
- The brief's second land shape (the claim re-established at the carry) not built: a proof at
  the carry is a walk of the copy against the partition, as many compares as the patches it
  saves in the probe, or a merge-side count of keys folded or dropped, a step a key in the merge
  probe, which holds a hundredth an op.
- The hints ahead of a patch and a walk (`copy_entry_at`, the patch's `by_cut`, `prefetch_block`)
  take any claim again, as upstream's did: 324a395a had them follow the claim by partition,
  which saved instructions and dropped the prefetches for carried copies.
- splice_run counts a form's bytes again only where the patch could move them (new; the
  saving that pays for the compare inside the same function).
- I ran `git checkout -- scripts/raises.txt tests/data/couplings.txt` once, to take back my own
  edit of those two files (they were clean at the start, git status empty); nothing else was
  touched by a checkout.
- CLAUDE.md gains one paragraph (a cold path that returns into a hot loop), with the two
  findings that taught it.

## Owed (timing, for whoever prices it; none taken here)
- `sh scripts/check.sh quick` on the committed tree, read against the last rows, for the scan
  pass (`FlatIndex::search` hoisted, the engine's scans -300 instructions a scan) and the lag
  sweep's bursts (the settle's planned seek, the patch's compare and byte count).

## Not verified
- scan_z/scan_long_z and merge pairings marked NOT verified in the table (read off function
  totals, not by instruction).
- That the hint sites' prefetches pay for their instructions on a carried copy (upstream's
  behaviour, restored; a timing question).
- valgrind's counts on another host; cross-arm and wasm groups were not run (no public
  signature of Bytes, Blob or MmapBytes changed).

## Repair round 1 (reviewers: settle_each +19.57 in land; M5 survives; eleven extra mutants)
- Step R1: cost.sh 9bfc4362 land scan_at at the tree as the round found it, COST_DIR=$U/r1/cw,
  COST_BASE_TARGET=$U/tb (kept, the base binary read by --dump-instr after); log $U/r1/cost-start.log
- R1 start ($U/r1/cost-start.log, EXIT 1): land -36.13 an op, settle_each +19.569 (reproduced),
  scan_at -373.05. Base binary kept at $U/ins/base.land_probe, tree's at $U/ins/L6.land_probe.
- settle_each by instruction at L6 (--dump-instr, `ins/se.sh`, inline frames `ins/*.se.agg{1,2,3}`):
  the search loop over the stride is 12 instructions a probe at both (525k probes); the gain is
  outside it: plan_seek +14 a call (the late top's returning call `top_late()` kept the query's
  head `h` and the position `was` on the stack, reloaded at each compare, and `n` from memory),
  cache_owner_planned +9 a seek (`heads_held`'s `lim` tests, `head(lo)`'s `lim` test beside
  `lo >= n`, the self pointer reloaded), the zc filter's closure +7 (the plan an xmm copied
  through the stack twice). The returning calls are `top_late` and `head_absent`.
- Shapes (each listing $U/q/<TAG>/land.self; R1a by instruction $U/ins/R1a.*):
  R1a: plan_seek's late arm out of line (`plan_seek_late`, cold), and the planned seek's by-the-piece
       arm out of line (`seek_planned_by_piece`), the tie test over the held heads (`exact_tied`):
       settle_each +7.911, land -39.93. KEPT.
  R1b: R1a + `Blob::key_at`'s by-the-piece arm out of line (`key_at_pieces`): key_at +7.89 -> +7.14,
       settle_each +7.911 (no move). KEPT.
  R1c: R1b + the stride's probes read unchecked over a slice the plan's bounds cut (the bound
       checked once, at the slice; `select_lower_bound` asks only below its `n`): settle_each
       -8.712, land -59.75; scan_at -373.01. KEPT. What is left of the codegen's growth (R1b's
       +7.9: the plan materialized through the stack, the filter's copies) is still there; the
       per-probe bound test the base made (`assert!(i < self.n)` in `head`) is gone, 2 a probe.
  R1d: R1c + `FlatIndex::search`'s `mid < nkeys` test taken out (every caller searches below the
       key count; the extra mutant that drops it reads equivalent): scan_at -373.01 -> -90.29, the
       probe's `scans` +142.6 over the base where R1c read -140.1 (282 a scan, the loop's codegen).
       Refuted, reverted: the test stays, its mutant an equivalent one (mutants.md).
- Finished copies of this round: $U/keep-r1/ (names with `/` as `_`), sums $U/keep-r1.sha256.
- Mutant pass of this round: runner $U/mutants_r1.py, verdicts $U/mutants-r1.results,
  logs $U/mutant-R1_*.log, run log $U/r1/mutants-run.log (read by its EXIT line).
- Whole run at the round's tree: /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T17/r1/cost-whole.log (COST_DIR /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T17/r1/cw-whole)
  EXIT 1 (`cost: RED` by function on the moves of step 3, as before), every total under upstream:
  read -6.15, read_z -0.78, scan -355.80, scan_long -6008.08, scan_z -562.77, scan_long_z
  -11501.65, load_durable -6.36, load_buffered -6.98, commit_durable -7.96, commit_buffered
  -7.95, work -11.89, merge -229.00, land -52.26, seal -1.02, scan_at -373.01 (an op).
  Pairs by probe: $U/r1/pairs-whole.txt. Against round 0's table (step 3) the rows are the same
  pairs but these: land `Reader::settle_each` is a LOSER now, -8.71 (was +19.57, no loser);
  `Blob::key_at` +7.14 in land and +228.02 in merge (was +7.89 and +252.02; `flat` -17.59 and
  -315.01 its losers as before, a move); land's `__memcpy_avx_unaligned_erms` +4.59 is libc noise
  of the base itself: the base's own count of it read 19.26M, 19.74M, 19.87M and 19.88M in four
  runs of one base build ($U/r1/cw*/out/base.land.self, $U/q/base.land.self), a spread of 6 an op.
  No gainer without a loser is left in land; the third costs round 0 reported (read_z
  `zpages::view_with` +3.05, merge's small gainers) stand as they were, not this change's.
- The two count mutants (R1_X8, R1_O5) are held by the gate by name (mutants.md); the probe's
  exit was RED before them, so the named line is the verdict.
- `cost.sh HEAD` at R1c ($U/r1/cost-vs-HEAD.log, EXIT 1): land's `Maint::rank_before_publish`
  +0.054 an op (+5,433 over the run, exactly one instruction a call of 5,467: a register copy
  after the call in `bound_pieces_of`'s seek closure, `ins/HEAD.rbp.*`, `ins/R1c.rbp.*`), and
  load_durable's `resident_share` +0.130 (a probe with no toggle). R1e (R1c with R1b's `key_at`
  taken back) puts rank_before_publish at HEAD's count to the instruction ($U/q/R1e/land.self):
  R1b's out-of-line arm of `key_at` saved key_at 0.75 an op and cost its callers' register
  allocation, so R1b is REFUTED and reverted; R1e is the round's finished shape (land -56.56
  by the q run; settle_each -8.712, unchanged). The whole runs are taken again at R1e:
  $U/r1/cost-whole-R1e.log and $U/r1/cost-vs-HEAD-R1e.log.
- Whole run at R1e ($U/r1/cost-whole-R1e.log, first part; pairs $U/r1/pairs-whole-R1e.txt): EXIT 1
  (`cost: RED` on step 3's moves), every total under upstream, per op: read -6.33, read_z -0.73,
  scan -355.46, scan_long -6008.41, scan_z -562.77, scan_long_z -11501.65, load_durable -7.69,
  load_buffered -10.81, commit_durable -7.96, commit_buffered -7.95, work -11.68, merge -224.64,
  land -54.97, seal -1.02, scan_at -373.01. Pairs as round 0's table but land's
  `Reader::settle_each`, now a LOSER at -8.71 (round 0: +19.57 with no loser); key_at back at
  round 0's +7.89 / +252.02 (moves with `flat`); land's memcpy +1.33 and merge's -7.48 libc.
  The tree's cost.sh HEAD at R1e: $U/r1/cost-vs-HEAD-R1e.log (a first try died at the base's
  link, `ld terminated with signal 7`, the disk at 2 GB: $U/tb/release/examples removed, 4 GB).
- cost.sh HEAD at R1e ($U/r1/cost-vs-HEAD-R1e.log): `cost: green`, EXIT 0. (The R1c run's
  load_durable `resident_share` +0.130 did not recur: the controller's clock-driven readings in
  a probe with no toggle.)
- Step 5 again (cipher): a scratch twin of the land probe under `Encryption::Cipher`
  ($U/r1/land_cipher_scratch.rs, copied into bench/examples for the two builds and removed after),
  built at HEAD (worktree $U/r1/wt-head, target $U/tb) and at the tree; read_only and lag0_probe
  supdb-cipher at the tree against round 0's HEAD listings ($U/cipher/HEAD.*). Outputs $U/r1/cipher/.
  Results (cost-diff.py, toggles as cost.sh): $U/r1/cipher/{head,tree}.land.self, tree.{read,scan}.self.
  - land under a cipher: -231.48 an op. The patch into a carried copy: `CachedBlock::lower_bound`'s
    closure -193.37 and `splice_run` -90.01 (the claim proven by its compare where HEAD searched).
    The settle's planned seek, which a cipher's ordered index (read by the piece) takes out of line
    now: settle_each -210.15 against `seek_planned_by_piece` +162.69, `plan_seek_late` +66.99,
    and the helpers those cold functions call rather than inline, `cmp_short` +24.50, `head_of`
    +14.00, `lower_bound` +9.15: +67.2 a key net, A COST UNDER A CIPHER of this round, reported.
    `Pieces::key` +6.30 / `FlatIndex::key_at` -5.16 (a move). Two shapes to take the +67 back were
    counted on the plain probe and refuted, each costing the plain store: R1f (the two arms not
    `#[cold]`, still out of line) settle_each -6.44 against R1e's -8.71; R1g (`head_of`,
    `cmp_short`, `lower_bound` `#[inline(always)]`) settle_each -1.63. Reverted; the plain store
    first, and the cipher's +67 a planned seek is an open item for the page layer's stage.
  - read_only supdb-cipher: -2.23 an op, `__memcmp_avx2_movbe` alone (libc).
  - lag0_probe supdb-cipher (the scan by the piece): -198.57 a scan, `Blob::scan_pieces` -200,
    `Blob::scan_at_in` +1 a scan (round 0's reading: the record walk's entry after the refusal).
- Fuzz of this round: /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T17/r1/fuzz/*.log (plain 0..2500 five at once, then cipher)

- Round 1's tests ($U/r1/after-tests.log, EXIT 0): lib 524, blob 17, cipher 25, db 261, dict 4,
  fences 13, segwriter 20, settle 18, source 75, all passed. Fuzz with the block cache on
  ($U/r1/fuzz/): plain seeds 0..2500 five at once, cipher 0..450 three at once, FAILURES 0 in
  every range. `check.sh lint` exit 0, `check.sh lists` exit 0, `scripts/fmt.sh` exit 0
  ($U/r1/lint2.log; the first try's `| tail` hid the status and was not read as a verdict).
  Removed at the end: $U/tb (the base's target), the test stores, the probe copies.

## Round 1: what changed, in sum
- src/ordindex.rs: `plan_seek` out of line where its top level is late (`plan_seek_late`, cold);
  `seek_exact_planned`'s by-the-piece arm out of line (`seek_planned_by_piece`, cold), the held
  heads' tie by `exact_tied`, the stride's probes unchecked over the slice the plan cuts. New test
  `an_index_of_no_keys_plans_nothing`.
- src/db.rs: `select_lower_bound`'s doc states the contract the unchecked read rests on (doc only).
- src/blob.rs: docs of `scan_at_paced` and `RunWalk::scan_at` say they refuse a source by the
  piece (doc only; R1b's `key_at` change taken back).
- tests/blob.rs: `the_whole_walks_refuse_a_source_that_lends_by_the_piece`,
  `a_seek_over_a_section_that_does_not_read_answers_the_end` (and the `StopsLending` source).
- tests/settle.rs: the claim test over 17-byte keys and over a whole block (`part` 16384).
- CLAUDE.md: round 0's paragraph corrected to what the instruction count found.
- settle_each by function against 9bfc4362: +19.57 -> -8.71 an op. Of that, R1a's two arms out of
  line took it to +7.91, and the remaining codegen growth (the plan stored and copied through the
  stack, the filter's copies: R1a's listing) is still there, outweighed by the per-probe bound
  check the base made and the tree no longer makes (R1c). That is said plainly: the +7.9 of the
  function's own growth is not removed, it is paid for inside the same function.

## Owed (timing, none taken)
- The quick row (`sh scripts/check.sh quick`) at the committed tree for the lag sweep's bursts
  (the settle's planned seek) and the scan pass, and a pairing under a cipher of the bursts
  (`bench/scripts/pair.sh supdb-cipher supdb 300000 --passes lag`), where the planned seek pays
  +67 instructions a key out of line (step 5).

## Repair round 2 (reviewers: no blocking finding; ten extra mutants)
- Start: the tree's six files are keep-r1's but for `cargo fmt` of two lines (ordindex.rs's and
  settle.rs's new test lines, formatting only: `diff` shows it). Finished copies of this round
  start at $U/keep-r2/ (sums $U/keep-r2.sha256), CLAUDE.md included.
- Mutant pass: runner $U/mutants_r2.py, verdicts $U/mutants-r2.results, logs $U/mutant-R2_*.log,
  run log $U/r2/mutants-run.log (read by its EXIT line). M4b under the fuzz: $U/r2/fuzz-M4b-*.log.
- Round 2's verdicts, each extra mutant (rows in mutants.md):
  - walk_fixed!'s rank by the cumulative count (R2_A) and the windowed walk's rank moved from the
    first window (R2_B): KILLED, but only in tests/cipher.rs (14 tests); tests/blob.rs's agreement
    of the three sources passed both, because its records have values of mixed lengths and the
    walk by the piece hands them to the general walk at the first record, so no scan reached a
    second window. New test `a_walk_by_the_piece_across_its_windows_answers_as_the_mapped_one`
    (records of one width, inline, full and compact, 300 keys, pieces of 64/4080/64K, scans of
    65..MAX from ranks 0..130): R2_A2 and R2_B2 (the same edits, tests/blob.rs alone) KILLED by it.
  - splice_run's own claim inverted (R2_C): KILLED in five claim tests, by the debug assertion
    "a copy's identity entry disagrees with its search" alone (as M1 was): a checked build's kill.
  - the tie read at `lo - 1` (R2_D), plan_seek's late arm by `n == 0` (R2_E): KILLED in ordindex's
    unit tests (and settle's for R2_D).
  - the stride cut `heads[base..hi.max(base)]` (R2_F): SURVIVED, equivalent: plan_over makes no plan
    with lo > hi and none past n. Answered as the reviewer asked: the planned seek asserts the
    plan's order and its end within the heads in a checked build (debug_assert, nothing in a
    release build), with a red-first unit test
    `a_plan_out_of_order_or_past_the_heads_is_refused_in_a_checked_build` (red before the
    assertion, $U/r2/before.log; green after, $U/r2/after.log). R2_F2 (the mutant again under the
    assertion) survives, equivalent under the asserted invariant. Own mutants on the assertion
    R2_O1..O4 all KILLED.
  - `||` -> `&&` at the patch (R2_G): SURVIVED the tests (equal answers) as expected; the cost
    comparator names it: `HOT GAINED CachedBlock::lower_bound::{closure#0} +193.378 an op`,
    `Reader::splice_run +63.744` ($U/r2/cost-R2_G_claim_and_compare.log; q.sh against the base's
    9bfc4362 output, the comparator cost.sh runs).
  - bytes_kept = true in place only (R2_H): KILLED by the debug assertion "a run written in place
    moved its form's bytes" (1650 vs 1793) on the upkeep thread in
    a_burst_lent_to_the_upkeep_thread_is_filed_before_the_first_scan (and a budget test): a test
    does reach a cloned in-place patch under the capacity count.
  - the search's unresolved regions answering `hi` (R2_I, round 0's M9): SURVIVED, equivalent; the
    region check has not moved since round 0 (it was rerun anyway).
  - M4b under the fuzz ($U/r2/m4b_fuzz.py; logs $U/r2/fuzz-M4b-*.log; reaches noted in
    $U/r2/m4b-stale-starts.txt): plain seeds 0..2500 five at once, cipher 0..450 three at once, with
    ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1: every range passed. The fuzz DOES reach a scan start over a
    copy whose claim names another partition, 1808 times, and every one at the block's first entry
    (`start - lo == 0`), where a rewrite kept by `same_blocks` keeps the first record, so the rank's
    entry and the search agree (the checked build's assertion at the start held at each). So the
    notes' "unreachable" is corrected: a stale claim is reached at a scan's start, at offset 0
    only, by every history the fuzz drew. M4b survives; the line is 324a395a's, not this change's,
    and searches there as it did.
- Release code: the round's only engine edit is a `debug_assert!` (src/ordindex.rs,
  seek_exact_planned). Counted: q.sh land and scan_at at the round's tree ($U/q/R2/) against
  round 1's R1e listing: no supdb:: function moved but `Maint::join_seal` (+0.004 an op, inside its
  bound), and std's fs wrappers (+334..+514 over the run, the probe's store paths, which carry the
  pid); scan_at -373.01 an op against the base, as R1e. land -62.15 against the base's output.
- Tests ($U/r2/all-tests.log): `cargo test --profile checked --no-fail-fast`, every binary green
  (27 `test result: ok`), EXIT 0. Fuzz at the tree ($U/r2/fuzz/): plain 0..2500, cipher 0..450,
  FAILURES 0 and `test result: ok` in every range (the logs' EXIT lines are the pipe's `tail`, not
  the test's; the verdict is the result line). `check.sh lint` EXIT 0, `check.sh lists` EXIT 0,
  `scripts/fmt.sh --check` EXIT 0 ($U/r2/{lint,lists,fmt}.log).
- Finished copies: $U/keep-r2/ (sums $U/keep-r2.sha256), the tree's files at the round's end.
- CLAUDE.md: no rule added this round (the windowed walk's lesson is CLAUDE.md's "a test that
  passes is read for the path that passed it", already there).
