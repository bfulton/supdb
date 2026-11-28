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

Proposed, and sent to the coordinator before it was written (13:42Z), since
it changes the engine's behaviour. `ask_abandon` stops judging by the `from`
the publish took before its swap. After the swap it reads the kept list
first, then the look's position (`upkeep_lives`, SeqCst). The look stores its
position before it trims, so a position read after the list is never newer
than a trim the list reflects. The two then agree.

Status: _pending the coordinator's answer._

## Mutants

_Pending the fix._
