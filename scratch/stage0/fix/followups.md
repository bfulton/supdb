# Follow-ups found at check-ins (orchestrator's list; not in any unit's brief)

## 2026-11-17 04:35 UTC, check-in during log-at-open's second commit step

1. Copies of a live store in tests/durable.rs (copy_store, ~50 call sites). The log-at-open gate went red
   3 runs in 10 of the whole durable binary: a_rotation_after_a_reopen_leaves_the_replayed_file_loose copied
   the store while a seal its commits had handed could land; the copy's slot named a segment the copy did
   not hold. Fixed for that test (hold_seal_landing across the commits and the copy). Not audited: every other
   copy of a live store. Known candidate: appends_below_the_greatest_read_back (delete-max section, line ~627):
   seal_bytes 4<<10, l0_trigger 2, 94 appends committed then copy_store with no hold -- a seal or a background
   merge may be in flight. A merge's landing has the same shape (outputs renamed in, slot written, inputs
   removed). Wanted: one helper that copies a live store under holds of the seal's and the merge's landing
   (so no test can forget), each call site moved to it or shown to have nothing in flight, then the whole
   durable binary stress-run busy (4 processes at a time, 40+ runs each), and each mutant credited to a test
   whose copy changed read again (CLAUDE.md: a mutant's kill is read again once a race under the killing
   test is found).

2. Owner item from log-at-open: a log file whose first sector is zeroes with frames behind it is cut to
   nothing at the open (before: refused "not a supdb WAL"). Within the crash model those frames were never
   synced. Media damage to a synced first sector reads the same and loses acknowledged frames silently.
   ROADMAP item added by the unit: set such a file aside when a commit frame behind the sector verifies.
   Tell the owner; ask whether the set-aside should be built now (plain log: the CRC seed is from the id in
   the file's name, so frames verify without the header; sealed log: the wrapped key is in the header, so
   nothing behind it can be verified).

3. Not counted under callgrind per unit; the run's count:callgrind step does it once for all units.

## From unit sync-run (read at the 06:09 check-in, from the unit's kept copy keep3/)

The unit fixes `Db::sync` for an OPEN ordered run (log synced whatever table is live; the
run's held tail written and its file synced, cold `Db::sync_run`). Both first-round
reviewers found the same hole on the path beside it, and the repair chose to name it in the
docs and ROADMAP rather than build it, because either fix adds a wait or a syscall the
run's rule 1 says must be priced first. Neither is to stay a documented exception: each is
a unit of the pricing workflow, built with its failing test first.

4. **sync over a run closed into a seal in flight** (plain store, `EveryN`). A run closed
   by any write it cannot take, by `Db::seal` or by the commit's seal at the threshold goes
   to the seal thread with its commits since the last due sync unsynced and its held tail
   unwritten; `Db::sync` then answers `Ok` (direct is None, `unsynced = 0`) and a kill or a
   power cut before the seal's durable landing loses them while a later log commit
   survives. Reproduction: `sync-run/round2/closed-run-test.rs` (24/24 cases fail on the
   fixed tree). Fix to price: a bool set in the cold close when the run closes with a
   count of unsynced commits; `Db::sync` waits for that seal's durable landing when it is
   set (or: the close writes the tail and keeps the file for sync's cold fdatasync --
   run-tail-kill's P1). Cost when nothing is owed: one load and branch inside `sync`.
   Then the SyncPolicy / Db::sync / EveryN docs lose their exception paragraphs.
5. **a run's due commit beside log frames no sync covered** (plain store, `EveryN`; by
   reading also `Always` after a reopen, where the frames were synced before the kill and
   only the accounting says otherwise). `commit_direct`'s due branch syncs the run's file
   and not the log, so a power cut can keep the run's commits and lose older frames of
   tables frozen before the run: not a prefix. Reproduction: `sync-run/due-commit.log`,
   `due-commit-test.diff`. Fix to price: a bool noted once per run at its open when the
   log owes, asked in the due branch, with a cold path that syncs the log first. Check
   whether workflow C (the shared log/run sequence, decision 1) makes this fall out before
   building it twice.
6. Cosmetic, for the unit's commit or a tidy after it: the SyncPolicy doc paragraph has
   three lines over the comment width after the round-2 edit ("commit, whatever that
   commit answers. A write of the run's file the device refuses is answered by ...").
7. Not priced in the unit: the buffered / nodrain bench arms' closing `sync` now pays the
   run's write and fdatasync (it is the durability the call promises; the cell that moves
   is the load's closing sync). Read it in the callgrind count and in the pricing
   workflow's first pairing.
8. From sync-run's third repair round (keep4): `Db::sync`'s log gate lost its
   `|| self.wal_dir_unsynced` clause as unreachable (mutant F9 survived): a loose rotation
   sets the flag with its file pushed onto `loose_wals`, which only `sync_wal` empties and
   only after the barrier; a chained rotation pays the barrier at once or keeps its
   failure, which `commit_staged` answers first. I read every site that changes
   `loose_wals` and `wal_dir_unsynced` and the argument holds. Two things left: the same
   clause still stands in `commit_staged`'s due gate (one of the two is wrong to differ),
   and nothing holds the invariant itself -- a debug assertion where the flag is read
   (`!wal_dir_unsynced || !loose_wals.is_empty() || lost.is_some()`) would, and a future
   change that broke it would otherwise skip a directory barrier silently.

## seal-sync, as its first review left it (07:30 UTC 2026-11-17; repair round 1 in flight)

To read at the unit's commit, not built by me:

9. Two checks on success paths the reviewers held under rule 1: `Db::sync`'s load of
   `seal_wedged` (every sync, one with nothing owed included, so beside no syscall) and
   `Maint::publish`'s (beside the slot's pwrite and fdatasync). Mutant S1 says the sync's check
   is load-bearing for the tests. Read what the repair did: folded into the writer's kept
   answer (`pending_err`), or taken out and reported. If taken out, ask what a `sync` answers
   after a seal failed for a direct run in the clear, whose records are in no log: `Ok` there
   is a wrong answer and not a cost question, so it cannot stay out; price the fold instead.
10. Behaviour for the owner: a write that ends an ordered run whose close cannot be made
    (the seal queue stopped, the frozen list full) is dropped and the refusal kept, every
    commit, sync and flush answering it until the store is reopened; before, the write
    panicked. The first version did the same for any error of the segment work with the list
    full, a merge's included, which the crash reviewer blocked; check the repair keeps the
    drop to a stopped queue and has the test the reviewer asked for.
11. Merges and promotions still start while the queue is stopped; their publish is refused
    and their outputs swept at the open. Wasted work, safe; in ROADMAP by the unit.
12. `awaits_writer` jobs land durable only at the writer's Drain: the reviewer's debug
    assertion (awaits_writer implies draining) is worth having.

## seal-sync after its first repair round (09:00 UTC 2026-11-17; second review in flight, cost reviewer asked for repairs again)

Item 9, as the repair settled it: the check in `Db::sync` is taken out, not folded, and the
check in `Maint::publish` is folded into `seal_between_phases` (a `Maint`-local `seals_stopped`).
A `sync` answers nothing of a stopped queue. Read against the three cases:
- a hashed table's seal failed: its writes are in the log, which is never retired; `Ok` is right.
- a run's close failed under `Always`: every commit synced the run's file; the temp name stays
  (no manifest names the segment) and the open rebuilds from it; `Ok` is right.
- a run closed under `EveryN` with commits no sync covered, and its seal failed: those commits
  are durable nowhere and never will be in this open, and every `sync` answers `Ok`. This is
  item 4's gap made permanent by the failure. Every `flush` answers the failure, so a caller
  that flushes learns; one that only syncs does not. The unit's docs say so plainly. Item 4's
  unit has to close it: a `sync` over a closed run whose seal has not landed either makes the
  run's records durable or answers; over a stopped queue it answers the failure. Price the
  check where item 4's own check goes (the count of a closed run's unsynced commits, which is
  zero under `Always` and for every hashed table), not as a load of `seal_wedged` on every sync.

13. `writes_go` reads `in_seal` (Acquire) before `take_fresh_mem`, once at the first write after
    a `sync`'s hand-off. The cost reviewer let it stand as rare. Read it in the callgrind count
    by function (`count:callgrind`), and fold it into `take_fresh_mem`'s own read of the count
    if it shows.
14. After a stop a finished merge is held for good and the segment work's `collect` enters on
    every poll for the rest of the open (a pin and a bump of `held_landings` each time). Free
    until a failure; latch the merge out once the queue is stopped.
15. The words race the second cost review names: on a failure arm `in_seal` is decremented
    before the wedge is set, so a writer can read a count of zero and no wedge and answer
    "did not land" words that name neither the wedge nor its cause, kept as lost. Durability
    unaffected. Check the second repair wedges before the decrement, and that the `seals == 0`
    arms are then either unreachable (taken out) or held by a test.
16. The second cost review's blocking item is the docs: "no manifest is written" is false for a
    writer-side presync failure before the awaiting job's readable landing (a merge or a
    promotion can still publish; nothing of the failed seal is named); under the inline arm
    every `Db::seal` and every commit at the seal threshold answers the failure with room in
    the list; `settle` answers it at every call and no doc or test says so. Check what the
    second repair made true and what it restated, and that settle's and the inline arm's
    answers are tested by their words.
17. The durable landing of a drain's run job no longer overlaps the writer's presync (it runs
    inside the writer's Drain ask, after the presync). Correct, and a serialisation the drain's
    stamps should price in the pricing workflow (`SUPDB_DRAIN_STEPS=1`, `bench/scripts/pair.sh`
    against `3df85d22` on the ordered load at 10k and 300k).

## seal-sync after its second repair round and third review (10:20 UTC 2026-11-17; third repair in flight, then the commit step with no further review)

Third review: crash lens passed with no blocking item; cost lens asked for a docs repair (below). What the earlier items became:

- 9 (the sync's answer): unchanged in code. The docs said "a sync answers nothing" flatly, and that is false once a write has been dropped behind the stopped queue with the frozen list full: a write that ends a run, or the first write after a `sync`'s hand-off under `adaptive_shape` (`writes_go` -> `take_fresh_mem` -> `keep_lost`). From that write on every commit, sync and flush answers. Round 3 restates SyncPolicy, Db::sync, the crash-discipline paragraph and ROADMAP to name both drops. The EveryN closed-run hole stands as written above: item 4's unit closes it.
- 13 (writes_go's `in_seal` load): gone. Round 2 put `stop_seals` before `in_seal.fetch_sub` at the three failure sites, which made the `seals == 0` arms of `writes_go` and `leave_direct` unreachable, and they were taken out with their loads. Two arms of the same kind are left: `land_seal`'s `if seals == 0 { "the frozen table's seal did not land" }` (Away arm) and `take_fresh_mem`'s `seals == 0 && !wedged`. Round 3 runs them as mutants; check at the commit whether they were taken out, and if they stand, why.
- 14 (a held merge polled for good): stands.
- 15 (the words race): closed by the order above; held by reading only (R7a/R7b cannot be timed).
- 16 (the docs' blocking item): made true, not restated: `Maint::seal_phases_due` sets `seals_stopped` in the branch that already loads `seal_wedged`, so a stop the writer raised (its presync) holds the segment work from its next look. Tests: `a_presync_that_fails_before_its_seals_publish_stops_every_manifest` (fails on round 1's code: slot (5,42) against (4,42)), and round 3 adds an inline variant. Settle's and the inline arm's answers are tested by their words (`a_stopped_queue_is_answered_by_every_seal_inline`, settle answers in four tests).
- 17 (the drain's durable landing after the presync): stands, and both lenses name it as the unit's one unpriced reorder. The cost reviewer's alternative, for the pricing workflow: an "ok" the writer stores after a successful presync, which lets `collect` land the job durable without waiting for the Drain ask's hop. Also in that window: merges finishing beside a drain are held until the ask.

New from the third review:

- 18. The rename arm (`manifest_slots` off, `supdb-ingestmanifestrename`): the seal's directory entries are made durable by the publish's own directory fsync inside `land_durable`, and `land_durable`'s failure counts the seal out without `stop_seals`, so the next landing's publish fsyncs the directory again and names the segments: a retry with a manifest behind it, on an arm the unit did not touch. Either `stop_seals` on `land_durable`'s error under that arm, or an open item in ROADMAP. Check which the commit has.
- 19. `self.shared.fail_presync.swap(..)` in `close_run`'s writer presync is an unconditional atomic read-modify-write on every presync; it predates the unit. The thread arm forty lines up guards the same seam load-then-swap. One line, in the next fix workflow, read in `count:callgrind` first.
- 20. `leave_direct` / `close_direct` now call `land_seal` before `scan_keys` is cleared, `drop_blocks` runs and the uncommitted tail is truncated. Inline that lands earlier seals with the writer's block tables still in place (a `carry_published` of forms about to be dropped?). Neither reviewer traced it. Read it; if it is extra work at an inline run close with a seal in flight, it is rule 1's.
- 21. Liveness on an error path: a flush whose `seal()` fails after `close_run` handed an `awaits_writer` job sends no Drain ask, so the job sits at the front, readable, with the manifest window shut, until the writer's next `land_seal`, settle or flush. No test, no doc.
- 22. Timing in two tests: the "both" presync variant sleeps 500 ms for the seal thread to return (on a slower host it weakens, never flakes red); the P1 writer-arm test waits up to 2 s for a landing that would come without the fix.
- 23. Wording left loose: "every seal" inline, where a `Db::seal` of an empty table answers `Ok` before `land_seal`; an inline write that ends a run over a stopped queue with room leaves the failure for the next ordinary commit, which the docs' list does not name; "has written its batch to the log first" for a direct run, whose batch is in its own file.
- Survivors argued and to re-read at the commit: R5, R6 (the close's own flag, by timing), R10 (equivalent through `done.is_ok()`, R10b killed), X6, X8, X11a.

## seal-sync as committed (11:45 UTC 2026-11-17; local 2744a21a, on the remote as 02b7a440)

Read against the commit's engine diff (seal-sync/commit-engine.diff):

- 9: the docs name both drops (SyncPolicy, Db::sync, CLAUDE.md, ROADMAP). The EveryN closed-run hole stands for item 4's unit.
- 13: closed. Both `seals == 0` arms (`land_seal`, `take_fresh_mem`) are gone with their `in_seal` loads; the writer's two waits for room read the stop flag and nothing of the count.
- 18: stands. `land_front` counts the seal out and returns `land_durable`'s error with no `stop_seals`: a manifest publish that fails (slot write or its fdatasync; rename arm's directory fsync) leaves the segments published and fsynced, unnamed, the queue going, and the next landing's publish writes a manifest again. Whether that is a retry over a failed fdatasync of the same slot file is the manifest's own question -- read slot-choice's result first, it touches the slots.
- 19: stands. The writer-thread presync still does `fail_presync.swap(false, AcqRel)` unconditionally (the presync thread's arm was made load-then-swap), and has a new `presync_hold` load before it: a test seam's load and an RMW beside an fdatasync, once a drain. Make both load-first in the next fix workflow; read `count:callgrind` for whether a drain shows them.
- 20: the reorder is real in `close_direct` (`land_seal` before the `scan_keys` clear and `drop_blocks`) and in `leave_direct`. `land_seal` does nothing while the frozen list has room (`rehold`, one length compare), so the carry of forms about to be dropped can only happen where the list was full and a landing was waited for: backpressure, not the common close. Low; read once with the lag counters if a pairing ever shows a drain's close slower.
- 14, 15, 17, 21, 22, 23: as written above; nothing in the commit changes them. 17 (the drain's durable landing after the writer's presync and a message hop) is the unit's one unpriced reorder: price with `SUPDB_DRAIN_STEPS=1` in workflow B.
- New, 24: E5/E10 are killed only through `Maint::publish`'s debug assertion on the inline close; on the segment work's own thread the assertion fires there and the close swallows it (the unit's not_verified). A debug build that propagates a segment-work panic at the close would make those kills real; a wider change to `Db::drop`, for the next fix workflow to weigh.
- New, 25: the unit's tests carry sleeps (500 ms in the inline presync test and the "both" variant, 2 s wait in the P1 writer-arm test, a 60 s watchdog proven under two mutants only). They can weaken on a slow host and cannot fail spuriously; replace with holds where a hold exists.
- Survivors re-read: E6/E7 are now the code; E9 (awaits_writer by `self.draining`) equivalent while flush is the only draining caller; R5, R6, R7a/b, R10, X6, X8, X11a as argued in mutants.md -- none read as a missing test of a reachable state.

## slot-choice, read at its first repair round (13:10 UTC 2026-11-17; kept copy r1/keep, snapshot 0e0ed0f9)

- 18, read against the kept copy (to be read again at the commit): a manifest publish whose fdatasync fails
  returns the error with `SlotChoice::landed` not called, so the durable slot stays the other one and the
  next publish writes the same slot again, the whole image from offset zero, and fdatasyncs that. That is
  a rewrite and a sync of the rewrite, not a second fsync believed over pages the first one dropped: a
  success covers every byte of the newer image, and the tail of a longer older image lies past the length
  the header names. So the retry item 18 worried about is sound for the slot once this unit lands
  (`a_slot_whose_fdatasync_failed_is_not_the_one_a_publish_keeps_off`, + cipher, holds it). What item 18
  still leaves: the queue goes on past a failed publish, which is the "store that goes on" ROADMAP item
  and no hole by itself, since the seal's segments were fsynced before the publish was tried.
- 26 (new, low): a slot write that fails after `write_slot` gave an owed entry up loses the `owed` answer,
  so the log files that waited behind the owed slot retire at the next `land_durable` and not at the next
  publish (reviewer's note; kept longer, never early).
- 27 (new): the open's added fdatasync of the slot it takes is one syscall an open, of nothing where the
  slot is clean; not timed (the run forbids timing runs). Price it in workflow B beside the others: an
  open's time before and after, plain and after a kill.
- 28 (new): the kpageflags tests (`an_open_makes_the_slot_it_takes_durable`, + cipher) run nothing without
  root; round 1 adds `Options::fail_open_slot_sync` and a root-free test. At the commit, check that the
  root-free test kills S3a and O6 (r1/mut-results.jsonl) and that the kpageflags test says so loudly
  where it does not run.
- 29 (new, cipher): a torn pending envelope under a cipher may poison the pager at `pending_whole` and
  refuse the whole open where it should fall back to the slot before (reviewer's note, shape older than
  the unit, no test builds it). For the manifest-pending rewrite or ROADMAP.

## slot-choice, read at its third repair round (14:35 UTC 2026-11-17; kept copy r3/keep, snapshot 3480d8a7)
- 18: unchanged (sound; the store-that-goes-on item stays ROADMAP's).
- 26: unchanged (owed answer lost on a failed write: kept longer, never early).
- 27: stands, and is one open(2), one fdatasync and one close(2) of the slot file at every open, the rename shape included; reviewer suggests keeping manifest_read_all's handle. Price in workflow B.
- 28: closed by round 2 as far as a kill goes -- the root-free seam test kills S3a, O6, open-sync-only-if-pending and E1 (the seam's error names the inode). What remains: an_open_makes_the_slot_it_takes_durable (+cipher) prints "not run" and passes without root, a verdict it did not earn; make it #[ignore] and run it by name under scripts/check.sh fscrypt's job, or drop it. Check at the commit what round 3 did.
- 29: stands (a torn pending envelope under a cipher may poison the pager at pending_whole).
- 30 (new, cost, owner should know): a publish whose fdatasync returned stores one word (SlotChoice::durable) into Maint's own field -- a store, not a load, beside an fdatasync; the fix's minimum state. Four seam loads beside a publish's write and sync (watch, tear, fail, synced record) could fold into one byte as the run writer's fault byte does: one load a publish. Read the callgrind count for Maint::publish at the end.
- 31 (new): no slot test ran with the segment work inline until round 3 added an "inline" arm to every_arm; check at the commit that it kills a landed-nothing mutant with wake none (reviewer's X-mutant), since inline the owed slot stays owed until the next publish, a settle or the close.
- 32 (new, pre-existing, ROADMAP beside dm-flakey): rename shape -- after a kill between a publish's rename and its directory fsync the open fdatasyncs `manifest` and not the directory, then sweeps; rests on the journal ordering the rename before the unlinks.
- 33 (new, ROADMAP beside dm-flakey): errseq across a reopen -- a slot whose fdatasync failed, the process exits with no later publish rewriting it, and the next open's fdatasync on a fresh descriptor returns Ok over bytes the device never got.
- 34 (new, low): the pending landing discards write_slot's owed answer; R1-pending-owed-not-taken survives because no state the tests build reaches an owed slot at a pending landing (reasoned, not proven). Assert it (debug_assert the entry is None at a pending landing) or build the state.
- 35 (new, docs): CLAUDE.md's slot shape entry narrates the unit's own review rounds about code that never landed (the owed entry's two words, the seam that failed whichever file); cut to the rule at the critic's pass. Its cipher sentence ("the open took the slot before it") against arm-dbg.log (no slot remained; refusal was "wal sequence gap"): check the commit's wording.
- 36 (new, test): a_store_moved_between_the_manifest_shapes_keeps_its_writes checks reads only, no crash state across a shape change; a guard, not evidence the rename arm is crash-safe under the new rule. a_store_from_before_the_pending_flags_... still places its v2 image by parity (fixture comment owed).
- open-order (next unit) owns the open half of manifest-pending-rewrite: the open takes a pending slot whose segments read whole from the page cache after a kill, writes a slot naming them durable and never fsyncs them. Both reviewers flagged it as not covered by slot-choice's open sync; the slot-choice commit message should not claim it.
