# G1-4 `offsets`: notes

Unit base (HEAD at start): 869a4fee (the stage script's commit after T17's notes).
Tree clean at start.

## Step: repair round 2 (see "Repair round 1" and "Repair round 2" at the end, which stand where
## they differ from the text above them). The finished copies are keep/ (sums in keep/SHA256SUMS); round 0's are keep-r0/.

## What the tree holds against the plan's reading at 682b2c09

- `Blob::willneed_ranks` (blob.rs ~1335), `willneed_dir` (~1372), `record_offset`
  (~1424) are as the plan read them: each over `self.flat()` alone, `0`/`None` for a
  source that lends by the piece, offsets by pointer subtraction
  (`recs.as_ptr() - sec.as_ptr()`, `dir.as_ptr() - sec.as_ptr()`).
- `record_region_place` (~1552) answers for the piece kind only (`self.pieces()?`),
  `None` for a whole reader; `record_region_span` (~1538) for the whole reader only,
  by `slice_at(0,1)` base subtraction. Callers of `place`: `records_resident`
  (db.rs, after `record_regions` is `None`) and the crossing's swap (db.rs, in the
  `else` of `record_region_span`): both reach it only where the whole kind answered
  `None`, so answering for both kinds moves neither.
- The stale doc the plan names moved: `record_region_bytes`'s doc is correct now
  ("however the reader holds its section"); the stale sentence is on
  `hash_region_bytes` ("0 for a reader that does not hold the section whole"), whose
  code answers 0 for `Index::Sparse` alone. Corrected there.
- New callers since the plan: `MergeAhead::at` (merge read-ahead) calls
  `record_offset` per rank; default `merge_read_ahead: 0` constructs no `MergeAhead`,
  and its `end` comes from `record_region_span`, `None` -> 0 under a paged source, so
  `ahead_span` answers `None` there: unmoved under a cipher even with the option on.
  `compact_run`'s `drop_at` now gates on `drops[i]` = `merge_drop_behind ||
  merge_drops_covered[i]`; both default false (db.rs Options::default).
- Who reaches the change under a cipher, verified in the tree: the `willneed_*` sites
  are under `pieces_willneed()` / `blocks_willneed()` (both need `zgate != GATE_RAW`);
  `cipher_controller` turns `Auto` off unless `zpages_under_cipher` (default off).
  So only `ZPages::Always` or `zpages_under_cipher` reach them under a cipher.

## The change

- `Blob::willneed_ranks`, `willneed_dir`, `record_offset`: the body moved into a
  `Sect`-generic `#[inline(always)]` helper (`*_in`); the whole arm calls it with the
  slice exactly where the old body ran; where `flat()` answers `None` the function
  calls a `#[cold] #[inline(never)]` `*_by_piece`, which takes `pieces()` and the same
  helper. [Round 0; round 1 replaced this, see "Repair round 1": the dispatch is on
  `flat_lent`, the offset is `Sect::start_in`, the pointer subtraction for a slice, and
  `region_starts` is gone.] The file offset is `index_offset() + region start (header word) + a` in both
  arms (`FlatIndex::region_starts`, new, `(recs.0, dir.0)`), never a pointer
  subtraction. For the slice arm `recs.as_ptr() - sec.as_ptr()` was `recs.0` by
  construction (`regions` is `sec.part(recs.0, ..)`), so the answer is the same.
- `record_region_place`: answers for both kinds through `indexed!` by the header's
  word (it answered `None` for a whole reader). Its two callers reach it only where
  the whole kind's `record_regions`/`record_region_span` answered `None`, so neither
  moves; G1-6 can now drop `record_region_span`.
- Docs: `record_offset`'s doc had been fused onto `willneed_dir`'s; each has its own.
  `hash_region_bytes`'s stale "0 for a reader that does not hold the section whole".
- Departure: the red test wraps a source (`Advised<B>`, recording every
  `advise_willneed` / `advise_dontneed_range`) around `Piecewise` and around a
  `VecBytes`, rather than adding counters to `Piecewise`: so the two readers are held
  to the same advice, call for call and range for range, and the whole one to the
  file (each record's key at its offset, each directory word the ask names giving the
  record's offset, the region's place equal to `record_region_span` of a mapped
  reader, every ask inside the region).

## Evidence (all in this directory)

- before.log: the first form of the test against the base, red (EXIT 101: "record offset at 0"
  by the piece). before-final.log: the final test with the base's src/blob.rs and
  src/flatindex.rs swapped in, red at the same assertion (EXIT 101); files put back by sum.
- after.log: whole suite `cargo test --profile checked --no-fail-fast`, EXIT 0, 1526 passed.
  after-dotdot-tmpdir.log is an earlier run of the same with five red (leak's
  a_closed_store_gives_back_what_it_allocated and four zpages mapping tests): my TMPDIR was
  written with a `..`, which CLAUDE.md names as failing exactly these (they match
  /proc/self/maps by the store's prefix). Rerun with a canonical TMPDIR: green. Not the change's.
- after-blob.log: tests/blob.rs whole after the test's last edit, 19 passed.
- stride.log: `sh scripts/check.sh stride`, EXIT 0.
- lint.log (clippy -D warnings, fmt, the door): green. lists.log: green. wasm.log: green.
- cost1.log: `sh scripts/cost.sh 869a4fee read scan scan_long merge land` -> "cost: green", EXIT 0
  (land run since flatindex.rs is touched). [WRONG, round 1 item 1: `record_offset` was out of
  line in the merge probe, 12 calls, 55 -> 57 a call, under the slack because the default path
  calls it once an input; the arms that call it a key were in no probe.] Libc only:
  - read: `__memcmp_avx2_movbe` +0.843 an op (+168,692), the same calls (200,582) from the same
    caller (`read_all_in`'s closure#5), identical in a second run (cost2.log). It is the same
    figure to the instruction (3,944,452 -> 4,113,144) G1-3 and T17 read for a head build against
    a base build with no engine change (G1-3 notes "memcmp +0.83/op in read" against itself;
    T17/r1/cost-vs-HEAD.log): a per-call count set by the operands' addresses. Under the libc
    slack.
  - merge (no toggle): memcpy/memcmp move both ways between two runs (+10.2 an op, then -5.1),
    libc noise; every engine function inside its slack.
  No `~` line, bound or slack touched. The base target was removed after the last run.
- strace (rule 1 and the plan's "must not"): a scratch example (g14_drop.rs, here; put in
  examples/ for the build and removed) building one partition and three pieces and merging them
  with `merge_drop_behind` on and `merge_drop_chunk` 64 KiB, under `setarch -R strace -f -e
  trace=madvise,fadvise64`, twice each, built at the tree and with the base's two source files
  swapped in: 224 fadvise64 and 242 madvise every run, the sorted calls with their arguments
  identical between runs and between base and tree (strace.{before,after}.{1,2}.norm).
- mutants.md: 18 mutants, all killed.

## What the change holds, and what it cannot see

- Holds: for a source that lends by the piece, `willneed_ranks`, `willneed_dir`,
  `record_offset` and `record_region_place` answer, call for call and range for range, as for a
  source that lends whole, and both readers' answers are held to the file (each record's key at
  its offset, directory words naming their records, place equal to a mapped reader's span, asks
  inside the region).
- Cannot see: what the pager does with the advice (`Paging::willneed`/`forget` behind
  `MmapBytes`); the test's source is a stand-in, and no store under a cipher with the compact
  form on was run with its asks counted. `compact_run`'s descriptor fadvise under a cipher is
  unchanged and S1's (the plan's "Leaves").
- Under a cipher, what starts to work (decision 16), on no default path: with the compact form
  on (`ZPages::Always`, or `Auto` with `zpages_under_cipher`) the builds' and settles' asks ahead
  of piece records and directory words (`pieces_willneed`, `blocks_willneed`,
  `pieces_dir_willneed`) now ask for the ciphertext's pages; with `merge_drop_behind` or
  `merge_drops_covered` a merge drops its inputs behind it through `Paging::forget`.
  `MergeAhead` (`merge_read_ahead`) still asks nothing under a cipher: its end comes from
  `record_region_span`, `None` there (G1-6's to move). Priced by S5 for the compact form, and by
  whoever turns `merge_drop_behind` on. No pairing taken. Owed for S5:
  `bench/scripts/pair.sh supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes read`
  at this commit and the one before.

## Departures from the plan

- The test wraps the source (`Advised<B>`) instead of giving `Piecewise` counting methods, and
  holds both readers to the file, not only to each other, so the equality is not the only check
  of the piece arm (mutants M8b, M8c).
- [Round 0, withdrawn in round 1.] The slice arm's offset was the header word too
  (`FlatIndex::region_starts`); round 1 keeps the pointer subtraction for a slice
  (`Sect::start_in`) as the plan said, and the +2 a call was not the header word but a
  callee-saved register (round 1, item 1). Round 1's departure instead: `Blob::flat_lent` and
  `Sec::lend`, a borrow with no error made, so the piece arm is reached with no call before it.
- `record_region_place` answers for the whole kind as well (its two callers reach it only where
  the whole kind answered `None`); `record_region_span` is left for G1-6.
- The stale doc was on `hash_region_bytes`, not `record_region_bytes`; `record_offset`'s doc had
  been fused onto `willneed_dir`'s and is split back.
- `raw_read_span` is another accessor over `flat()` alone ("None for a reader that does not hold
  the section whole"), used by the hot-raw policy; outside this unit's section, left, for G1-6.

## Not verified

- The pager's handling of the advice now reached (above).
- Row 3 (read_z, scan_z, scan_long_z) not counted: the plan names rows 1, 2, 4 for this unit
  (and land, counted).


## Repair round 1

Two reviewers' blocking items, both about the plain store's cost on the arms that call these
functions a key or a block at a time. Evidence under r1/.

### 1. record_offset's slice arm was +2 an out-of-line call (the reviewer was right)

Round 0's notes said "no engine function moved" and "record_offset inlines in compact_run": both
false. The scratch probe r1/drop.* (g14_drop.rs: one partition and three pieces merged with
`merge_drop_behind` on, so `drop_at` calls `record_offset` a key) counted by function
(callgrind, whole run, `callgrind_annotate --tree=caller`):

| build | record_offset calls | inclusive | a call |
|---|---|---|---|
| base 869a4fee | 120,001 | 6,600,055 | 55.0 |
| round 0 (header words) | 120,001 | 6,840,057 | 57.0 |
| round 1 with the pointer subtraction restored alone (`Sect::start_in`) | 120,001 | 6,840,057 | 57.0 |

So the header word was not the cause: restoring the subtraction changed nothing. The listing
(r1/ro.base.asm, r1/ro.tree.asm, objdump of the out-of-line function) named it: `push %r14` /
`pop %r14`, one callee-saved register more. The function's `flat()` failure path calls
`not_lent` (the error `Sec::get` makes and drops) before it returns; with a call to the piece
arm after that, `rank` has to live across `not_lent`'s call in a callee-saved register, which
the prologue and epilogue save and restore on every call, the slice arm's included. The rule
CLAUDE.md gives ("a cold call that returns is a call the caller's registers are saved across").

The fix: the three functions dispatch on `Blob::flat_lent` (new, `#[inline(always)]`): `flat`
through `Sec::lend` (new), the same `lend_at` test with no error made where the source does not
lend whole, so the piece arm is reached with nothing called before it and takes the function's
arguments as they came. The slice arm's offset is the pointer subtraction again, through
`Sect::start_in` (new trait method: a slice's pointer difference, a `Pieces`' offsets'
difference); `FlatIndex::region_starts` is gone. With that shape alone `record_offset` was small enough
that LLVM inlined it into `compact_run` and `MergeAhead::at`: the drop arm read 5.5 a key fewer
(r1/drop4.tree.self) and the default merge read RED, `compact_run` +800,580 over cost.sh's
merge run, +4.0 an op (cost3.log): the merge's loop, with the function inlined in it, kept a
register less for itself, though the default path calls it once an input. So it is
`#[inline(never)]`, out of line as the base had it, and the count by function is (r1/drop5,
r1/ahead5; calls from the cfn/calls records, the same in both builds):

| build | arm | record_offset calls | its instructions | a call | callers |
|---|---|---|---|---|---|
| base 869a4fee | merge_drop_behind | 120,006 | 6,600,330 | 55.0 | compact_run 61,088,588 |
| tree | merge_drop_behind | 120,006 | 6,120,306 | 51.0 | compact_run within 300 of the base |
| base | + merge_read_ahead 1 MiB | 240,007 | 13,200,385 | 55.0 | MergeAhead::at 5,185,001 |
| tree | + merge_read_ahead 1 MiB | 240,007 | 12,240,357 | 51.0 | MergeAhead::at within 300 |

Four instructions a call fewer: `flat_lent` makes no error where `flat` made one and dropped it,
and the one save the cold arm's call cost is gone with the call before it. cost.sh's merge row
with this shape is cost4.log (below).

The intermediate measurements, kept for the reviewers: with `record_offset` inlined, base against tree
(r1/fnsum.py over the self rows):

- r1/drop.* (merge_drop_behind): base `record_offset` 6,600,330 + `compact_run<NoRest>`
  61,088,588; tree `record_offset` 0 (inlined) + `compact_run` 67,028,880: 660,038 fewer over
  120,001 calls, 5.5 a key fewer. Every other engine function within a few hundred over the run
  (thread timing).
- r1/ahead.* (the same merge with `merge_read_ahead` 1 MiB, `G14_AHEAD`): base `record_offset`
  13,200,385 (240,002 calls, 55.0) + `MergeAhead::at` 5,185,001 + `compact_run` 61,808,594; tree
  0 + 10,345,098 + 67,748,886: 2.1M fewer, 8.7 a call fewer.
- The default merge path (`dropped_to`, a call an input) is under cost.sh's merge row below.

A probe whose own loop calls the public function (r1/count.*: g14_count.rs, every rank of a
mapped partition through `record_offset`, every 64th through `willneed_ranks` and `willneed_dir`,
each loop an `#[inline(never)]` function with `black_box` around the blob and the rank) reads the
inlined copies at +1, +4 and +1 instructions a call (offsets 1,180,026 -> 1,200,028; ranks
57,311 -> 58,563 over 313 calls; dirs 50,114 -> 50,425): the probe's own register allocation
around an inlined body with a second arm (r1/off.*.asm: a cmove for the Option and the
`black_box` reloads). That is the caller's codegen and not an engine function; every engine
caller counted (here and in item 2) reads the same or lower. Said here so the reviewers can hold
it: an engine caller that inlines these and reads higher would be a cost, and the ones counted
are compact_run, MergeAhead::at, scan_blocks, overlay_runs and build_slot.

### 2. willneed_ranks and willneed_dir counted where they run

No cost.sh probe sets the gate away from the raw form, so none reaches them (the reviewer was
right; their callers are gated on `pieces_willneed()` / `blocks_willneed()` /
`pieces_dir_willneed`). The scratch probe r1/gate.* (g14_gate.rs: a plain store, one partition
and three pieces, `ZPages::Off` so no form file serves the partition, `Db::zgate_set(1)` --
GATE_CENSUS -- by hand, then 1,000 scans of 50 through the block cache and 20 commits of 100
writes, settled) reaches them: about 5,800 `mincore` calls, the asks' residency checks (strace),
in the blocks' builds and the settles. Both functions inline into every engine caller in base
and tree alike. By function, base against tree, two runs each (r1/gate.*.self, r1/gate2.*.self):

- `Reader::scan_blocks` 4,376,158 -> 4,319,869 (-56,289, every run), `BuildCtx::overlay_runs`
  8,922,900 -> 8,921,962 (-938, both runs) and, with the directory's dead guard taken out
  (below), 8,915,396 (r1/gate4.tree.self), `Reader::build_slot` the same to the instruction.
- `Reader::sweep_retired_states`, `Readers::oldest_for` and `WriterOp::drop` move by up to 1.5M
  between two runs of one build (1,174 / 1,221,352 / 2,761,453 across runs): what the upkeep
  thread's pins happened to hold at each operation's end. No change of this unit reaches them.

### strace (rule 1), again with round 1's code

`setarch -R strace -f -qq -e trace=madvise,fadvise64,mincore` over r1/drop.* (merge with drop
behind) and r1/gate.* at base and tree: drop's sorted calls identical (466 each); gate's the
same counts by kind (5,794 mincore, 4 madvise WILLNEED, 9 DONTNEED, 1 fadvise64) -- the files
differ only where strace split a call across threads as `<unfinished ...>` / `resumed`
(r1/strace.gate.*.kinds).

Again with the final code (`#[inline(never)]` on `record_offset`, the directory guard gone):
drop's sorted calls identical to the base's (r1/strace.drop.fin.norm against strace.drop.base.norm);
gate, three runs each of base and tree (r1/strace.gate3.{base,fin}.{1,2,3}): 5,794 `mincore`, 4
`MADV_WILLNEED`, 8 `MADV_RANDOM` in every run of both; `MADV_DONTNEED` 11, 11, 11 at the base and
10, 11, 10 at the tree, and 9 in the base's first run above -- a count that moves between runs of
one build (the blocks and memtables the upkeep thread and the segment work free), not between
builds.

### The tests, held further (tests/blob.rs)

- `Advised` says one range is resident (`resident_at`), none by default; `holds_to_the_file`
  sets the first record's first byte resident and requires `willneed_ranks(0, ..)` to ask
  nothing and `willneed_ranks(keys/2, ..)` to ask, then the directory's first word resident and
  `willneed_dir(0, 1)` nothing, `willneed_dir(1, 1)` an ask, and rank 0 asked again once its page
  is no longer the resident one: on both kinds of reader.
- Every ask over ranks and over directory words asks for more than nothing, and the ask past the
  last rank (`(keys, 3)`) is no ask at all.

### Docs and lists

- `tests/data/cost-entries.txt`: `Blob::record_offset`'s reason said "once a build or a sample",
  false since `drop_at` and `MergeAhead::at` call it a rank at a time; it says what calls it and
  that those arms are off by default and run by no probe. A reason's text, no count moved.
- Owed (not built here): a probe of scripts/cost.sh that sets the gate away from the raw form
  (g14_gate.rs's shape, a toggle on the main thread's scans) and one that merges with
  `merge_drop_behind` / `merge_read_ahead`, so these arms are counted by the gate and not by a
  unit's scratch probe; with no toggle the upkeep thread's sweep noise above would read red
  against itself, so it wants a toggle.

### The directory ask's guard taken out (a check nothing reaches)

The extra mutants X3b (the piece arm's directory span cut at the section's end) and X5b
(`a >= b` to `a > b` in `willneed_dir_in`) survived, and so would any mutant of those two lines:
`dir_span(rank, n + 1)` ends at `min(rank + n + 1, nkeys)` words with `rank < nkeys`, so it is
never empty, and an index that parses has a directory of at least `nkeys` words
(`FlatIndex::parse`: with no capacity the region is `nkeys * 4` bytes; with one, `live_n <=
dir_cap` and the live buffer is one of two of `dir_cap` words). `b.min(dir.size())` and `a >= b`
were the base's, and could not fire. Taken out (rule 5), with a comment saying why the span is
inside and not empty; the slice arm is two compares lighter (overlay_runs above). `prefetch_dir`
keeps its own `b.min(dir.len())`, outside this unit.

### Mutants that survive, each read

- X7 (`#[cold] #[inline(never)]` dropped from `willneed_ranks_by_piece`): no test can see it; the
  count does. r1/gate4.{tree,x7}: with the attributes gone `willneed_ranks` is out of line and
  `Reader::scan_blocks` + `willneed_ranks` read 75,689 more over the run. Held by the scratch
  probe's count, by no gate (the cost.sh probe for the gate away from the raw form is owed, above).
- Y4 (`flat_lent` through `flat`, the error made): the same answers; the count is what tells it,
  and it was counted: it is the round-1 shape with the subtraction alone, 57 a call against 55
  (item 1's table). Held by the same scratch count, by no gate.
- X8 (the piece arm's region place answered by `Pieces::offset`): equivalent. A part's
  `Pieces::offset` is the section's offset in the source plus its start in the section, and the
  section's offset is `index_offset()` (`Sec::off`), so the two expressions are equal for every
  input; O6 (the old shape, piece kind only) is killed by the whole kind.

### What the round left owed

- The scratch probes (r1/g14_count.rs is $B/g14_count.rs, g14_gate.rs, g14_drop.rs here; put
  in examples/ for the builds and removed) are what count the arms that call these per key or
  per block; no probe of scripts/cost.sh does. A probe with the gate set away from the raw form
  (Db::zgate_set, a toggle on the main thread's scans) and one merging under merge_drop_behind
  and merge_read_ahead are owed to the cost job.
- The inlined copies in a probe's own loop read +1 (record_offset, measured while it was inlined;
  it is out of line now and 51 a call), +4 (willneed_ranks) and -5
  (willneed_dir) a call (r1/count.base.self2 against r1/count4.tree.self): the caller's register
  allocation around a body with a second arm, a cost no engine caller counted shows.

### The rows, counted again with round 1's code (rule 1)

- cost3.log: `sh scripts/cost.sh 869a4fee read read_z scan scan_long scan_z scan_long_z merge
  land` with `record_offset` inlined -> `cost: RED`, EXIT 1: `compact_run` +800,580 over the
  merge run (item 1). That is what made `record_offset` `#[inline(never)]`.
- cost4.log: the same rows, final code -> `cost: green`, EXIT 0 (COST_DIR cost4, the base's
  target kept in tb between and removed after). By function: `compact_run` identical
  (66,101,734 both sides), `record_offset` 660 -> 612 over its 12 calls (55 -> 51 a call), no hot
  function moved in read, read_z, scan, scan_long, scan_z, scan_long_z; in land
  `BuildCtx::overlay_runs` -1,564 and `Reader::scan_blocks` -62 over the run; `read`'s memcmp
  +0.75 an op is the libc per-call figure G1-3 and T17 read against an unchanged engine (3,962,770
  -> 4,113,144, the 4,113,144 of round 0's head again). No line of scripts/cost-hot.txt, no bound
  and no slack touched. Rows 3 (read_z, scan_z, scan_long_z) counted this round too: in none of
  them is an ask a block or a piece: the toggled windows of read_z and scan_z make no `mincore`
  or `madvise` at all and scan_long_z's one, where the scratch gate probe makes some 5,800; so
  rows 1-4 cannot see the willneed functions, and item 2's probe is what counts them.
- lint-r1.log, lists-r1.log, wasm-r1.log: `sh scripts/check.sh lint`, `lists`, `wasm`, each
  "all checks passed", EXIT 0.
- after-r1.log: `cargo test --profile checked --no-fail-fast`, EXIT 0, 1526 passed in 27
  binaries, none failed. `cargo fmt --check` clean.
- stride-r1.log: `sh scripts/check.sh stride` red once, EXIT 101, in a test the diff does not
  reach: `db::pager::tests::a_view_says_what_is_in_memory_and_drops_and_asks_for_what_stands_behind_it`
  at pager.rs:1779, `m.resident_at(100_000)` Some(false) where Some(true) -- the kernel's page
  cache of a test file read back a moment before, under the whole lib binary's load (src/db/pager.rs
  and src/bytes.rs are untouched by this unit, and the same test passed in after-r1.log).
  pager-alone.log: the test alone under `SUPDB_TEST_STRIDE=1`, five runs, five green.
  stride-r1b.log: the whole stride group again, EXIT 0, "all checks passed: stride". Reported, not
  read as the unit's; not in the launcher's list of known flakes, so named here for whoever keeps it.

### Where everything is

- Finished copies: keep/ (keep/SHA256SUMS: src_blob.rs 9f26e74d..., src_flatindex.rs
  ac1913c3..., tests_blob.rs 8eccd4af..., tests_data_cost-entries.txt 3cca5125...), the same as
  the working tree at the end of round 1. Round 0's are keep-r0/.
- Round 1's runners: mutants_r1.py (results-r1.txt), mutants_r1b.py (results-r1b.txt); scratch
  probes g14_count.rs, g14_gate.rs, g14_drop.rs and their callgrind outputs under r1/ (binaries
  removed); cost3/, cost4/ (cost.sh outputs; the base's target tb/ removed).

## Step: round 1 done, uncommitted. For the commit step: the message says that what this changes
## for a store under a cipher is behind the compact form (ZPages::Always, or Auto with
## zpages_under_cipher) or merge_drop_behind / merge_drops_covered only (decision 16), priced
## by S5 (`bench/scripts/pair.sh supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes
## read`, owed).

## Repair round 2

No blocking items; ten extra mutants. No compiled engine code changed this round (src/ is
round 1's to the byte: src_blob.rs 9f26e74d..., src_flatindex.rs ac1913c3...); two tests
gained assertions or were added, so the unit's callgrind rows were not counted again.
Finished copies: keep/ (keep/SHA256SUMS, now with src_db.rs, the tree's unchanged db.rs the
runner restores, and tests_fences.rs).

### The mutants (runner mutants_r2.py, verdicts results-r2.txt, logs mutants-r2.log, mutants-r2b.log)

Eight of the ten are code mutants run against tests: Z1, Z2, Z3, Z4, Z7, Z10 (and Z10s, Z10p,
the guard taken out of one arm at a time) killed by `the_advice_by_rank_...` as the reviewers
expected. Two survived the first pass, and each was a missing test:

- Z6 (`Sec::lend`'s whole arm by `slice_at` in place of `lend_at`): a source that lends by the
  piece then lends the key section whole, the reader takes the slice arm, and every answer is
  the same -- over a paged mapping that is `slice_slow`, the section made present whole through
  the pager at every call. The test now holds the bytes the piece reader asks its source for
  across the trace's calls (`Piecewise::asked`) under a quarter of the record region: 4,848
  bytes a trace against regions of 39,432 and 33,720. Killed (tests/blob.rs, the new assertion).
- Z5 (`compact_run`'s first drop from `record_offset(1)`): no test held where the drops behind a
  merge begin. New test `tests/fences.rs`
  `a_merges_drops_behind_begin_at_each_inputs_first_record`: one partition and two pieces, the
  merge held at its start (`hold_merge_start`), each input opened as a `Blob` and its ranks walked
  as `drop_at` walks them, the expected bytes summed, and `merge_dropped_bytes` held to the sum
  after the landing. It passes on the tree as it stands (the mechanism is the base's; the
  mutants are its red input). Killed, with three of my own on the same lines: Z11 (a drop to the
  record's page, not a page behind), Z12 (`dropped_to` not moved, every drop from the start), Z13
  (the first drop from the file's start): each killed by the new test.

Mutant 8 (`#[cold] #[inline(never)]` dropped from `willneed_dir_by_piece` and
`record_offset_by_piece`), counted: r2/count8.sh, EXIT 0 (r2/count8.log), the scratch probes
g14_drop and g14_gate built at the tree and with the mutant, by function (r2/{drop8,gate8}.{tree,mut}.self):
- drop (merge_drop_behind, `record_offset` a key): `record_offset` 6,120,306 -> 7,680,384 over
  120,006 calls, +13 a call: with the piece arm inlinable it is inlined into `record_offset` and
  the slice arm pays. Every tests passes under it (no answer changes); the cost.sh merge probe
  calls it 12 times on the default path, 156 instructions under the 2,000 of its slack, so it
  is held by the scratch count and by no gate (the probe that merges under merge_drop_behind
  is owed to the cost job, as round 1 said).
- gate (blocks built with the gate away from raw): `BuildCtx::overlay_runs` 8,915,396 ->
  8,903,202, 12,194 fewer over the run with the attributes off; `willneed_ranks` is an
  out-of-line symbol in both builds and the sum of it, `scan_blocks` and `overlay_runs` is
  13,595,482 (the same to the instruction as round 1's r1/gate4.tree) against the base's
  13,659,275 and the mutant's 13,583,288. So on the directory's arm the attribute costs
  2 or so a call against its absence, while the tree is still under the base; it is left as it
  is (no engine change this round), and named here: dropping it from `willneed_dir_by_piece`
  alone is a cheaper shape for whoever counts the rows next. Correction to round 1's notes:
  "both functions inline into every engine caller" -- in the final tree `willneed_ranks` is out
  of line (its own self rows in r1/gate4.tree and r2/gate8.tree), `willneed_dir` inlined.

Mutant 9 (`#[inline(never)]` off `Blob::record_offset`) as a gate red input, a script step:
r2/red9.sh, EXIT 0 "red9: ok" (r2/red9.log): a copy of the tree with the edit, `sh
scripts/cost.sh 869a4fee merge` -> exit 1, last line `cost: RED`, `HOT GAINED:
supdb::db::compact_run +4.003 an op, +800,580 over the run` (r2/red9/mutant.log); the same copy
without the edit, the same targets -> exit 0, `cost: green` (r2/red9/control.log). The shared
target and the copy removed after.

### The tests, lint and lists this round

- after-r2.log: `cargo test --profile checked --no-fail-fast`, EXIT 0, 1527 passed in 27
  binaries (round 1's 1526 and the new fences test), none failed.
- lint-r2.log (`sh scripts/check.sh lint`), lists-r2.log (`lists`): "all checks passed", EXIT 0
  each. `cargo fmt --check` clean. wasm not run again: no compiled engine code changed.
- The tree is keep/ file for file (cmp). The private TMPDIR removed at the end.

## Step: round 2 done, uncommitted. The commit step's note from round 1 stands.

### Where everything is (round 2)

keep/ (sums in keep/SHA256SUMS); mutants_r2.py, results-r2.txt; r2/red9.sh, r2/count8.sh and
their outputs. The scratch probes' binaries were removed and examples/ holds none of them.
