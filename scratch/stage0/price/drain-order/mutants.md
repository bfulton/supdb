# drain-order mutants (tree that lands; runner mut.py, `cargo test --profile checked --test durable -- --test-threads=4`, 420 s limit; results mut-results.txt, logs mut-<name>.log; fixed src restored by sha256 after each)

| mutant | line(s) | verdict | killed by |
|---|---|---|---|
| M0 the old shape: collect_seal lands an awaits_writer front whenever its thread returned (followup 17's unsound shape; the deferred/report mutant for the presync order) | Maint::collect_seal | killed | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, ..._whichever_way_its_seal_lands |
| M1 writer's seam load never takes the cold path (`!= 0` -> `== 0xff`) | close_run, writer presync | killed | the_presync_seams_hold_and_fail_apart, a_flush_after_a_failed_presync_*, a_stopped_queue_answers_the_failure_that_stopped_it, a_presync_that_fails_before_its_seals_publish_stops_every_manifest(+_inline) |
| M2 the inline arm's thread never takes the cold path | close_run, presync thread | killed | a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records |
| M3 the hold loop never waits | Db::seamed_presync | killed | the_presync_seams_hold_and_fail_apart, a_flush_after_a_failed_presync_* |
| M4 the failure bit's test flipped | Db::seamed_presync | killed | 7 tests incl. the_presync_seams_hold_and_fail_apart |
| M5 the writer's presync not held (`holds` true -> false) | close_run, writer presync | killed | the_presync_seams_hold_and_fail_apart, a_flush_after_a_failed_presync_* (and, once, an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes, which then passed 30 of 30 runs alone on the fixed tree: not attributed to the mutant) |
| M6 the writer's seam check inverted (`!= 0` -> `== 0`) | close_run, writer presync | killed | 6 tests incl. the_presync_seams_hold_and_fail_apart |
| M7 item 12's assertion inverted | Db::hand_seal | killed | 136 tests |
| M8 the hold never lifted (`fetch_and(!PRESYNC_HOLD)` -> `fetch_and(!0)`) | Reader::hold_presync | killed (timeout: the held writer waits for good) | the binary hung at the 420 s limit |
| M9 item 12's assertion deleted | Db::hand_seal | SURVIVED, by construction: awaits_writer is `presync.is_some()`, and presync is Some only where `self.draining`; the brief asked for the assertion, which holds callers to it. M10 shows it firing. | -- |
| M10 close_run hands every non-inline close as awaiting the writer (`presync.is_some()` -> `true`) | close_run | killed, the item-12 assertion's message 22 times in the log | 22 tests incl. a_flush_after_a_run_closed_into_a_seal_makes_the_runs_commits_durable |

Not run: a mutant of the item-21 assertion: the assertion was built, fired in the gate on a reachable state (a flush after a presync failure, whose earlier awaits_writer job sits at the front of a stopped queue), was read as wrong and taken out; item 21 is held by a comment in Db::flush and by a_drain_names_its_runs_segment_durable_before_it_answers(+_inline), which pass before and after and are not evidence.

## Repair round 1: the reviewers' extra mutants and three of my own (runner mut2.py, same command, 600 s limit; results mut2-results.txt, logs mut2-<name>.log; src/db.rs and tests/durable.rs restored by sha256 after each)

The extra list names nine; two pairs are the same mutant (the fail bit not consumed; the inline thread held), so seven distinct, plus X8-X10 mine.

| mutant | verdict | killed by |
|---|---|---|
| X1 seamed_presync: `fetch_and(!PRESYNC_FAIL)` -> `load` (the fail bit never consumed; extra items 1 and 5) | killed | an_injected_presync_failure_is_taken_once(+_inline) (new). The first reading -- that a store whose presync failed never presyncs again, so the one-shot property was unobservable -- was WRONG: the new test showed a stopped queue still takes a later run's close while the frozen list has room, and presyncs it. The test does not re-arm and requires the first failure's record to stand; then re-arms and requires a new inode, so the second presync is shown to happen. |
| X2 the inline arm's thread passes `holds = true` (extra items 2 and 8) | killed | the_inline_arms_presync_is_not_held (new: hold on and a failure armed on the inline arm; the flush answers the failure with nobody lifting; the watchdog would lift at 20 s and fail the test) |
| X3 hold_presync(true): `fetch_or` -> `store(PRESYNC_HOLD)` (clears an armed failure) | killed | a_held_presync_takes_the_failure_armed_before_the_hold_after_its_lift (new: failure armed first, then the hold), and 4 others |
| X4 the seams test's lifter: `h.hold_presync(false)` deleted | killed (fails, no hang) | the_presync_seams_hold_and_fail_apart: ps_watchdog lifts the hold after 30 s and the test fails at "lifted by the watchdog". Before this round it hung. |
| X5 the hold loop after the fail check (the failure decided before the hold) | survived round 1a (my first test checked the extent while held, which the mutant records after the hold too); killed round 1b | a_failure_armed_under_a_held_presync_is_taken_at_its_lift (new: the failure armed from a handle while the writer is held -- fail_next_presync moved to Reader for it) |
| X6 hold_presync(true): `fetch_or(PRESYNC_HOLD \| PRESYNC_FAIL)` | killed | the_presync_seams_hold_and_fail_apart (its first flush().unwrap() / failed_presync_extent == None), both runs |
| X7 the hold loop `if` for `while` (MAINT_LAND_POLL is 100 us) | killed | the_presync_seams_hold_and_fail_apart, a_held_presync_..., a_flush_after_a_failed_presync_* |
| X8 (mine) hold_presync(false) clears the FAIL bit, not the HOLD bit | killed: round 1a by timeout (hang); round 1b by the watchdog's exit ("watchdog: a lifted presync hold still holds the flush", exit 101) | ps_watchdog's second stage: its own lift not freeing the flush in 20 s ends the process |
| X9 (mine) Reader::failed_presync_extent answers None | killed | 8 tests |
| X10 (mine) injected_presync records nothing | killed | 8 tests |

No survivor stands.

## Repair round 2: the reviewers' five and five of my own (runner mut3.py, `cargo test --profile checked --test durable -- --test-threads=4`, 600 s limit; results mut3-results.txt, logs mut3-<name>.log; src/db.rs and tests/durable.rs restored by sha256 after each)

This round first made the hold's wait observable: the held writer's presync sets `PRESYNC_WAITING` in the seam byte while it waits (inside the cold `seamed_presync`), `Reader::presync_hold_state()` reads (hold armed, waiting), and the three lifters wait for the writer AT the hold (`ps_wait_held`, 20 s bound, "the writer's presync never waited at the hold") before the segment's publication and the 100 ms check. The inline test asserts the hold it armed still stands after the unheld presync, and the seams test that nothing is left waiting after the flush.

| mutant | verdict | killed by |
|---|---|---|
| E1 hold_presync(false) as `presync_seam.store(0)` (clears a failure armed before the lift) | killed | a_held_presync_takes_the_failure_armed_before_the_hold_after_its_lift (as expected), a_failure_armed_under_a_held_presync_is_taken_at_its_lift, a_flush_after_a_failed_presync_* (2) |
| E2 the writer's check `& PRESYNC_FAIL != 0` for `!= 0` (a hold armed alone never holds) | killed, and by the wait, not the sleep: both failing tests' lifters panic at "the writer's presync never waited at the hold" (log lines 274-275, 310-311) | the_presync_seams_hold_and_fail_apart, a_failure_armed_under_a_held_presync_is_taken_at_its_lift. Before this round the kill was the 100 ms sleep's (the flush had to answer within it under the mutant); the wait makes it deterministic. |
| E3 `self.draining = true` after `seal()` (no awaits_writer job, no presync) | killed by 12 tests (all a_flush_after_a_failed_presync_*, the seams tests, taken_once x2, a_stopped_queue_answers_the_failure_that_stopped_it, a_presync_that_fails_before_its_seals_publish_stops_every_manifest(+_inline)); run with a_drain_names_its_runs_segment_durable_before_it_answers ALONE it SURVIVES (mut3-E3-draining-after-seal-alone.log) | confirms that test is not evidence of the order (as round 0 said: it passes before and after) |
| E4 seamed_presync: no fetch_and, then an unconditional `presync_seam.store(0)` (consumes the hold bit too) | killed | the_inline_arms_presync_is_not_held, at the new "the presync the hold did not hold took the hold the test armed". On the writer arm the store(0) runs after the lift, so it clears only bits armed between the lift and the sync's end, a window no hold reaches; the inline arm is where an unheld presync meets an armed hold. |
| E5 collect_seal `!(j.awaits_writer && j.draining)` for `!j.awaits_writer` | SURVIVED: equivalent. `awaits_writer` implies `draining` for every job (Db::hand_seal's debug assertion, item 12; M10 shows it firing on a caller that breaks it), so the two gates agree on every reachable job; no test can pin awaits_writer apart from draining, since the state that would is the one the assertion refuses. | -- |
| O1 PRESYNC_WAITING never set | killed | the three lifter tests ("never waited at the hold") |
| O2 PRESYNC_WAITING never cleared | killed | the_presync_seams_hold_and_fail_apart ("the held presync still says it waits after it answered") |
| O3 presync_hold_state's two bits swapped | killed | the_inline_arms_presync_is_not_held |
| O4 the `holds &&` guard dropped (the inline thread held) | killed | the_inline_arms_presync_is_not_held, an_injected_presync_failure_is_taken_once_inline |
| O5 WAITING set after the hold loop instead of before | killed | the three lifter tests |

Survivor: E5 only, equivalent by the item-12 assertion.

## Round 3 (mut4.py; results mut4-results.txt, logs mut4-<name>.log; tree restored to keep/ by sum after each)
- R1 PRESYNC_WAITING set on entry, never cleared: killed by the_inline_arms_presync_is_not_held
- R2 writer arm hands its job not awaiting: killed by a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, ..._whichever_way_its_seal_lands
- R3 (= R6) fail bit taken before the hold: killed by a_failure_armed_under_a_held_presync_is_taken_at_its_lift
- R4a Drain asked only after a run: killed (79 tests, e.g. a_flush_after_a_run_closed_into_a_seal_makes_the_runs_commits_durable)
- R4b Drain not asked after a run: killed by a_drain_names_its_runs_segment_durable_before_it_answers(+_inline), a_drain_lands_its_run_under_a_cipher and 9 more
- R5 inline thread passes holds=true: killed by the_inline_arms_presync_is_not_held
- R7 landing gate lets an awaiting front land on the seal thread's Ok: killed by a_flush_after_a_failed_presync_is_not_ok_* (2)
- R8a wedge moved after presync_failed: SURVIVED, equivalent (two consecutive stores on the writer's thread; presync_failed is read by the writer alone, so no observer sits between them)
- R8b wedge deleted: killed by a_flush_after_a_failed_presync_* (2), an_injected_presync_failure_is_taken_once, a_presync_that_fails_before_its_seals_publish_stops_every_manifest(+_inline)
