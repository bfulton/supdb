# M1 upstream-merge: notes

Upstream head of this change (fetched once, 2026-11-21 19:50 UTC): 7c62153ee45fc904a89ac697c0252ded1b04e33e
Merge base: 9bfc43627c8707c50fb53e835ff76b916d841a11 (verified with git merge-base). 31 upstream commits.
Branch tip at start: b1e9a809 (status clean).

## Step
- [x] merge --no-commit --no-ff started 19:52 UTC; conflicts CLAUDE.md, src/db.rs, tests/fuzz.rs
- [x] Repair round 3 done (see the section at the end)

## What differs from the plan / trial merge
- The real merge is of 7c62153e, the same head the brief's trial merge used: the conflicts are the
  trial's (src/db.rs six hunks, tests/fuzz.rs one, CLAUDE.md one). Nothing more upstream.
- A text-clean merge that did not compile: upstream's `OrdIndex::head_bracket` (00745f1a) reads
  `self.top[..]`, a field this branch made a method (`top()`, the top level read at the first
  seek for a mapping by the piece, `top_late`). Resolved as `self.top()[..]`, which is also what
  makes it right under a cipher (a paged index has an empty `top_eager`).
- A text-clean merge that made a flake: both sides added tests in tests/settle.rs whose stores
  were named `copy-identity-rewrite` (upstream's `a_copys_identity_is_its_partitions_and_not_the_one_a_rewrite_left`,
  this branch's `claim_across_rewrite("copy-identity-rewrite", ..)`), one directory
  `supdb-next-settle-copy-identity-rewrite-<pid>` for two tests of one process: 15 runs of 15 of
  the binary red (one or both, at create, settle or the asserts). This branch's five renamed
  `copy-claim-rewrite*`; 15 runs of 15 green after. A grep of every tests/*.rs for a store name
  given twice found no other (durable.rs's `oo-pend-noord` is one test's create and cleanup).

## The identity shape (hunk 1-6 of src/db.rs)
Taken: upstream's representation whole (`identity: bool`, `identity_of: u64`,
`CachedBlock::identity_for`, `Reader::block_cache_identity`, the builds' `identity_of: src.seg.blob.id()`
/ `seg.blob.id()`), with this branch's semantics at the patch kept: `splice_run` takes the rank's
entry under any claim where `identity_of == table.blob || key_eq(entry key, key)`, the search
otherwise; `copy_entry_at` (the patch's hint) takes any claim as the patch does; `prefetch_block`
and `by_cut` take any claim (both sides already did); the walk's start takes the claim of its
own partition alone (`identity_for(src.seg.blob.id())`, both sides). `agrees` (checked only) holds
the stronger of the two: `!other.identity || self.identity_for(other.identity_of)`.
Why: (i) both sound by a check the code makes (upstream: a claim of another partition is never
taken; ours: it is taken only where the key compare at that entry proves it, a copy holding each
key once in order); (ii) the count decides between them -- T17 measured the search under a stale
claim at +274 an op on the land probe; to be confirmed against 7c62153e below.

## Gates over upstream's new code (B)
- Wall (tests/source.rs, couplings.txt): green, 74 keys, no raise (lists group).
- Workflows: red first (before.log: `KEYS: ${{ inputs.keys }}` on no list), answered by a
  CI_ENV entry beside ARMS with its reason. CI_ENV is a const in tests/source.rs, no list the
  lists group holds, so no raise. scripts/check.sh quick still the one definition: it runs
  `cd bench && sh scripts/check.sh quick`, which reads KEYS itself (upstream's bench/scripts/check.sh).
- Cost entries: red first (before.log: Reader::block_cache_identity, Reader::piece_bounds on
  no line). Both upstream's `#[doc(hidden)]` test seams ("A test's"), read only by tests
  (tests/db.rs, tests/settle.rs); given `-` and the test-seam reason, and the two raises
  0 -> 1 declared in scripts/raises.txt (lists group red before the declaration,
  before-lists.log; green after, lists2.log). Keys: Reader::block_cache_identity (c5491c59),
  Reader::piece_bounds (00745f1a).
- Door: no file/mapping/advice/sync/process-read call in upstream's added src lines outside the
  ordindex test module (Vec::as_slice, std::fs::remove_file in a test); the lint group's door
  run is the gate (below).
- G1-6: no call in upstream's added lines to a method G1-6 removed, no residency or advice by
  address. Upstream touched no plain form build ask or drop, so G1-6's strace gate is not run.

## Upstream's mechanisms under a cipher (C), from the code
- Bounds walk by heads (`Options::bounds_by_heads`, `block_bounds_of`, `OrdIndex::head_bracket`):
  reads the ordered index's top level (in memory; under a cipher `top()` -> `top_late`, the
  level read by the piece at the first use) and, only where a source key falls inside the
  bracket, the boundary record by rank through the caller's `key_at` (`raw_key_into`, a
  record by the piece under a cipher). Runs right under a cipher once `top()` was used; as
  merged by text it would not compile, and with the field it reads (`top_eager`) it would
  panic out of bounds under a cipher (mutant M6, killed by the new ordindex test
  `a_heads_bracket_by_the_piece_is_the_mapped_ones`).
- Landing leaves bounds nobody reads (`Options::land_bounds_read`): reads two atomics
  (`writer_forms_off`, `live_handles`); no bytes. Same under a cipher.
- Quiet third merge (`merge_parallel_quiet`, `Shared::write_quiet`, `merges_room`): atomics and
  the gate (`zgate == GATE_Z`); no bytes. Under a cipher the compact form is not installed by
  the controller by default (`zpages_under_cipher` off), so with `merge_quiet_any_gate` off the
  third merge does not run under a cipher by default: it runs right where it runs; it is a
  default upstream priced on the plain store only.
- Cursor steps the fence (zfile `ZCursor`, `memo.cursor_steps`): the fence is `ZFence`, in
  memory; the page is loaded by `load_in`, through the form page by `slice_at`. Runs right.
- Sparse stride walk steps a key by its field (`step_key(.., direct)`, `render_field`): writes
  the key from the codec's field in registers; reads no bytes. Runs right.
- Sparse form's ask at the build as an arm (aa1f2cba): an option on the stride build path,
  which under a cipher builds through the paged mapping as the stride form did.
- New counters (ZCounts fields): atomics.

## The differential fuzz beyond the gate (gate range 0..4); logs M1/fuzz/*.log, each with EXIT
Seeds 1000..3000 (two thousand), eight processes, two a core, binary target/checked/deps/fuzz-b95676fc811c3756:
- cipher: ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384 ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1: FAILURES 0 in all eight.
- clear: ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1: FAILURES 0 in all eight.
Seeds 1000..2000 with the compact form forced (ZFUZZ_ZPAGES=1), cipher and clear: FAILURES 0.
Seeds 1000..2000 with ZFUZZ_ZPAGES=1 ZFUZZ_FORMSOFF=1, cipher and clear: FAILURES 0.
Upstream's options are drawn under the cipher too, by the summary counters (sum over the runs):
bounds by head cipher 33860 / clear 34684 (plain runs); quiet merges 8 / 9; cursor steps (form
forced) 3144 / 3184; landings left (forms-off seam) 18 / 16; stale exits 1087 / 1088.
`heads owned` is 0 under the cipher in every run and 12,425 in the clear: pre-existing (the
heads copy is a mapped file's), not this merge's.

## Groups run on the merged tree (logs under M1/)
- whole suite `cargo test --profile checked --no-fail-fast` (after.log): EXIT 0, 1545 passed, 0 failed, 7 ignored.
- stride group as check.sh runs it (stride.log): EXIT 0. (The ordindex unit test was added
  while it ran; a cfg(test) addition, run on its own after.)
- lint group (lint.log): EXIT 0 after `sh scripts/fmt.sh` reformatted one line of the
  resolution (`by_cut`); the door's red check and run green ("door: ok").
- wasm group (wasm.log): EXIT 0.
- lists group, SUPDB_BASE=HEAD (b1e9a809, the branch tip a push compares against): red before the
  raises were declared (before-lists.log), green after (lists2.log).
- bench group: RED first (bench-red.log): `docs/figures is not what runs/ draws`. Upstream's
  head leaves docs/figures behind its own rows: 7c62153e and the two commits before it bank ooc
  rows (a new class, b1024m) with no redraw, and docs/figures, bench/runs and
  bench/src/figures.rs are upstream's to the byte in the merged tree, so the check is red on
  upstream's tree by its own inputs (inferred from that identity; upstream's head was not built
  to see it). DEPARTURE from "docs/figures taken as it is": redrawn with
  `sh bench/scripts/figures.sh`, the check's own remedy (a function of the rows), `--check`
  green, bench group EXIT 0 after (bench.log).
- Roster opened once at the smallest rung, one rep, nothing else running (D): `bench run
  --scale quick --top 10000 --bottom 10000 --reps 1 --out M1/roster/out` (the 13 ARMS), and
  `--arms` with the bounded roster's arms not in ARMS and every cipher arm (supdb-cache256tofit,
  lmdb-nordahead, rocksdb-tuned-lz4, supdb-cipher, supdb-cipher64, supdb-cipherauto,
  supdb-cipherlog, supdb-ingestcipher, supdb-ingestcipherauto, supdb-ingestcipherlog) into
  M1/roster/out2: every arm opened and ran, EXIT 0 both, no refusal, failure or kill in the
  logs. No rate read or kept; the rows are under the change's directory, not bench/runs.

## Defaults upstream changed that the cipher arms inherit unpriced (not this change's to decide)
96481143 made `bounds_by_heads`, `land_bounds_read` and `merge_parallel_quiet: 3` defaults on
plain-store pairings. Owed, for whoever prices the cipher's cells (S5): the cipher arm against
the same arm with the options off. No such cipher arm exists; the plain arms are
`supdb-boundsold` (both bounds options off) and `supdb-quiet0`. The pairings owed:
`bench/scripts/pair.sh supdb-cipher <supdb-cipher with bounds_by_heads and land_bounds_read off> 300000`
and `... <supdb-cipher with merge_parallel_quiet 0> 300000`, and the same with `--bound` at ten
million keys for the bounds options. The quiet third merge is kept to where the compact form is
installed (`merge_quiet_any_gate` off), and under a cipher the controller does not install it
(`zpages_under_cipher` off), so by default the third merge runs under a cipher only in
`supdb-cipherauto` / `ZPages::Always`; bounds-by-heads does run under a cipher by default
(the fuzz's `bounds by head` counter, 33,860 under the cipher).

## Docs
- CLAUDE.md: the two identity entries made one ("It came a third time as a flag", under "A rank
  kept across the rewrite that moved it"), telling what the merged code does; and a new entry,
  "Two lines' tests merged by text, one store between them", the rule this merge taught.
- docs/engine.md: one clause, the patch by rank under a claim of another partition.

## The count (E): sh scripts/cost.sh 7c62153e, all fifteen probes (cost.log, EXIT 1, `cost: RED` by function)
Every probe at or under upstream's head by its total (an op): read -12.92, read_z -4.31, scan
-349.88, scan_long -6002.21, scan_z -570.92, scan_long_z -11478.95, load_durable -8.51,
load_buffered -6.82, commit_durable -8.04, commit_buffered -8.03, work -10.74, merge -200.71,
land -328.33, seal -1.03, scan_at -373.05. RED by function as T17's count against 9bfc4362 was
(143 gainers there, 147 here), the gate's slack being per function and this branch's reshape
of the read bracket and the walks being moves. gaincmp.py holds the gainers against T17's
reconciled list (scratch/plan/build/T17/r1/cost-whole-R1e.log); what is new or larger:
- land `Reader::settle_each` +5.13 (T17: a loser, -8.71): upstream's c5491c59 stopped the patch's
  hint (`copy_entry_at`) for a claim of another partition; this branch's hint took any claim.
  Answered: `copy_entry_at` takes upstream's shape (`identity_for(table.blob)`). Recount of
  land/scan/read (cost-r2.log): settle_each -9.84 under upstream, land -343.30 an op (from
  -328.33). The patch keeps the key compare: `CachedBlock::lower_bound` -193.37 and
  `Reader::splice_run` -90.26 an op against upstream's search under a stale claim -- the count
  that chose this branch's patch shape over upstream's (rule (ii)).
- `Reader::scan` +11.00 a scan (T17 +5.00; paired then with `scan_blocks` -16.8 and `raw_scan`
  +1, a move by inlining). The +6 more is this merge's: the merged tree against the branch tip
  (cost-vs-branch.log, `cost.sh b1e9a809`) reads Reader::scan +39 a scan where upstream's own
  9bfc4362 -> 7c62153e growth is +33 (216,011 -> 249,013 over 1,000 scans): upstream's
  `merge_parallel_quiet: 3` default makes `note_read` run past its first test on every scan
  (quiet_out). By instruction (ins/base.scan.ann against ins/head.scan.ann, --dump-instr over
  the two lag0_probe builds): the same body, with register allocation around the inlined
  note_read spilling more in this branch's larger scan (eight more `mov N(%rsp)` loads/stores a
  scan) and the bracket's `attn` test (four, T17's). [Round 1: removed, see below.] NOT REMOVED in round 0: no source shape was found that
  keeps note_read upstream's and takes the spills away; the probe's total is -349.88 under
  upstream. Reported as a cost of the merge, for G1-7 or whoever next counts the scan.
- land `Reader::cut_raw` +0.547 an op (T17 +0.297): +2 instructions a call over the branch tip
  (12,500 calls), source unchanged; by instruction the inlined `BuildCtx::cut_at` reads Seg at
  other offsets (this branch's Seg is larger) with one more register move. Codegen, not removed. [Round 1: wrong reading, and removed; see below.]
- +1 an operation in `Reader::leave_looked` / `WriterOp::drop` on every probe: upstream's own
  bracket grew by one (its `Reader::leave` 51 -> 52 a scan), mirrored where this branch's bracket
  stands (the 55c3e526 move), level with upstream's growth.
- `MmapBytes::resident_share` +0.130 an op in the loads: G1-6's rename, its loser
  `db::resident_share` -0.130 in the same listing (a move).
- commit probes `Db::start_keeper` +200 and `Options::clone` +72 over the run: once a store
  (a store's open), the owner's rule's exemption; `Options::clone` larger by this branch's
  options. Not reconciled further.
- scan_long_z / scan_z `ZCursor::load_in` (new) against `ZCursor::load` (lost): upstream's
  267f0199 split, a move (against the branch tip +220 / -195 a scan in scan_z; against upstream,
  the listing pairs them).
scripts/cost-hot.txt: no line names a function upstream renamed or removed (the run reached
every line but the five it already reached none of: flatindex::lookup, parse_record,
Blob::seek, Db::put, Db::delete). No line changed.
Instruments' dirs: M1/costw/out, M1/costb/out, M1/costr2/out (callgrind outputs; base worktrees
and targets removed), M1/ins (annotated listings).

## A flake met on the way, fixed
The whole suite after the hint change (after2.log, EXIT 101) failed one test,
tests/durable.rs `a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken`,
background arm: "the commit after that answers Ok: a test failed this landing". Alone five times
green (durable-rerun.log). The test read the seal queue's stop by its flag and then took the
first commit's answer as the place the segment work's error falls; the segment work sets the
flag and leaves its error for the writer's next commit after its look, so in that run the error
fell to the commit after the run's close (CLAUDE.md's "a test that waits on one signal and reads
another"). The test now answers the error first, with commits of nothing staged until one
answers it (its words held to the cause), and the run's batch's commit is held to `Ok`. Ten runs
of the durable binary green after (durable-fix.log). Not red on demand: the window is the
segment work's look, which no seam holds, so there is no before.log for it; the one red is the
whole-suite run that found it.

## Final state (21 Nov ~22:50 UTC)
Merge in progress (.git/MERGE_HEAD = 7c62153e), nothing staged; the commit step's `git add -A`
takes the resolved tree. Whole suite after every change (after3.log): EXIT 0, 1547 passed,
0 failed, 7 ignored. Lint (lint2.log) EXIT 0. Stride, wasm, bench, lists green earlier (the
later changes: the hint's shape in src/db.rs, docs, CLAUDE.md, a durable test).
Finished copies: keep/ (SHA256SUMS). Scratch targets removed (tbkeep, the cost worktrees);
outputs kept in costw/out, costb/out, costr2/out, ins/.

## Departures
- docs/figures redrawn (upstream's head is behind its own rows): the bench group's figures check.
- copy_entry_at takes upstream's hint shape where this branch's took any claim: by the count.
- tests/durable.rs changed for a flake met on the way (not a file either side touched).
- A unit test of `CachedBlock::identity_for` added (src/db.rs `copy_claim`) to hold the predicate
  the walk's start rests on, which no store test reaches (M4).
- The full gate (F) is the commit step's and was not run as one command here; each group ran.

## Not verified
- That upstream's head alone is red on the figures check (inferred from byte identity of runs/,
  docs/figures and figures.rs; not built).
- The Reader::scan +6 and cut_raw +2 a call of codegen are understood to the instruction and not
  removed.
- Cipher-arm pricing of upstream's new defaults (no timing taken by rule).
- The durable flake's fix has no red-before input (window held by no seam).

## Repair round 1 (22 Nov, from ~22:20 UTC): step log
- [x] CLAUDE.md's pointer to the folded entry fixed (the cipher section's seed-1179 sentence).
- [x] Functions both sides touched: both/ (both.py, union.py), list and check below.
- [x] whole3.py (exact per-line sum) over every file either side changed: whole3.log.
- [x] Count: upstream's head built once for instruction listings (M1/upbase, target M1/upbase-tb,
      4.3 GB, removed after the count); listings under M1/ins2. Shapes: ModeNote a cell a field
      (src/db.rs), the heads' search over the held slice (src/ordindex.rs), with_policy set in
      place (bench/src/engines.rs). Recount cost-r3.log (below). After the count only cfg(test)
      code changed under src/ (the copy_claim and ordindex tests), and tests/ and the bench's
      test module: no release build differs from the counted one (mutcount.py's clean runs
      rebuilt the probes from the final tree).
- [x] Extra mutants and this round's own: mutants.md (repair round 1), mutants-r1.results.
- [x] Tests added: ordindex `a_heads_bracket_is_refused_under_a_prefix_learned_short`, db
      `copy_claim::a_copy_agrees_with_a_build_only_where_it_claims_the_builds_partition`,
      tests/db.rs `a_quiet_stretch_under_the_idle_seals_files_no_backlog`, bench
      `engines::tests::the_cipher_arms_differ_from_supdb_cipher_in_their_one_option`; each red
      on its mutant (X8, X3b, O4b, O6) before it was kept.
- [x] Whole suite after-r1.log (one red, below), lint-r1.log, bench-r1.log, wasm-r1.log green;
      fuzz beyond the gate again for this round's engine changes: fuzz-r1/.

## Functions both sides touched (brief A), each merged body read once
Found by M1/both/both.py: each side's `git diff -U0 9bfc4362 <side>` hunk mapped to the base
function whose body (a `fn` line and its braces) holds the changed lines, intersected; held
against the hunk headers git's rust driver prints (`diff=rust`), which name the same set (plus
`OrdIndex::whole_key_at` in ordindex.rs, a header that names the function before upstream's
inserted `head_bracket`, which only upstream touched). For each, M1/both/union.py holds the
merged body to the base plus both sides' changes exactly (the +/- lines from upstream's body to
the merged one are the base-to-branch ones, and from the branch's to the merged one the
base-to-upstream ones): UNION where that holds. Outputs: both/db.union, eng.union, u.fuzz.rs,
u.db.rs, u.run.rs. Files with no function both touched: zfile.rs (12 upstream, 32 branch),
zpages.rs, ordindex.rs, blob.rs, bytes.rs, flatindex.rs, vault.rs, pager.rs, zselect.rs,
tests/fences.rs, tests/settle.rs, tests/zpages.rs.
src/db.rs:
- `Options::default`: UNION. Upstream's seven defaults (zpages_cursor_steps, zpages_stride_step_direct,
  merge_parallel_quiet 3, merge_quiet_any_gate, bounds_by_heads, land_bounds_read,
  zpages_stride_sparse_waits) beside the branch's nine (encryption, run_spare, six open seams,
  zpages_under_cipher); no field twice (the compiler holds that).
- `Slot::new`: UNION (upstream `zsteps`, branch `page`). `SealCounts::waits`: UNION (upstream
  `merges_quiet`, branch `run_spares`, `wal_set_aside`, `stale_deletes`).
- `Db::create`, `Db::open`: UNION; upstream's two lines each are `Shared`'s `writer_forms_off`
  and `write_quiet`, beside the branch's vault and open work, nothing between them.
- `Maint::start_compact`: UNION; upstream's `merges_room()` and its `merges_quiet` count beside
  the branch's `vault::len` and the landing hold's WAITING bit; read with `merges_room` (atomics
  and the gate, no file).
- `copy_entry_at`, `Reader::splice_run`, `Reader::scan_blocks`, `BuildCtx::copy_block`,
  `BuildCtx::copy_from_deltas`: the identity resolution, read against both commits. copy_block,
  copy_from_deltas, scan_blocks' `at_rank` and copy_entry_at are upstream's bodies to the line
  (upstream->merged empty); splice_run is upstream's `&& blk.identity` with the branch's
  `(blk.identity_of == table.blob || key_eq(..))` after it (the shape the count chose).
bench/src/engines.rs: `Policy::default`, `Supdb::with_policy`, `Engine for Supdb::name`,
`counters`, `guarantee`, `open`: UNION; upstream's are its nine new arms, their policy fields and
its new counters, the branch's the cipher arms and theirs; `with_policy`'s destructure takes
every field of both (the compiler holds that a field left out is an error).
bench/src/run.rs `one_pass`: UNION (upstream's `SUPDB_SCAN_COUNTERS` lines and the mixed scans'
fold, the branch's five lines elsewhere).
tests/db.rs `a_second_open_of_a_store_is_refused_until_the_first_is_gone`: UNION (upstream's
`zpages_settle` before the listings, the branch's lock lines).
tests/fuzz.rs `pick_cfg`, `opts`, `run_seed_inner`: UNION. `fuzz_range`: the branch moved its
body into `run_range` (for the cipher's ranges); upstream's eight changed lines (the point
array to twelve, the summary's four counters) are in the merged `run_range` exactly as the
branch's run_range plus them (the conflict hunk, resolved by hand: every counter of both).

## Nothing lost, line by line (brief A): whole3.py (whole3.log), replacing whole.py
whole.py's test was a heuristic of its own and misread a side that removed copies of a line
elsewhere as a line lost (its tests/db.rs MISSING `db.commit().unwrap();` reads, exactly:
base 569, upstream 576, branch 568, merged 575 = 569 + 7 - 1; and src/db.rs's
`let _entered = self.enter();` is in upstream's `piece_bounds` at the merged file's line of
that function). whole3.py holds every non-blank line of every file either side changed (84,
bench/runs, docs/figures and scratch/ aside) to merged == base + (upstream - base) + (branch -
base). 74 files hold exactly. The ten that do not are the ten under keep/ (the files the
resolution wrote by hand: CLAUDE.md, docs/engine.md, scripts/raises.txt, src/db.rs,
src/ordindex.rs, tests/data/cost-entries.txt, tests/durable.rs, tests/fuzz.rs,
tests/settle.rs, tests/source.rs), and every line off the sum in them is a line of the
resolution, read: the identity shape's lines and docs (db.rs), the `copy_claim` unit test, the
`top()` in `head_bracket` and the new ordindex test, the summary line, the renamed settle
stores, the KEYS entry, the two cost entries and their raises, the durable test's loop, and
CLAUDE.md's folded entry, its pointer and the new rule (plus counts of `}`, `0`, `#[test]` and
the like those added lines carry). whole3.log was taken before this round's source shapes and tests; they add their own lines to src/db.rs, src/ordindex.rs and tests/db.rs, and bench/src/engines.rs to the files off the sum, each under keep/.

## The count again, repair round 1 (cost-r3.log, EXIT 1 `cost: RED` by function as before)
`COST_DIR=M1/costr3 COST_BASE_TARGET=M1/upbase-tb sh scripts/cost.sh 7c62153e...` over all
fifteen probes, after the three source shapes below. Every probe under upstream by its total:
read -38.15 (was -12.92), read_z -31.07 (-4.31), scan -381.88 (-349.88), scan_long -6034.79,
scan_z -602.92, scan_long_z -11547.85, load_durable -10.18, load_buffered -4.38, commit_durable
-8.06, commit_buffered -8.05, work -23.44 (-10.74), merge -214.51 (-200.71), land -344.02
(-328.33), seal -1.03, scan_at -373.05. The instrument for each shape: upstream's head built once
(M1/upbase, target M1/upbase-tb) and the probes run under `--dump-instr=yes` (M1/ins2/run.sh),
each function's instructions laid to the source line of their innermost inline frame by the
text of that line in each side's tree (M1/ins2/bytext.py, from T17's perins.py), base against head.

(a) `Reader::scan` +11.002 a scan -> -20.998 (249,013 -> 228,015), in all four scan probes (scan_z
-18.998, scan_long_z -19.049). By line, base against the tree before the shape (ins2: the head
binary of the tree at the round's start): the read bracket's tests (the handle's `attn` and the
gate, the end's `attn_open` look, `leave_looked`'s call) 13 against upstream's 8, which is
T17's move with `Reader::leave` (-52) and `leave_looked` (+36); `note_read`'s first test +2
(`if !(seal_idle || merge_parallel_quiet > 0)`, 4 -> 6); and around the call sites +2 here, -2
there (`fs()`'s match, `held.is_null`, `scan_blocks`' call -4, `option.rs`, `cell.rs`, `mod.rs`),
the spills the notes named. Shapes tried: (1) the reviewer's, `note_read` past its first test
cold and out of line, not built: under `merge_parallel_quiet: 3` (upstream's default) the
writer's every scan runs past the first test (`m.scans += 1`, the `quiet_out` test), so the
helper would be a call a scan, the same work under another name plus the call -- a gain moved,
not removed. (2) Built: `ModeNote` a cell a field (src/db.rs). The note was one `Cell<ModeNote>`
copied in whole at every read and out whole at its end: upstream's base reads 17 instructions
of `Cell::get` and 15 of `Cell::set` a scan under `note_read` (ins2/base.scan.agg2), for a read
that bumps one counter and looks at one flag. A cell a field, the writer's read loads and
stores its counter and loads `quiet_out`, and the scan reads 21 under upstream (the four
scan probes 19 to 21, head1.scan.ins against base.scan.ins: the `Cell::get`/`set` lines gone,
`note_read`'s two tests 7 a scan). The same shape takes 38 an op off the point read (`Reader::read_all_in`'s closure)
and the writer's `Db::note_read_mode` from 5,402 to 3,001 over the commit probes. Semantics
the same: every field read and written where it was, `quiet` and `quiet_out` set together at
the commit's look as before (held by `a_third_merge_runs_only_in_a_stretch_of_writes_nobody_reads`
and `a_commit_seals_whenever_the_sealer_is_idle`, mutants O3-O5).
(b) `Reader::cut_raw` +0.547 an op -> under upstream (2,325,287 -> 2,308,218, -1.37 a call over
12,500 calls; it is on no gainer line of cost-r3.log). By line (ins2, base against the tree at the
round's start): the source of `cut_raw` and `BuildCtx::cut_at` is the same on all three trees;
the gain was in `OrdIndex::seek_in_exact`'s binary search over the heads, inlined into it, a
`head()` a probe: upstream's head asserts `i < n`, this branch's tests `i >= lim` and calls
`head_absent` (the read by the piece) behind it, a call that returns, so 15.2 instructions a
call at that line against upstream's 12.2 (190,546 against 153,046), with `Some(p) => p` and the
`uint_macros` lines one more each. Shapes: (1) `held[lo..hi].partition_point(..)` over
`heads_held()`: 2,463,600, worse (std's binary search is the longer loop for windows of a block);
(2) built: the same loop over `heads_held()` read unchecked, the window bounded once
(`hi <= held.len()`), and the head-at-a-time loop for a mapping read by the piece, which holds
none in memory: 2,308,218. No test of `seek_in_exact` changed; `cut.base-head3.bytext` has the
listing. The settle's seeks (`seek_in`) take the same loop.
(c) `BuildCtx::copy_block` +29.589 an op: a move, named: `BuildCtx::emit_over` -40.435 an op in the
same listing (4,043,469 -> 0), inlined into copy_block on this branch since T17 (its call line
gone); net copy_block + emit_over -10.85 an op. The 85,899 over T17's figure: 85,118 is upstream's
own, its copy_block 2,708,562 at 9bfc4362 and 2,623,444 at 7c62153e with emit_over unchanged at
4,043,469 (its source changed by the one `identity_of` line, so the compiler's, the base side
alone); the 781 left is the merged copy_block against the branch tip (cost-vs-branch.log,
5,581,594 -> 5,582,375), upstream's `identity_of: src.seg.blob.id()` store at every build, the
line upstream's copy_block has itself (the merged body is upstream's to the line).
(d) `supdb_bench::engines::Supdb::with_policy` +9,089 over each load (was +9,354): once a store,
the suite's own open (bench/src, each side's), not the engine's and not per operation. By
instruction (ins2 head.wp.ins against base.wp.ins): 13,750 distinct instructions each executed
once against upstream's 4,705, the `Options { .. }` literal's lines 9,564 against 1,284, and
`drop_glue::<supdb::db::Options>` only in the head's listing: this branch's `Options` holds
`Encryption`, whose `Cipher` holds `Arc`s, so the struct update `..Default::default()` the
literal is built by is materialized and moved field by field. It predates the merge (G1-3 and
T17 recorded +8,289); the 800 more is upstream's new fields in that literal. One shape built,
the two struct updates the branch added (`let opts = if cipher { Options { .., ..opts } }`, then
`Options { encryption, ..opts }`) set in place instead: 265 fewer, the rest the literal's.
Removing the rest would mean rewriting upstream's literal of several hundred fields as
assignments, a change to the suite's code of its own, and is not this merge's; reported here
with its count and cause.
(e) Everything else this count gains is T17's reconciled list at T17's count to 64 over the run
(`gaincmp.py` against scratch/plan/build/T17/r1/cost-whole-R1e.log), but: `Db::start_keeper` +200
and `Options::clone` +72 in the commit probes and +96 in land (once a store, `Options` larger by
both sides' fields); `WriterOp::drop` and `Reader::leave_looked` +1 an operation (upstream's own
`Reader::leave` grew 51 -> 52, mirrored where this branch's bracket stands, the 55c3e526 move);
`MmapBytes::resident_share` +0.130 in the loads (G1-6's rename of `db::resident_share`, -0.130 in the
same listing); `Maint::join_seal` +0.034 against its `~` bound of 0.010 (T17: +0.027, a wait on the
seal thread, the line's own reason). cost-hot.txt: unchanged; the same five lines reached by no
probe.

## A red met in round 1's whole suite, in a test upstream added, not reproduced
after-r1.log (EXIT 101): tests/db.rs `a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie`
(upstream 00745f1a's), at its last assertion, near true: "the two walks' boundaries ...
[(229, 234), (0, 926)], left 463 right 926" (bounds-flake-red.txt) -- the store walked with the
option off read every boundary twice, 926 being twice the 463 the other store placed and read;
the bounds the two walks made were equal (the assertion before it passed). Every other test of
the suite passed (1,549 passed, 1 failed, 7 ignored). Run again: the test alone five times (bounds-alone-*.log) green;
eight copies of it at once, three times (24) green; the merged db binary whole five times alone
green and three at once beside three more processes green; and upstream's head alone, its db
binary built in a scratch worktree with a target of its own (391 MB, removed), by turns with the
merged binary, five whole runs each (alt.log): ten green. So one red in fourteen whole runs of the
merged binary and none in five of upstream's. The counter is the store's (`Shared::asks`,
`block_bounds_of` counts at each walk), and a second whole walk of the store's partitions in that
store means a piece's bounds were walked again where they stood cached (`IdCache::get` before
every walk, at the landing and at a table): either two walkers missed together (the cache's doc
allows it: "two threads that miss together both compute and both insert") or a second piece over
every partition. Neither is read off the code with certainty, and nothing in this round's changes
(`ModeNote`, the heads' search) decides how many walks are made. NOT FIXED and not accepted: it
is named here for the next look with the one red output it has; a test that compares two stores'
walk counts by equality is fragile to either cause, and the fix wants the race made wide first.

## Round 1: the reviewers' items, each
1. The count leaves gains neither removed nor named (Reader::scan +11, cut_raw +0.547,
   copy_block +29.589, with_policy): Reader::scan and cut_raw REMOVED by source shapes, each
   counted (the count again, (a) and (b)); copy_block a MOVE named with its partner
   (`BuildCtx::emit_over` -40.435 in the same listing) and its 85,899 over T17 split to
   upstream's own copy_block (85,118) and the 781 of upstream's line ((c)); with_policy reported
   with its count and cause as the branch's own, predating the merge, one shape built ((d)).
2. CLAUDE.md's pointer to the folded entry: fixed (the cipher section now names "the third
   coming of the entry 'a rank kept across the rewrite that moved it'").
3./5. The list of functions both sides touched, each merged body read: the section "Functions
   both sides touched" above, from the two diffs' hunks and held to git's rust hunk headers; and
   whole.py replaced by whole3.py, rerun on the final tree, every residual explained.
4. The two per-operation gains: as 1; each shape tried is in (a) and (b) with its count (the
   out-of-line note_read reasoned and not built, the partition_point search built and counted
   worse, the two kept shapes counted under upstream).
Extra mutants: all eleven run (two of them the same mutant twice, X3 and X6, run once each with
its pair), table in mutants.md; survivors answered by a test (X3 -> X3b, X8, O4 -> O4b, O6) or by
the count (X1, O5), and X6 by its pair X6e/X6eb.

## Departures (round 1)
- `Reader::note_read`'s `ModeNote` and `OrdIndex::seek_in_exact`'s search over the heads are
  upstream's code reshaped by this merge, for the count (rule 1); behaviour the same, held by the
  tests the mutants name.
- bench/src/engines.rs `with_policy` set in place, and a bench test of the cipher arms' options
  (O6 had no test before).
- A red in an upstream test met in the round's whole suite, not reproduced in fourteen runs and
  not fixed (the section above).
- The fuzz beyond the gate for this round's engine changes ran seeds 3000..4000 in each mode
  (eight processes, two a core), not the two thousand of round 0.

## Not verified (round 1)
- Why `a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie` read a
  second whole walk once; whether upstream's head has the same red (five whole runs green there
  says nothing at a rate of one in fourteen).
- `with_policy`'s remaining +8,824 over upstream is laid to the `Options` literal's field moves
  by the listing's line counts and `drop_glue::<Options>`; the literal rewritten as assignments
  was not built to confirm it.
- The O5 and X1 counts are of one build each way in mutcount.py's environment, not cost.sh's;
  both sides of each pair ran in the same environment.

## The stride group in round 1: two reds, each once, then green whole
- stride-r1.log (EXIT 101): tests/cipher.rs `the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit`,
  "the builds gave nothing back" (cipherflake-red.txt has the line: PagerCounts budget 2,097,152,
  peak 3,847,092, evicted_units 690), the undiagnosed red the launch names. Run again alone five
  times under the group's `SUPDB_TEST_STRIDE=1`: green (cipherflake-alone-1.log kept, the four
  after it the same and removed). The group stopped at that step, so it was run again.
- stride-r1b.log (EXIT 101): every test of the first step green (1,553), then the group's last
  fuzz step, `ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1` `concurrent_range` seeds 0..4, failed its
  steer check, "no partition left the stride form in seeds 0..4" (every seed `moved in 0 out 0`):
  a check both sides have had since 69b512fc, on a conversion the controller's clock times. The
  same step alone eight times: green.
- stride-r1c.log: the group whole, EXIT 0, `all checks passed: stride` (1,554 passed, 0 failed).
Neither test is one this round's diff names; the second is not on the launch's list, and is
recorded here as met once with its output (stride-r1b.log lines 1751-1763).

## Final state, round 1 (22 Nov ~02:30 UTC)
Merge in progress as before (.git/MERGE_HEAD 7c62153e, nothing staged by this round). The
finished copies of every file this change edited by hand are keep/ (SHA256SUMS, checked
against the tree). Logs read by their EXIT lines: cost-r3.log (EXIT 1, every probe under
upstream; the gainers answered above), after-r1.log (one red, above), lint-r1.log, bench-r1.log,
wasm-r1.log, lists-r1.log, stride-r1c.log (EXIT 0), fuzz-r1/{cipher,clear}.log (EXIT 0, FAILURES 0 in
all sixteen ranges). Scratch removed: M1/upbase and M1/upbase-tb (4.3 GB), M1/upwt and its target
(391 MB), the probes' binary copies, the test TMPDIR. Nothing of this round's left running.

## Repair round 2 (22 Nov, from ~01:00 UTC): step log
- [x] Bounds flake: instrumented, cause found, red on demand, red on upstream's head, fixed (below).
- [x] Stride steer flake: cause found, red under load before, red on upstream's head, fixed (below).
- [x] Extra mutants (mutants_r2.py -> mutants-r2.results; counts mutcount2.py -> mutants-r2.counts).
- [x] Whole suite, stride group, lint, bench, lists again (after-r2.log, stride-r2.log, lint-r2.log, bench-r2.log, lists-r2.log).

### Round 2: the bounds flake (blocking items 1 and 3), found, red on demand, fixed
Instrument (scratch, not kept): an eprintln in `block_bounds_of` under `SUPDB_BW_TRACE`, with
the walk's thread, partition, length, counts and the top four `supdb::` frames of a forced
backtrace, and markers in the test at each store's start, before round 0, before the seal and at
the count (bw-green.txt, bw-inline.txt, the sum by caller bwsum.py). src/db.rs and tests/db.rs put
back from keep/ by their sums after it. What it read, near reads, in every store: TWO walks of
the same keys against the same 29 partitions -- the upkeep thread's fill after round 0's commit,
walking the burst's unsealed snapshot (`Reader::fill_forms` -> `complete_forms` ->
`BlockTable::snap_at` -> `BuildCtx::snap_bounds`), and the landing's walk of the sealed piece
(`land_readable` -> `rank_before_publish` -> `bound_pieces_of`). On 229/234 a walk, off 0/463:
green is (458, 468) against 926. The red's [(229, 234), (0, 926)] is the store with the option on
at ONE walk: its seal froze the burst before the lent fill ran (the off store's second walk was
not extra; the on store's was missing). Neither of the reviewers' candidates: no two walkers
missed the IdCache together (the landing puts before it publishes, so no reader can miss the
piece's bounds) and no rank_before_publish ran again (one landing walk a pair in every trace).
Red on demand: r2/bounds-wide.py (the upkeep thread held before its pass, `hold_upkeep_pass`, in
the store with the option on, across round 0's commit and the seal): r2/bounds-wide-before.log,
red 5 of 5 with exactly the red's counts. The same script on upstream's head 7c62153e, built in
a scratch worktree with a target of its own (391 MB, removed): r2/bounds-wide-upstream.log, red
5 of 5, the same counts. UPSTREAM'S: the test's premise (both stores make the same walks) rests
on a race between the lent fill and the seal's freeze.
Fix (tests/db.rs only; the engine is right): the test's stores fill on the writer's thread
(`upkeep: Upkeep::Inline`, with the reason at the option), and the counts held are the walks
since the burst was filed (`c - before`, `before` taken after the commit's fill). Each half is
needed: BM1 (Inline taken out, the race made wide) red 5/5; BM2 (the deltas taken out) and BM3
(`before` moved above the commit) red 3/3 under r2's `filldiff` (`commit_forms_build: !on`, the
fill made to differ between the stores, a stand-in for the fills' room closing in one); the fix
under filldiff green 3/3 (r2/tmut-*.log). After: r2/bounds-wide-after.log, the wide race on the
fixed test green 5/5 (the hold is a no-op with no thread); r2/bounds-load.log, 24 of 24 green
eight copies at once three rounds.

### Round 2: the stride steer flake (blocking item 2), found, red under load, fixed
`run_concurrent` asks a conversion every 53 writer steps (`Db::zpages_convert_ask`, a word the next
ask replaces) and the segment work takes an ask only at its turn with the builder free and nothing
due (`Maint::z_convert_asked`, from `z_step` and the policy's look). Beside the seals' and merges'
builds a seed's asks can all be replaced untaken, and the range's check (`no partition left the
stride form in seeds 0..4`) is then red over stores that were never asked: the green runs alone
already read most seeds at `moved in 0 out 0` (seed 0 alone moving, 1-6 out). Not the controller's
clock (HANDOFF's class): the seam, not the policy, and no clock in it. Red before under load:
r2/convert-load-before.log, the round-1 test (r2/pre/fuzz.rs) six copies at once, four rounds: 13
of 24 red with that line; and on upstream's head 7c62153e the same: r2/convert-load-upstream.log, 16
of 24 red (UPSTREAM'S). Red on demand: r2/fmut.py's `noasks` (the writer's timed asks taken out,
the race lost for certain): the round-1 test red 3/3 (r2/fmut-before-noasks.log). Fix (tests/fuzz.rs
only): after the writer's last settle, with the readers still running, the served partitions are
asked by turns, each ask followed by `zpages_settle` (which takes it: `z_step` under `z_settling`
calls `z_convert_asked` once nothing is due), until `stride_converts_back` moves, at most 64 asks,
an error naming the seed past them. After: noasks green 3/3 (r2/fmut-after-noasks.log); the stride
step six copies at once, four rounds, 24 of 24 green (r2/convert-load.log); every seed moved out at
least once in three runs alone. Mutants of it: FM1 (the settled asks taken out), FM2 (the loop's
test inverted), FM3 (the asks not settled) each red under noasks (r2/fmut-fm*.log). The random asks
stay, beside seals and merges, as the seam is for.

### Round 2: the extra mutants (mutants_r2.py -> mutants-r2.results; counts mutcount2.py -> mutants-r2.counts, r2/counts.log)
In mutants.md, round 2. Two tests added in bench/src/engines.rs for survivors: the arms' encryption
pulled into `encryption_of` (the match with_policy had inline, the same body), held by
`engines::tests::the_dirkey_arm_asks_the_kernel_and_the_cipher_arms_a_cipher`, which also opens
`supdb-cipher64` and `supdb-cipher` and holds the first's cache budget to its policy's 64 MiB.
bench/src code only; not the engine, so the cost rows are not counted again (with_policy is once a
store, the suite's open; encryption_of its body moved out).

### Round 2: what changed under src/
Nothing: src/db.rs and src/ordindex.rs are round 1's to the byte (keep/ sums), the instrument
taken out. No engine row is counted again.

### Round 2: groups on the final tree (each read by its EXIT line)
- after-r2.log: `cargo test --profile checked --no-fail-fast`, EXIT 0, 1550 passed, 0 failed, 7 ignored.
- stride-r2.log: `sh scripts/check.sh stride`, EXIT 0, `all checks passed: stride` (its concurrent
  convert step the fixed one).
- lint-r2.log EXIT 0; bench-r2.log EXIT 0 (first run red at the bench's own fmt check of the test I
  added, `sh bench/scripts/fmt.sh` applied, run again green; keep/ holds the formatted file);
  lists-r2.log (SUPDB_BASE=HEAD) EXIT 0. wasm not run again: no file under src/ changed this round.
- The CLAUDE.md entry this round taught: "Two counts compared across a worker's race, and a steer
  that asked and was never answered" (after "A note in one cell, copied whole to bump a counter").

### Final state, round 2 (22 Nov ~03:40 UTC)
Merge in progress as before (.git/MERGE_HEAD 7c62153e, nothing staged). Files this round edited:
tests/db.rs (the bounds test), tests/fuzz.rs (run_concurrent's settled asks), bench/src/engines.rs
(`encryption_of` and its test), CLAUDE.md (one entry); each under keep/ with SHA256SUMS (12 OK).
src/ unchanged from round 1. Scratch removed: M1/upwt worktree and its target (464 MB), the test
TMPDIR. Nothing of this round's left running.

### Departures (round 2)
- Both flakes are upstream's (red on upstream's head under the same seam or load) and fixed in the
  tests, not the engine: the engine's behaviour (a lent fill racing a seal; an ask replaced
  untaken) is what the code says it does.
- `encryption_of` pulled out of with_policy (bench/src) so the dirkey arm's encryption can be held
  by a test without an encrypting filesystem.

### Not verified (round 2)
- E2's cheaper shape (`hi < held.len()`, four instructions a call under the land probe's
  `cut_raw`) is counted and not taken: an engine change and a fifteen-probe recount, for G1-7.
- with_policy/encryption_of not counted again (bench code once a store; no engine code changed).
- The bounds test's `filldiff` stand-in (`commit_forms_build: !on`) stands for the fills' room
  closing in one store; the room's own gate was not driven (no seam for the group's refaults at
  the commit's fill).

## Repair round 3 (22 Nov, from ~02:15 UTC): step log
- [x] Blocking 1/2 (one item, two reviewers): the dirplain test's doc block moved back onto
  `the_dirplain_arms_differ_from_their_defaults_in_the_directory_hint_alone`, so the cipher-arms
  test's doc begins "The cipher arms are `supdb` under a cipher ...". Checked for any other doc
  split by the resolution with `M1/docs_of.py` (every fn's `///` block in the tree against HEAD's
  and MERGE_HEAD's over every .rs file either diff touches): before the fix it named the dirplain
  test; after it, the only difference is `src/db.rs` `copy_entry_at`, whose doc the identity
  resolution rewrote on purpose (round 1). `sh bench/scripts/fmt.sh` EXIT 0 (r3-fmt.log, r3-fmt2.log).
- [x] Extra mutants on bench code (mutants_r3.py -> mutants-r3.results, logs mutant-r3-*.log).
  Two changes for survivors: (a) with_policy's `opts.direct_ingest = opts.direct_ingest &&
  !cipher_log` was a check nothing reached (no Policy sets direct_ingest, so the operand is
  always the engine's default, true; extra mutant 7 equivalent): written `if cipher_log {
  opts.direct_ingest = false; }`, the same for every arm. (b) `0 => c` was held only by "not
  64 MiB" (B8, `c.cache_bytes(1 << 30)`, survived against round 2's test: B8b): the encryption
  test now opens a store under the same cipher with no bound beside `supdb-cipher` and holds the
  two budgets within a quarter (the default is read off the process's memory at each open, so not
  to the byte; on a host whose default is within a quarter of 1 GiB the mutant would be equivalent
  -- here the default is about 6 GiB). Bench tests green: r3-benchtest.log.
- [x] Blocking 3: the count over the probes that hold with_policy, after both bench edits:
  `COST_DIR=M1/costr4 sh scripts/cost.sh 7c62153e load_durable load_buffered work merge`
  (cost-r4.log, START 02:24:04, EXIT 1 `cost: RED` by function as in cost-r3). with_policy
  13,612 / 13,596 (round 1: 13,794 / 13,778, -182 each); `encryption_of` on no line of any probe
  (inlined). Totals all under upstream: load_durable -5.75 an op, load_buffered -8.24, work
  -24.72, merge -215.14. Every HOT GAINED line is cost-r3's to the instruction (gaincmp over
  the two logs) but with_policy -182, `WriterHelper::run` -52 and `Reader::abandon_look` +38 (the
  waits on another thread round 1 named). The base's worktree and target removed by cost.sh
  (costr4 16 MB of logs left). A first start of the count was killed when the B7 rewrite was
  chosen (it would have counted the shape before), its worktree removed by `git worktree remove`.
- [x] Extra mutants on the tests (r3/run.sh, r3/tmut.py; logs r3/*.log, each ending VERDICT and
  RESTORED by keep/'s sum):
  * bounds test, `before` from before round 0's commit, Inline kept (extra 1): green 24/24,
    eight copies at once three rounds (r3/r1-eight.log). Deterministic while both stores' commits
    fill; but whether a commit fills is the fills' room's, a gate on the group's refaults
    (`Options::forms_settle_room_pressure`, `refaults_moved`), and with the fill made to differ
    (round 2's `filldiff`, `commit_forms_build: !on`) the test counted from before the commit is
    red 2/2 with the original red's counts (r3/bt-new-filldiff.log) where the test as it stands
    is green 2/2 (r3/bt-narrow-filldiff.log). So the narrowing is NOT undone: it keeps a platform
    reading out of the count. A comment at `before` now says so. tests/db.rs is round 2's text
    plus that comment.
  * bounds test, `before` after `db.seal()` (extra 9): green 3/3 (r3/r9-alone.log). Equivalent by
    the store's order, not a hole: `seal()` hands the frozen table to the seal thread and returns
    (its doc), and the landing's walk (`land_readable` -> `rank_before_publish`) runs on the
    segment work's thread after the seal thread's readable phase, so it falls after a `before`
    read at the seal's return -- which the mutant's own green near-case assertion
    (`landed.land_bound_walks > before.land_bound_walks`) shows in every run. The deltas do hold
    the landing's walk.
  * run_concurrent `zpages_convert_ask(nth)` -> `(0)` (extras 2 and 5) and the bound `64` -> `1`
    (extra 6): both survived (noasks 3/3 each, six-way load 12/12, alone 6/6). Counted: the loop
    took exactly one ask in every one of 160 seeds (noasks, alone and loaded), the first served
    partition a stride file each time. So the turns and the bound were a check nothing reached.
    Rewritten (tests/fuzz.rs): the first served partition asked three times, each settled, and
    the moves held to `in + out == 3` with one each way at the least (`(1, 2)` or `(2, 1)`):
    out, back in (the range's keys are numbers of one width, `StrideNo::askable`), out. Green
    alone 6/6, noasks 3/3, six-way load 18/18 (r3/fix-*.log), every seed `moved in` >= 1 (most
    seeds of the range had moved in 0 before). Mutants of it: FA two asks (1,1), FB asks not
    settled (0,0), FC the check taking only one-way moves, FD (engine, src/db.rs) a block file
    never asked back into the stride form (0,1), FE the asks taken out under noasks ("no
    partition left the stride form in seeds 0..4"): each red 2/2.
- [x] CLAUDE.md: round 2's entry ("Two counts compared across a worker's race ...") says what the
  two tests do now and adds the rule round 3 taught (a retry loop is counted for its turns
  before its bound is believed; a gate on a platform reading stays outside a held count).

### Round 3: what changed under src/
Nothing. src/db.rs and src/ordindex.rs are round 1's to the byte (keep/ sums); FD's mutant put back
by its sum. No engine row is counted again; the bench rows that hold with_policy were (above).

### Round 3: groups on the final tree (each read by its EXIT line)
- r3-unit.log EXIT 0 (the bounds test; the fuzz binary built). r3-fuzz.log: `cargo test --profile
  checked --test fuzz`, EXIT 0, 5 passed (the gate's cipher fuzz ranges among them).
- r3-stride.log: `sh scripts/check.sh stride`, EXIT 0, `all checks passed: stride` (its
  concurrent convert step, seeds 0..4, the new three-ask shape; its whole suite under
  SUPDB_TEST_STRIDE green).
- r3-lint.log EXIT 0 `all checks passed: lint` (scripts/fmt.sh --check inside it; r3-fmtmain.log
  `sh scripts/fmt.sh` EXIT 0); r3-bench.log EXIT 0 `all checks passed: bench`; r3-lists.log
  (SUPDB_BASE=HEAD) EXIT 0 `all checks passed: lists`.
- The whole suite and wasm not run again: no file under src/ changed; the files changed (tests/
  fuzz.rs run_concurrent, a comment in tests/db.rs, bench/src/engines.rs, CLAUDE.md) are each
  covered by the groups above.

### Final state, round 3 (22 Nov ~03:30 UTC)
Merge in progress as before (.git/MERGE_HEAD 7c62153e, nothing staged by this round). Files this
round edited: bench/src/engines.rs (doc block, with_policy's cipher_log line, the encryption test's
budget check), tests/fuzz.rs (run_concurrent's settled asks), tests/db.rs (a comment at `before`),
CLAUDE.md (round 2's entry); each under keep/ with SHA256SUMS (12 OK, each tree file equal to its
copy). Scratch removed: the test TMPDIR (plan/build/tmp/M1), the killed first count's worktree and
directory, the count's base worktree and target (cost.sh). Nothing of this round's left running.

### Departures (round 3)
- Extra mutant 1's suggestion (undo the bounds test's narrowing) not taken, with the run that says
  why (r3/bt-new-filldiff.log).
- run_concurrent's loop by turns replaced by three asks of the first partition (the turns and the
  bound reached by nothing in 160 seeds); the new shape also holds a move into the stride form.
- with_policy's `&&` operand taken out (a check nothing reached), same behaviour for every arm.

### Not verified (round 3)
- The fills' room was not driven closed by real refaults in the bounds test (no seam for the
  group's refaults at the commit's fill); `commit_forms_build: !on` stands in, as in round 2.
- B8's kill depends on the host's default cache being more than a quarter away from 1 GiB (here
  about 6 GiB); on a host with about 1 GiB of budget the mutant is equivalent.
- The three-ask shape rests on the range's keys allowing the stride form (numbers of one width,
  by run_concurrent's construction under ZFUZZ_STRIDE); it was run over seeds 0..4 (and the gate's
  stride group), not over a wider seed range.
## Commit step (22 Nov ~03:20 UTC): gate logs under M1/commit/ (gate1 build test stride lint lists, gate2 wasm, gate3 bench)
- gate1.log (build test stride lint lists, default base): EXIT 1. Every group green to the last
  test result but lists: base.sh with no event took origin/main (5aad09dd), where every list is
  new, and the ratchet read the two declared raises as "matches no raise": RED. With the change's
  base (SUPDB_BASE=HEAD = b1e9a809, the remote branch tip, a push's before):
  gate1-lists-base-head.log EXIT 0. gate2 wasm EXIT 0, gate3 bench EXIT 0. Committed, and the
  base dependence is left open in the body (the ratchet's handling of a declaration for a list
  new since its base).
