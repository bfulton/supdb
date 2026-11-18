# drain-order (followups 17, 19, 21, 12)

## State (keep current)
- Step: 1-3 small changes in (item 19 seam byte, item 12 assert, item 21 assert); building measuring field next.
- Fixed copy: keep/src_db.rs.fixed (sha256 in keep/sha256.txt). Tree is the fixed one unless "MEASURING IN" below.
- TMPDIR for tests: scratchpad/stage0/tmp/drain-order

## Reading
- awaits_writer jobs: made only in close_run's non-inline arm (close_finish_inline off, the default), presync Some only when draining && close_presync (default on). So the default arms (supdb, supdb-ingest) take this path at every flush that closes a run.
- collect_seal: `sealed && !front.awaits_writer` lands the front whole; awaits_writer jobs land durable only in join_seal (Drain ask).
- Item 21: unreachable as the code stands. flush -> seal(); the ordered branch is `commit_direct()?; return self.close_direct();`; close_direct = land_seal()? (before any hand-over) then close_direct_with_room (no Result) -> close_run (no Result), then Ok(()). So seal() never answers Err after close_run handed an awaits_writer job; the Drain ask follows every such hand-over. Held by a debug assertion in flush (inline segment work only: the writer cannot see the Away queue) and a comment. No test can build the state.
- Item 12: debug_assert!(!awaits_writer || draining) in hand_seal.
- Item 19: fail_presync (AtomicBool, swapped unconditionally on the writer path) + presync_hold (AtomicBool, loaded before) -> one AtomicU8 presync_seam (PRESYNC_FAIL=1, PRESYNC_HOLD=2), loaded once (Acquire); nonzero -> cold seamed_presync. Thread arm on the inline path: same byte, not held (as before).
- MEASURING IN (from here until 13g): Options::price_drain_order_old (collect_seal lands an awaits_writer front whenever sealed), arms supdb-drainorderold / supdb-ingestdrainorderold (bench-arms.patch). Twins read: create_drainorderold = Policy{price:true,..default} vs create = default; create_ingest_drainorderold = {partition:false,durable:false,price:true} vs create_ingest {partition:false,durable:false}. Only field: price -> price_drain_order_old.
- bench build: bench-build.log / bench-build.status
- pairs: sh pairs.sh list1.txt -> pairs/, status pairs/status.txt, runner pairs/runner.log (started 08:49:48)

## Pricing (step 5) -- done 08:56 UTC
Commands (OUT=price/drain-order/pairs, via pairs.sh list1.txt, list2.txt):
- m:  sh bench/scripts/pair.sh supdb supdb-drainorderold {10000,300000} --passes load ; same for supdb-ingest supdb-ingestdrainorderold
- st: sh bench/scripts/pair.sh SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1 <same arms> {10000,300000} --passes load
- st2: the durable stamped pairing at 300000 run once more.
Rates (B/A; sitting 1 columns new,old so B/A=old/new; sitting 2 old,new so B/A=new/old): no load or load-shuffled ops cell marked in any table; device bytes identical.
  durable 10k m: load 1.018 / 0.965; shuffled 0.976 / 1.031.  300k m: load 1.015 / 0.965; shuffled 1.021 / 0.957.
  buffered 10k m: load 0.970 / 1.000; shuffled 0.983 / 0.972.  300k m: load 1.005 / 1.021; shuffled 0.961 / 0.987.
  Marks: only counts (freeze_ms, seal_snap_ms, maint_cpu_us, prime us_per_page), one sitting each, opposite signs; coin-level.
Mechanism (drain.py over st*.err, ordered load, medians, us from flush entry; gap = joined - max(fsynced, presynced), the landing after the seal thread's return, the only phase the field changes):
  durable 10k  gap new/old 76/75 (s1), 91/68 (s2)
  durable 300k gap 145/176, 157/152; st2 139/134, 117/181
  buffered 10k gap 79/64, 65/67;  buffered 300k gap 176/172, 176/186
  The writer's presync returns 0.1-4 ms after the flush's entry and the seal thread's trailer fsync 1.5-29 ms after it: in 207 of 208 stamped drains the presync (and so the Drain ask) came before the seal thread returned, so the durable landing runs from join_seal in the ask in BOTH arms; the field's branch in collect_seal fires only where the seal thread returns first (1 drain in 208, writer descheduled ~2 ms).
  strace -f -c over load_only 300000 1 (strace/c-*.txt): identical counts of every file syscall arm against arm (fdatasync 303/303 durable, 7/7 buffered; fsync 6/6; pwrite64 17/17, 15/15; linkat, rename, unlink equal); futex differs (666/702, 221/240). -tt (strace/t-*.txt): the writer's presync fdatasync (fd 7, 0.1 ms) ends ~24 ms before the seal thread's fsync of the run's file in both arms; the manifest's slot writes follow that fsync in both.
  One cell read worse in all four sittings: the durable ordered load at 300k, the drain's 'readable' stamp (seal thread's finish, link and readable signal) new longer by ~0.6-2.6 ms (drained +1.4, +2.9, +0.6, +1.8 ms); the buffered pair read the same stamp new SHORTER in both sittings. That phase precedes every instruction the arms differ in (the ask is in before it in both), so it is read as not the change's; not verified by a control pairing of an arm against its own twin.
Verdict: no cost resolved in any rate; the phase that holds the change level; one whole-drain cell (durable 300k ordered) longer in both sittings of two pairings, argued outside the change. Recorded as "unresolved". Nothing of the order changes; the "ok" alternative is not built.
- 13g done: measuring.patch saved; field and arms out; src/db.rs = keep copy
- step 4: neigh.sh running -> neigh.status, n-*.log, after.log
- 08:59:39 item-21 debug assertion taken out (a check nothing reaches); comment kept in flush. seams test gained a 'flush still held' check. keep/ updated (src, tests, CLAUDE.md). neigh.sh started BEFORE this edit: durable's n-log may be of the earlier tree; rerun durable after.
- step 7: mutants running (mut.py -> mut-results.txt, mut-*.log); fixed file restored by sum after each

## Final (09:5x UTC) -- unit done, uncommitted
State: tree = keep/ (src_db.rs.fixed, tests_durable.rs.fixed, CLAUDE.md.fixed; sums in keep/sha256.txt). Measuring field and arms out (measuring.patch). TMPDIR dir removed.

What was wrong / done:
- 17: the drain's durable landing serialised behind the writer's presync and the Drain ask's hop. PRICED (above): no cost; the ask is in before the seal thread returns in all but 1 of 208 stamped drains, so both shapes land from join_seal in the ask. Nothing of the order changes; the "ok" alternative not built. CLAUDE.md's presync paragraph amended (it said "no wait anyone made before" unpriced; now says how it was priced and the rule).
- 19: presync seams load-first: Shared::fail_presync + presync_hold (AtomicBool each; the writer path did a load of the hold and an unconditional swap) -> Shared::presync_seam: AtomicU8, PRESYNC_FAIL | PRESYNC_HOLD; each presync arm does one Acquire load, nonzero -> #[cold] Db::seamed_presync (hold wait where `holds`, then fetch_and of the fail bit). The writer path loses an atomic RMW; the inline arm's thread path is a load as before (Relaxed -> Acquire, same instruction on x86).
- 21: unreachable as the code stands: seal()'s ordered branch ends in close_direct, which waits for room before it takes the run and then cannot fail; close_run answers nothing. So every awaits_writer hand-over is followed by the flush's Drain ask. Held by a comment in Db::flush. A debug assertion of it was built first (seal() Err => no awaits_writer job queued, inline only) and fired in the gate: a flush AFTER a presync failure finds the earlier job at the front of the stopped queue -- correct (the queue is stopped, the window shut for good), so the assertion was wrong and taken out (a check of an unreachable state is a line nobody can hold).
- 12: debug_assert!(!awaits_writer || draining) in Db::hand_seal; M9 (deleted) survives by construction, M10 (a caller that hands outside a drain) is killed by it.

Tests (tests/durable.rs, section "drain-order"): a_drain_names_its_runs_segment_durable_before_it_answers(+_inline), a_drain_lands_its_run_under_a_cipher, the_presync_seams_hold_and_fail_apart. All four pass before the change (before.log) and after: not evidence of a fix (no behaviour changed); the seams test kills M1, M3-M6.
Neighbours: after.log whole suite --no-fail-fast exit 0 (20 binaries). The first neighbour pass (n-*.log) had durable red at the item-21 assertion (above) and cipher red once at the_builders_rest_at_every_block_of_a_store_of_wide_values (pager peak 8.4 MB over a 2 MB budget); cipher then passed 4 times (n2-cipher, after.log). Not attributed; the pager is untouched.
Lint: scripts/check.sh lint green after sh scripts/fmt.sh (tests formatting).
Not verified: the 'readable' stamp difference at durable 300k (control pairing of an arm against its own twin not run); Encryption::Kernel not exercised; no callgrind count of the presync path (one load replaces a load + an RMW, read off the source); bench/target/release/bench and examples/load_only left built from the measuring tree.

## Repair round 1 (started 2026-11-18)
- Blocking: none. Extra mutants (9, two duplicates): fail bit not consumed; inline thread held; hold arming by store; lifter's lift deleted (hangs today); hold loop after the fail check; hold arming sets FAIL too; hold loop `if` for `while`.
- Plan: (a) the seam tests get a watchdog that lifts a forgotten hold and fails the test (no hang); (b) a test arming fail then hold, extent None while held, the failure fires after the lift (order + store mutants); (c) a test that the inline arm's presync is not held; (d) the fail bit's consumption: a store whose presync failed presyncs no more in that open (test), so the consumption is unobservable -> taken out (load, not fetch_and), docs say so; (e) Db::failed_presync_extent moved to Reader so the lifter can read it (test seam, cold).
- No success-path change planned -> no re-pricing (seamed_presync is cold; the success path's single load unchanged).
- keep/ = round-0 fixed copy until this round's edits pass; then updated.
- Round 1 progress: (d) was WRONG as planned: a test showed a store whose presync failed DOES presync again (its stopped queue still takes a run's close while the frozen list has room; a second run's file, a new inode, was presynced). So the consumption is observable and stays (fetch_and); test an_injected_presync_failure_is_taken_once(+_inline) holds it (no re-arm: extent stays the first; re-armed: a new inode). (e) done: Reader::failed_presync_extent. New tests: a_held_presync_takes_the_failure_armed_before_the_hold_after_its_lift, the_inline_arms_presync_is_not_held, taken_once x2; seams test + watchdog (ps_watchdog lifts after 30 s and the test fails). All 5 pass (r1-new.log). keep/ updated (sums in keep/sha256.txt).
- mutants round 1: mut2.py -> mut2-results.txt, mut2-<name>.log; restores keep copies (src and tests) by sum after each.
- round 1: full suite -> r1-after.log/.status, lint -> r1-lint.log/.status (started 10:11)
- Round 1 DONE: tree = keep/ (sums match). Full suite --no-fail-fast exit 0, 20 binaries, 1136 tests (r1-after.log); lint green (r1-lint.log); fmt --check green. TMPDIR removed. Mutants X1-X10 all killed (mutants.md, round 1). No re-pricing: the round's src changes are test seams only (failed_presync_extent and fail_next_presync moved from Db to Reader, reached through Db's Deref; the cold seamed_presync's docs); no success-path instruction changed.

## Repair round 2 (started 2026-11-18, after round 1's keep/)
- Blocking: none. Extra mutants E1-E5 (store(0) lift; writer tests FAIL bit only; draining after seal; store(0) after presync; collect_seal gate && draining).
- Plan: (a) a held presync says it waits (PRESYNC_WAITING bit set/cleared inside the cold seamed_presync; Reader::presync_hold_state() -> (hold armed, a presync waits)) so the lifters wait for the writer AT the hold, deterministic, and E2's kill is that wait's, not the 100 ms sleep's; (b) the inline test asserts the hold it armed still stands after an unheld presync (E4); (c) E5 equivalent by the item-12 assertion (reason in mutants.md); (d) E3 run whole and with a_drain_names_* alone.
- Success path: the writer's and the thread's `presync_seam.load != 0` unchanged; WAITING set only inside the cold path. No re-pricing.
- keep/ = round 1 copy until this round's edits pass.
- round 2 mutants: mut3.py -> mut3-results.txt, mut3-<name>.log; restores keep/ by sum after each (keep updated with round 2 edits, sums in keep/sha256.txt)
- round 2: mutants E1-E5,O1-O5 done (mut3-results.txt); fmt clean; full suite -> r2-after.log/.status, then lint -> r2-lint.log/.status
- Round 2 DONE: tree = keep/ (sums match). Full suite --no-fail-fast exit 0, 20 binaries (r2-after.log); lint green (r2-lint.log); fmt clean. TMPDIR removed. Mutants: E1-E4, O1-O5 killed; E5 survives, equivalent by the item-12 assertion (mutants.md round 2). src changes this round: PRESYNC_WAITING set/cleared inside the cold seamed_presync, Reader::presync_hold_state (test seam), and two orphaned doc paragraphs (hold_seal_landing's, fail_next_landing's) that sat on seamed_presync's doc moved to their functions. The writer's and thread's success-path `presync_seam.load != 0` unchanged -> no re-pricing; no reviewer showed a pairing missed the mechanism.

## Repair round 3 (started 2026-11-18)
- Blocking: none. Extra mutants R1-R8 (brief list in order). keep/ = round 2 copy (sums match at start).
- round 3 mutants: mut4.py -> mut4-results.txt, mut4-<name>.log; restores keep/ by sum after each. R3 and R6 are one mutant (run as R3); R4 run two ways (a: ask only after a run, literal; b: no ask after a run, the job never asked for); R8 two ways (a: reorder, b: wedge deleted).
- Round 3 STATE at hand-back: mut4.py (nohup) still running R1 first; the tree holds a mutant of src/db.rs until the runner restores keep/src_db.rs.fixed by sum after each (it asserts the sums). No verdict recorded yet in mut4-results.txt. No src change this round beyond the runner's mutants; no re-pricing owed (no success-path change, no reviewer showed a pairing missed the mechanism). Still owed: mut4 verdicts into mutants.md, then full suite + lint with the tree back at keep/.
- Round 3 mutants DONE 11:05 (commit unit): R1-R5, R7, R8b killed; R8a survives, equivalent. Tree back at keep/ (sums match). Next: gate (check.sh build test lint) -> gate.log, then commit.
- COMMITTED c07c95a15339b078887aca3ce43ab7feca8e08f3 after gate green (gate.log)
