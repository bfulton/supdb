# T23: the concurrent fuzz's convert steer counted four moves for three asks

Seen once: T21's commit gate, stride group, `concurrent_range` with ZFUZZ_CONVERT, seed 0:
"three settled asks of the first partition moved it in and out (1, 3) times"
(red-in-T21-gate.txt). Green in 53 runs after it (T21's r10/loop-tree.log, fuzz-rerun.log).
Not T21's: its src diff is renames.

## The test
tests/fuzz.rs run_concurrent, the block `if convert { ... }` after the writer's loop (written at
the upstream merge's repair, M1): after `db.commit(); db.settle();` it read
`before = db.zpage_counts()`, asked partition 0 three times with `zpages_settle()` after each, and
required in + out == 3 and |in - out| == 1.

## Two guesses, both refuted (0 red)
1. A conversion's build made long (scratch SUPDB_SCRATCH_ZCONVERT_MS = 0, 20, 100, 300 ms):
   0 red of 8 at each width (sweep-convertdelay-refuted.log).
2. The segment work's own turns made rare (scratch SUPDB_SCRATCH_MAINT_POLL_MS = 10, 100, 1000):
   0 red of 6 at each (sweep-before.log).

## The trace (scratch SUPDB_SCRATCH_ZTRACE; trace-seed0.log)
Every ask taken, every conversion collected and counted, each plain settle's and form settle's
begin and end with (build in flight, ask pending), and the test's reading and asks, on one clock.
First run, seed 0, green: at the test's reading a build was in flight and ask 76 (the loop's last)
was pending; the test's first ask overwrote it, the form's settle waited the build out and then
took the test's ask. Three moves.
What the code says of the red (src/db.rs): `MaintAsk::Settle` joins seals, merges, finishes,
rewarms and readahead asks; it joins no form build (`zbuild`) and takes no conversion ask. The
segment work's loop handles a job and then, on its next iteration, runs `z_step`, which with the
builder free and nothing due takes a pending ask (`z_convert_asked`). So with the builder free
at the last settle and the loop's last ask untaken, that turn starts the conversion within
microseconds of the settle's answer, the test reads its baseline with it in flight, and it is
counted inside the first settled ask: four moves. With the leftover on another partition the
count is (1 in, 3 out), the gate's; on partition 0 it is (2, 2).

## The state built, red first (forced.sh; forced-before.log, forced-after.log)
Scratch, all environment-gated and all removed: before the last commit and settle the test
settles the form (builder free, nothing pending) and stores one ask (SUPDB_SCRATCH_LEFTOVER);
the writer's thread wakes 2 ms late from the settle (SUPDB_SCRATCH_PRE_BEFORE_MS=2); a
conversion's build lasts 40 ms (SUPDB_SCRATCH_ZCONVERT_MS=40).
- Unfixed test: 4 red runs of 4, all four seeds in each: seed 0 (2, 2), seeds 1-3 (1, 3).
- Fixed test, same schedule: 0 red of 6 runs.
(An earlier forced schedule, a plain settle that waits for the build in flight, was 0 red of 3:
after that build a first build of a merge's output was due and took the builder, so the leftover
stayed pending until the test overwrote it. old/.)

## The fix
`db.zpages_settle()` before the baseline is read: it takes what was asked and waits for it, so
the count starts with the builder at rest and nothing pending. Test only; no engine change.
CLAUDE.md: the entry "Two counts compared across a worker's race" has the case and its rule.

## Scratch removed
src/db.rs and tests/fuzz.rs restored from keep/*.orig, sha256 checked against keep/orig.sha256
(db.rs c412605c..., fuzz.rs de748fab... before the fix); the fix applied by fix.py alone;
scratch-and-fix.patch keeps the scratch for the record.

## Verification (verify.sh, verify.log)
See the log's "-- exit" lines: the fuzz binary under the test group's environment, under
SUPDB_TEST_STRIDE=1, the stride group's four fuzz commands, twelve more runs of the concurrent
range with conversions, then the lint group.
