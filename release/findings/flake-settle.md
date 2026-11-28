# Flake: `tests/settle.rs` `a_copy_of_fixed_size_records_is_one_stream_until_a_write_it_cannot_take`

Times are UTC, 2026-11-28. Tree: `abeecbdd` (the coordination branch's head
when this started). `$B` is the `settle` binary built with
`cargo test --profile checked --test settle --no-run`, and every run has a
`TMPDIR` of its own, cleared between runs.

## Cause

**The engine's, not the test's premise, and not the memory reading.** The
writer gives the upkeep thread a false "the writes outran you" ask, and the
thread's next step answers it by giving its forms up (drop site 7,
`Reader::abandon_forms`). That drops every copy the test carries across the
merge.

The race is between one writer freeze and the upkeep thread's look:

1. The second seal's freeze (`Db::freeze_swap`, `Options::freeze_settles`
   off) reads where the upkeep's look stands, in `Db::kept_from`:
   `from = 1`.
2. The upkeep thread's pass over the state the first freeze published
   (`lives 2`) looks (`Reader::note_over`). It stores `upkeep_lives = 2` and
   clears the kept table it has now read.
3. The freeze's publish swaps in `lives 3`. `Db::swap_state` trims the new
   state's kept list by the look's position read *after* the swap (2). That
   leaves one table, which is right: the look is one table behind.
4. `Db::ask_abandon` judges that same list against the `from` read in step
   1: `State::keeps_short` sees `3 − 1 = 2 > 1 kept`, and it stores the
   ask. The look is not behind what was kept for it.
5. The pass's next step finds the ask with the scan count unchanged
   (`Reader::abandon_by_writer`) and gives the forms up: `drop_blocks_for(7)`,
   every copy gone. The two extra drops at site 1 (refusal 1) are the
   handles' own reads that follow.

The same stale `from` reaches `ask_abandon` from `Db::freeze_table`,
`Db::set_mem_with` and the landing that swaps the live table in
`Reader::publish_segs_with`.

What it is not:

- **Not the keep.** `upkeep_keep_bytes` resolved to 1792296448 here: the
  memory cgroup's `hierarchical_memory_limit` of 14338371584, divided by 8.
  It is read from the limit, not from `MemAvailable` or the page cache, and
  the test's tables are kilobytes. The pin's ask (`abandon_by_pin`) never
  fired in any run. Every abandon was the writer's ask.
- **Not what the earlier binaries leave.** The probed `test` group passed
  this test once. Unforced, the test alone, four copies at once, fails 2 of
  200 at `abeecbdd` and 3 of 400 at `793cec2b`. The earlier binaries move
  the timing, nothing more.
- **Not new in the three merges.** `793cec2b` fails the same way, in both
  arms. The gate's green there was luck, so there was nothing to bisect.

## Evidence

The probe is `flake-settle/force-stale-look.diff`. It prints the inputs at
each ask, each look's store, each `kept_from` and each swap's fresh read,
and it adds the seam below behind `PROBE_FORCE=1`. libtest captures the
output of threads a test spawns, so read the probe with `--nocapture`.

- Unforced, at `abeecbdd` with the probe, four loops of 50 at once:
  `TMPDIR=<own> $B --exact a_copy_of_fixed_size_records_is_one_stream_until_a_write_it_cannot_take`
  failed 2 of 200. Each failing run printed
  `PROBE ask_abandon lives 3 from Some(1) kept 1 keep_bytes 1792296448 scans 31`,
  then `abandon_by_writer at 31`, then `abandon_forms at 31 thread Some("supdb-upkeep")`.
  It also gave the gate's message: site 1 from 3 to 5, site 7 at 1,
  `left: (0, 0)`. Read once more with `look_now`, the ask printed
  `look_now 2`: the look already stood at 2 when the writer asked with 1.
- The order in a passing run (`--nocapture`, `false` arm):
  `look stores 2 understudy true thread Some("supdb-upkeep")`, then
  `kept_from Some(2) lives 2`. The look comes just before the freeze's read.
  The failure is the same look falling just after it.
- Unforced, at `793cec2b` (a worktree with a target of its own, no probe),
  the same loops: 3 of 400. The messages are the same in both arms (`true:`
  with `right: (11, 11)`, `false:` with `right: (0, 11)`).

## The forced phase

With `force-stale-look.diff` applied, the look over `lives 2` on the upkeep
thread waits until the freeze has read `from`. The freeze then waits 20 ms,
time for the look to store its position and clear. The look then waits until
the ask is made, so that the pass's next step meets it.

`TMPDIR=<own> PROBE_FORCE=1 $B --exact a_copy_of_fixed_size_records_is_one_stream_until_a_write_it_cannot_take --nocapture`
fails 5 of 5, with the gate's message exactly:
`true: carried as they were; drops by site and refusals before ([0, 3, 0, …], [0, 3, 0, …]), after ([0, 5, 0, 0, 0, 0, 0, 1, 0, …], [0, 5, 0, …]) … left: (0, 0)`.

An earlier form of the seam is part of the evidence. It held only the freeze
after its read, and the ask then came 5 of 5 (`from Some(1) kept 1 look_now 2`).
But no step of the pass came after it before the writer's own look at 3,
and the test passed. Both halves are needed: the stale ask, and a step of
the pass after it.

## The sibling

`a_copys_identity_outlives_a_merge_that_kept_its_partitions_keys` has the
same cause. It runs the same sequence (`--nocapture`: the upkeep's look
stores 2 just before the second freeze's `kept_from`). Under the same seam,
`TMPDIR=<own> PROBE_FORCE=1 $B --exact a_copys_identity_outlives_a_merge_that_kept_its_partitions_keys --nocapture`
fails 3 of 3 with the hosted message:
`the copies carried across: 0 claimed, 0 of their partitions`, via
`ask_abandon lives 3 from Some(1) kept 1 … look_now 2` and then
`abandon_forms … supdb-upkeep`.

The same cause explains macOS. There `zfile::read_budget` reads nothing, so
the keep is unbounded and the pin's ask is off, as the flakes session found.
But the writer's ask does not depend on the keep.

## Fix

The fix is commits `7a026f5`, `d20d75a` and `4485fca`. It changes the engine's behaviour, so it was sent to the coordinator before it was written (13:42Z) and approved (13:42Z).

`State::keeps_short` stops judging by `keep.from`, the position the publish read before its swap. After the swap it reads the kept list, then the look's position (`Shared::upkeep_lives`, SeqCst). The publish's own "no look" (`keep.from == u64::MAX`) still asks nothing, so it asks only where the old judgement asked. This covers every site that hands `ask_abandon` its `from`: `freeze_table`, `freeze_swap`, `set_mem_with`, and the landing that swaps the live table in `publish_segs_with`.

**No fence.** The comment at `keeps_short` gives the ordering argument, and the coordinator agreed (13:46Z). Every trim of a kept list is made under the list's lock, after its maker stored or loaded a position at least the trim's bound: the look in `Reader::note_over`, a publish in `swap_state`. So a list read after a trim is followed by a position at least that bound. That is a message passed through the lock, not the store-buffering shape. A fence there would have no litmus that can fail. `swap_state`'s own store-buffering pair, its CAS then load of the position against the look's store then load of the state, is SeqCst operations on both sides and is unchanged.

**Seams and test.** `Db::look_inside_a_freeze` arms three bits of `kept_hold`, and the holds chain themselves. Each wait spins (`freeze_chain_wait`, cold) for a minute at most, and the seam makes no thread.
- The upkeep thread's look that moves the position waits before its store until the freeze waits (`KEPT_HOLD_LOOK_STORE`, in `note_over`).
- The freeze that read a position behind its state waits until the look has stored and trimmed (`KEPT_HOLD_FREEZE`, in `kept_from`; `freeze_held`, cold).
- The look then waits after its trim until `Db::let_the_look_go` (`KEPT_HOLD_LOOK_TRIMMED`).
- `Db::look_inside_a_freeze_held` says whether the chain ran whole.

The test is `tests/settle.rs` `a_look_that_lands_inside_a_freeze_asks_the_upkeep_to_give_nothing_up`. It asserts its premise first: the chain ran whole, the look one table behind, one table kept. Then it asserts no ask, nothing given up, and the copies carried across the merge.

The seam's first shape spawned a thread to drive the holds. The gate's `source` tests refused it: 5 more `std::thread` uses in `src/db.rs` (the door, 123 > 118), and public functions on no line of `tests/data/cost-entries.txt`. The chained shape adds no thread use. Its three entry points are on `cost-entries.txt`, declared in `scripts/raises.txt`.

## Verdicts

- Mutant, judging by `keep.from` again (the code before the fix):
  `$B --exact a_look_that_lands_inside_a_freeze_asks_the_upkeep_to_give_nothing_up`
  red 3 of 3, `no ask to give the forms up for a current look`, with the premise assertions passing. That held under both seam shapes. With the fix: green 3 of 3.
- Unforced, four at once, 100 runs each:
  - `a_copy_of_fixed_size_records_is_one_stream_until_a_write_it_cannot_take`: 400 of 400 green (before: 2 of 200 red).
  - `a_copys_identity_outlives_a_merge_that_kept_its_partitions_keys`: 400 of 400 green.
  - Under `SUPDB_TEST_STRIDE=1`, two at once, 100 runs of each test: green.
- Cost, `sh scripts/cost.sh abeecbdd land merge seal`: green.
  - `land` −4.95 an op over 100,000; `Db::freeze_table` +32 instructions over the run.
  - `seal` +0.00; `merge` +5.04 an op (a whole-mode probe, within its slack).
  - The first count, with `kept_from` out of line, was RED on `Db::kept_from` +352 over the run. Commit `d20d75a` made it inline again.
  - `sh scripts/check.sh cost` against its own base (`b3a043dc`, C0) is RED, but on functions this change does not touch (`Blob::scan_at_in` +5367 an op on `scan_at`, `Reader::settle_each` +14 on `land`, `compact_run`, `Reader::scan`). Against `abeecbdd`, `settle_each` does not move. That RED is the coordination branch's since C0, not this change's.
- Whole gate, `sh scripts/check.sh`, on `4485fca`, after `git fetch origin main`, with a `TMPDIR` of its own: passed twice, 14:57–15:38Z and 15:38–16:01Z (`all checks passed: build test stride onefile lint wasm bench lists`). `sh scripts/check.sh lists` let the three raises through.

## Left open

- The seam's own mutants (a hold removed) were not run: each would end red after its 60 s bound at the premise `the look and the freeze each held`.
- The test cannot catch a mutant that reads the position before the list. In the seam, the look's trim lands before the publish, so that order makes no difference there.
- The `cost` group against C0 is RED on the coordination branch's own changes (above). That is for whoever counts the branch.
