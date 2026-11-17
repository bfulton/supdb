# log-errors (wal-write-fail, wal-sync-fail): notes

Base: HEAD f8d130a8 (delete-max committed on 3df85d22). Uncommitted:
src/db.rs, tests/durable.rs, CLAUDE.md, ROADMAP.md.

## What was wrong
- wal-write-fail. `Wal::write` returned through `?` on a failed
  `write_all` before `written` moved and before `pending.clear()`. The
  failed batch and its commit frame stayed pending, and whatever part of
  the write reached the file sat past `written` with the cursor (or the
  append position) behind it. The next commit's frame took its batch CRC
  over all of pending (the failed batch's commit frame included), replay
  rejected it, and every commit acknowledged after the failure was lost; a
  partial write shifted every frame after it; a rotation after either made
  the open refuse with "wal sequence gap". Same in the cipher arm, which
  sealed all of pending as one record (and, after a partial write, sealed
  it at an offset the file did not have).
- wal-sync-fail. A failed fdatasync of the log was the commit's answer and
  then forgotten: `synced` did not move, and the next commit or `sync`
  issued fdatasync on the same descriptor, which Linux answers with success
  over pages it failed to write back and marked clean. A loose file's sync
  and the directory barrier a rotation owes did the same (the barrier by
  design: "retried by the syncs after it until it succeeds").

## What changed (src/db.rs)
- `Wal::put_back` (cold): on any failure in `Wal::write` -- the plain
  arm's write, the cipher arm's `seal_into` or write -- the batch's commit
  frame (fixed length, 21 bytes) is cut off `pending`, `seq` is given back
  by one, and the file is `set_len(written)` and `seek(written)`. If the
  cut fails the Wal is marked `stray`.
- `Db::lost: Option<io::Error>` -- the kept error, the first after which
  the device may not hold what the store gave it. `Db::keep_lost` (cold)
  keeps it and arms `pending_err` with its answer (`lost_answer`, words
  "... until it is reopened: <original>"). `Db::answer_pending` (cold) is
  called in the existing `if let Some(e) = self.pending_err.take()` branch
  of `commit`, `commit_staged` (so `sync`, `flush`) and `seal`, and refills
  `pending_err` while `lost` stands. `Db::wal_write_failed` (cold) keeps a
  write's error when the Wal is `stray`; it replaces `?` at the five
  `wal.write()` sites (commit, commit_staged, seal x2, log_run).
- `sync_wal`: the live file's fdatasync, a loose file's fdatasync, and
  (through `wal_dir_barrier`) the directory fsync keep their error. The
  barrier is no longer retried (its callers in `rotate_wal` keep it too).
- Docs: SyncPolicy, Db::commit, Db::sync, Db::flush, Db::open, and the
  comments that described the barrier's retry (`wal_dir_unsynced`,
  `rotate_wal`, `seal_table`, `wal_dir_barrier`, `commit_staged`,
  `sync_wal`).
- Seams (#[doc(hidden)] pub, no cfg): `Db::fail_next_wal_write(after)`,
  `fail_next_wal_cut()`, `fail_next_wal_sync()` (zeroes the frames the
  failed sync was to cover, `Wal::lose_unsynced`, as the reproduction's
  seam did), `fail_next_loose_wal_sync()`, `fail_next_wal_dir_sync()`.
  Fields on `Wal` (`fail_next`, `fail_cut`) and `Db` (`fail_log_sync`, a
  bit per site); writer-owned, no atomics. Each check is a load and branch
  right beside the syscall it stands in for (the write, the fdatasync, the
  directory fsync), and the injected error goes through the same error
  line as the real one, so the mutants of the real path are reachable.

## Why this shape for (b)
The failed commit frame dropped, with its sequence number given back, and
not the batch CRC taken from the batch's start. Replay checks the sequence
across every frame, commit frames included, so dropping the frame without
giving the number back would be a gap the open refuses (mutant W2 shows
it); given back, the failed batch's records join the next batch under one
commit frame whose CRC covers all of pending, which is exactly what
`frame()` already computes. The other shape needs a batch start kept on
every commit: a load and a bounds check in `frame`, a load in
`mark_commit`, and a reset in `write` -- per commit, on the success path.
This one costs nothing until a write fails. The joined batch is
all-or-nothing at replay, which is what the memtable does: the failed
batch's writes are in the table uncommitted, and the next commit's mark
makes them visible with its own. A commit retried with nothing new staged
finds `pending` non-empty (the failed records) and `mark_commit` closes it
under a fresh frame. A rotation after the retry carries the sequence on
as it always did (the fourth reproduction now opens).

## Success-path cost (rule 1, (c)) -- counted by reading the diff
- The kept error adds nothing to the success path: its check is the
  existing `pending_err.take()` branch; the new call is inside the `Some`
  arm.
- `?` replaced by `if let Err(e) = .. { return Err(cold(e)) }` at the
  write sites and the sync sites: the same branch, a cold call inside it.
- The cipher arm's `?` on `seal_into` became `let mut r = ..; if r.is_ok()
  {..}`: the same branch.
- Seams: one load and branch per log write (`fail_next.is_some()`), per
  live fdatasync, per loose-file fdatasync and per directory fsync of the
  log, each beside that syscall.
- No syscall, wait, lock or allocation on any success path. Not verified
  by an instruction count: rule 10 forbids benchmark runs and I did not run
  callgrind (owed: `bench/examples/load_only.rs` and `work_probe.rs` under
  callgrind, by function, against origin/main, for `Db::commit` and
  `Wal::write`).

## Tests (tests/durable.rs, section "log-errors")
All 14 failed before the fix (before.log; assertion each failed at below)
and pass after (run2.log, after.log).

| test | fails before at | holds |
|---|---|---|
| a_failed_log_write_loses_no_commit_acknowledged_after_it | "zero, the crash: acknowledged after the failure: k2 reads back once", left [] | a write failed whole; k1 invisible to a handle until the next commit, then once, live (writer and handle), after a kill copy and at a reopen |
| a_log_write_that_failed_part_way_loses_no_commit_acknowledged_after_it | "part-1, the crash: the batch written again: k1 reads back once" | cuts at 1, 5, 20 bytes (all three now run and pass; before, the loop stopped at 1) |
| a_failed_log_write_loses_no_commit_acknowledged_after_it_under_a_cipher | "cipher-0, the crash: acknowledged after the failure: k2" | the cipher arm, cuts 0, 5, 40 |
| a_commit_with_nothing_staged_after_a_failed_write_makes_the_batch_durable | "plain: committed by the commit after: k1 reads back once" | retry with nothing staged, plain and cipher, cut at 5 |
| a_sync_after_a_failed_log_write_makes_every_commit_before_it_durable | "sync: committed before it: k2" | EveryN, sync() as the acknowledgement, seal held |
| a_store_whose_log_rotated_after_a_failed_write_opens_with_every_acknowledged_commit | "the crash opens: wal sequence gap" | rotation after a partial write |
| a_failed_write_that_cannot_be_put_back_is_kept_until_the_store_is_reopened | "a commit after: answered Ok after the log's device error" | cut fails -> kept: commit, sync, flush, commit refused in words; reads go on; close refused; reopen commits |
| a_commit_answered_ok_after_a_failed_log_sync_survives_a_crash | "plain: a commit answered Ok after a failed sync of the log is durable", left [] | reproduction's test, plus the commit after is the kept error |
| ..._under_a_cipher | same, cipher | the cipher arm |
| a_sync_answered_ok_after_a_failed_one_makes_every_commit_before_it_durable | "a sync answered Ok after a failed one made the commits before it durable", left [] | EveryN, plus the second sync is the kept error |
| a_failed_log_sync_is_kept_until_the_store_is_reopened | "the commit whose sync failed: answered with an error that is not the kept one" | commit, commit of nothing, sync, flush, Txn, seal all refused in words; reads go on; close refused and seals nothing (segment names unchanged); reopen: flushed and synced keys back, the lost frame absent, commits and syncs again |
| a_failed_sync_of_a_loose_log_file_is_kept | "the sync after: answered Ok after the log's device error" | loose file's fdatasync |
| a_failed_sync_of_the_log_directory_is_kept | "the sync after: answered Ok ..." | the barrier owed to a sync by policy |
| a_failed_sync_of_the_log_directory_at_a_rotation_is_kept | "a commit after: answered Ok ..." | the barrier a rotation under Always pays itself |

Skeptic's problems, dealt with: k1 is now asserted exactly once (live,
crash, reopen), and asserted invisible to a handle before the next commit;
all three partial cuts run in one test; the seam's `Option::take` on every
write is an `is_some` check; the rotation test's widened window
(`seal_bytes` 64 KiB, `seal_max_pct` 0) is kept and said in the test; the
sync seam's doc says it models a power cut after a later successful sync;
the cipher arms have tests; the loose-file and directory sites have seams
and tests. The rotation test's power-cut claim (whether the new file's
name is durable) is not made: the copy is a kill's.

## Mutants
23 run (mutants.md): every mutant the report lists for these two defects
in the shape built (its (f), "cleared by a flush whose manifest does not
cover", does not apply: nothing clears the kept error but a reopen), plus
seven of my own. 20 killed. Three survived: Sd (clearing on a later
successful fdatasync) and O5 (latest error replaces the first) are
equivalent, since every path to a sync or a keep is behind the refusal;
O6 (chain after the sync) is outside the unit's lines and has no test
anywhere (reported).

## Restated tests
None. No existing test encoded the retry; tests/db.rs (226), cipher (24),
lib (404), fuzz (5), segwriter (19) passed after the first form of the fix
(neighbours.log), and the whole suite with --no-fail-fast after the final
form (after.log).

## Encryption
- Kernel: the same code path as the plain store.
- Cipher: the log is a real file in the directory; `Wal::write`'s sealed
  arm gets the same put-back and `sync_wal` is shared. Tests under a
  cipher: the failed write (three cuts), the retry with nothing staged,
  and the commit after a failed sync.
- `log_run` (a cipher run's frames into the log) goes through the same
  write; `d.logged` moves before the write, and the records put back stay
  pending for the next `log_run`'s commit frame. The run's own state after
  a failed log write (commit_direct) is run-write-fail's; not checked here.
- vault::persist, by reading: a failed `sync_all` of an envelope leaves
  the entry without `disk`, and the next persist writes the whole envelope
  again from the memory file into a file `File::create` made afresh before
  it syncs, so it never relies on a sync of pages already marked clean. No
  analogue of this defect there. What a seal or a manifest does with that
  failure is seal-after-failed-sync's. The pager issues no sync. Memory
  files: an fsync of one is of nothing.

## Left out, and why
- Clearing the kept error at a covering durable landing (the skeptic's
  option): decision (a) says until the store is reopened.
- `seal_into`'s failure (a commit over 4 GiB or nonce exhaustion) goes
  through `put_back` like the write's, by the same line, but no test makes
  it fail.
- The success-path instruction count (above).

## Runs
- before.log: the 14 new tests, 14 failed; the 14 delete-max tests passed.
- run2.log: durable 28/28 on the final code.
- neighbours.log: db, cipher, lib, fuzz, segwriter on the first form of the
  fix (before the sync and write error paths were folded so the seam goes
  through the real error line).
- after.log: the whole suite, --no-fail-fast, final code: every binary ok
  but leak (1) and zpages (3, form-file mapping and advice). Cause: this
  run's TMPDIR was spelled with `..`, the same fragility delete-max's notes
  record (those tests match the kernel's canonical path); after-canonical.log
  reruns leak (1/1) and zpages (136/136) under the canonical spelling.
- clippy.log: `cargo clippy --release --all-targets -- -D warnings`, exit 0.
- fmt.log: `sh scripts/fmt.sh --check`, exit 0 (applied once with
  `sh scripts/fmt.sh`; it changed only this unit's files).
- mutants: mut-run.log, mutants.md, m-*.diff, m-*.log.

## Not verified
- A real errseq or a real ENOSPC: the seams model them (ROADMAP item
  added: a dm-flakey/dm-error device).
- That `set_len` on a prefilled or recycled live log (which truncates the
  preallocated tail on a failure) costs only what it should; no test runs
  a recycled log through a failed write.
- The power-cut half of the rotation case (directory durability of the new
  name after a put-back), and a crash between the failed write and the
  cut-back (the cut is not synced; a kill there leaves the stray bytes,
  which replay ends at, as at any torn tail).
- No callgrind count, no timing.

## Repair round 1 (two blocking review items, ten extra mutants)

1. **The cut-back's `set_len` was held by no test.** Every write-failure
   test made its store with `Db::create`, so the log was written at its
   cursor and the seek alone did the work; a reopened store's log comes
   from `Wal::open_append` -> `vault::open_append`, which is `O_APPEND`,
   where only `set_len` keeps the next write from landing behind the stray
   prefix. `a_failed_write_is_written_again` takes `reopened`: the store
   is made, k0 committed, a kill's copy taken and opened as the store
   under test (asserted `levels() == (0, 0)`, the log alone holding k0),
   then the failure. New test
   `a_log_write_that_failed_part_way_in_a_reopened_store_loses_no_commit_acknowledged_after_it`
   (cuts 1, 5, 20), and the cipher test runs cuts 5 and 40 reopened as
   well. These pass on the round-0 fix (it had the set_len); they are
   evidence through mutant E1 (set_len deleted, seek kept), which they
   kill and nothing else did.

2. **A cipher run's retried commit or sync answered Ok without writing
   the batch.** `commit_direct` counts a batch the run's (`d.committed`,
   `mem.commit()`) before `log_run` writes it to the log; after a failed
   log write a commit or a `sync` with nothing new for the run returned at
   `committed == mem.len()`. Fix (1) of the review: that early return,
   taken only when there is nothing to commit, also asks
   `mem().after & RUN_LOGGED != 0 && !wal.pending.is_empty()` and, if so,
   closes the put-back frames through `log_run_again` (cold): `log_run`
   with no new entry, synced whatever the policy, since the failed commit
   may have owed the sync and its count (`unsynced`) is spent by then.
   `wal.pending` non-empty under a run can only be the run's own frames:
   `direct_can_open` opens a run only over an empty log. `d.logged` moves
   before the write on purpose (commented): the frames stay staged and the
   next call closes them under its own frame, never staging them twice
   (mutant E5). Success-path cost: none -- the added test sits inside the
   "nothing to commit" branch, which no commit that commits anything
   takes; a commit or sync with nothing new in a cipher run pays one load
   of `mem.after` (already loaded for the table) and one of
   `wal.pending.len()`. Not counted by callgrind.
   Tests (fail before at the data, not at a counter): 
   - `a_runs_batch_whose_log_write_failed_under_a_cipher_is_written_by_a_commit_of_nothing`
     before: "commit: the batch written again: a1 reads back once", left [].
   - `..._written_by_a_sync` before: "sync: the batch written again: a1
     reads back once", left [].
   - `..._written_by_the_runs_next_commit` passed before (the normal
     `log_run` closes the staged frames with the next batch): not counted
     as evidence; it holds E5.
   Each also asserts that a commit and a sync with nothing new in a
   healthy run write nothing to the log (`direct_runs` unchanged; holds
   E9 and M3), cuts the crash copy's log to `wal_durable`'s synced length
   (a power cut; holds M2), and reads the closed store. before-r1.log.

3. **Stray-write keeps at each writing call** (E2, E3, E4, E6 would have
   survived): `a_stray_write_is_kept` with a write and a cut both failed,
   through `sync`, `flush`, `seal` (both `seal_rotates_wal` arms), and a
   cipher run's commit; the call itself answers the kept words (E6), the
   commit and sync after it too, reads go on, the store reopens. The
   existing stuck test's first assertion is now `kept(..)` rather than
   `is_err()` (my own test from round 0, tightened, not an existing
   test of the repository). These pass on the round-0 tree; they are
   evidence through E2/E3/E3b/E4/E6 only.

4. Docs: `Db::commit` says a run's batch under a cipher is written again
   too; CLAUDE.md's "A failure that left the buffer..." entry amended with
   both findings and their corollaries. SyncPolicy's sentence ("the next
   commit writes it again with its own") is now true on the cipher run's
   path.

Not built / not mine: `Db::sync` under a cipher run with `EveryN` skips
`sync_wal` (`!self.mem().ordered` gate) even when `log_run` left frames
unsynced; that is sync-ordered-buffered's P3 (drop the gate). The retry
here syncs regardless (`log_run_again`), so the failed-write case is held
under every policy; the plain `EveryN` unsynced-frames case is that
unit's. A run left by `leave_direct` while its failed frames are staged
(the frames then precede the tail's in the log and close under the next
hashed commit's frame) was checked by reading the replay's partial-cover
skip only; no test.

Runs this round: before-r1.log (new tests against the round-0 tree),
run-r1.log (durable 36/36 after), mut2-run.log, after-r1.log (lib,
durable, db, cipher, fuzz, segwriter, --no-fail-fast), clippy-r1.log,
fmt-r1.log (`scripts/fmt.sh --check`, exit 0; applied once, fmt-r1-apply.log).

## Repair round 2 (one blocking item, ten extra mutants)

1. **Blocking: the SyncPolicy doc promised a failed batch invisible until
   the next commit, which a cipher run breaks.** True: `commit_direct`
   marks the run's table (`mem.commit()`) before `log_run` writes the log,
   so a run's batch whose log write failed is visible to a `Latest`
   handle from the failed commit on, on no device. Took the cheap fix the
   reviewer named: the docs say so. `SyncPolicy` names the exception (the
   run's batch is visible from the failed commit, durable from the call
   that writes it again: a commit with or without anything new, or a
   `sync`); `Db::commit` says the log's batch is invisible until the next
   commit, "except an ordered run's under a cipher". Not built: moving the
   run's mark after `log_run`, which changes the run's documented "a
   reader sees the batch before the log holds it" (as a plain run's
   before its sync) and would need pricing. The runs test now asserts
   the documented behaviour (a claimed handle reads a1 after the failed
   commit) so the doc is held to the code; the put helper's comment says
   it is about `put`, whose batches go to the log alone. CLAUDE.md's entry
   gains a sentence on the visibility and the corollary "a contract
   written for a failure is read against every path that reaches it".

2. **`RUN_LOGGED` half of commit_direct's early-return test taken out**
   (reviewer's mutant, probably equivalent): frames pending under an
   ordered table are only a logged run's put-back -- `direct_can_open`
   needs an empty `wal.pending`, and `append` below the greatest, `put`
   and `delete` all call `leave_direct` before staging a frame. The test
   is now `!self.wal.pending.is_empty()`, one load fewer on the
   nothing-to-commit path; the comment says why.

3. **Seal-failure seam** (reviewer's X10: `if r.is_ok()` -> `if true` in
   the cipher arm). New `Db::fail_next_wal_seal()` (doc(hidden), no cfg)
   sets `Wal::fail_next` to `SEAL_FAILS` (`usize::MAX`); the cipher arm
   asks `self.fail_next == Some(SEAL_FAILS)` before `seal_into` and
   answers an error in its place. New test
   `a_log_record_that_fails_to_seal_loses_no_commit_acknowledged_after_it`
   (the put helper under a cipher, the seal failing). The first form read
   `fail_next.is_some()` once into a local and reused it for the write's
   seam, which made the write's seam run under X10 and mask it (X10
   survived in mut3-run.log); asked apart, X10 dies. Cost: one load and
   compare of `fail_next` in the cipher arm only, before the record's
   write syscall, besides the write seam's existing `is_some` after the
   seal (the compiler may merge the two loads; not checked). The plain
   arm is unchanged.

4. **Buffered cipher run retried by a `sync`** (reviewer's X6, due only
   under Always): `a_runs_batch_is_written_again` takes its options; new
   `a_runs_batch_whose_log_write_failed_under_a_buffered_cipher_is_written_by_a_sync`
   under `EveryN(u32::MAX)` with the power-cut copy. Kills X6.

5. **Seam fidelity** (own O8 survived in mut3): nothing held that the
   cipher's injected partial write reaches the file. `a_stray_write_is_kept`
   now asserts the file is longer than the log's `written` after the
   failed write whose cut failed, under every caller (plain and cipher run).

Evidence and rule 4: the new tests pass on the round-2 code. None can fail
before the fix as evidence: the seal seam did not exist on the round-1 tree,
and the buffered-sync and visibility assertions hold on the round-1 tree
(the forced sync and the visibility were already there). They are counted
through their mutants only (X10, O7, X6, O8, and X7 for the handle check
that was already in the put helper). No before log this round.

Mutants: mutants.md, "Repair round 2". 13 run, 10 killed; X3 and X9
equivalent (reasoned there), X5b a control that confirms the power-cut
truncation is what kills X5/M2. `sh scripts/fmt.sh` was applied after the
mutant run; it re-wrapped one line of the seal seam (formatting only).

Runs this round: run-r2.log (durable 38/38), mut3-run.log (intermediate
tree), mut4-run.log (final tree), fmt-r2.log (`scripts/fmt.sh --check`,
exit 0 after fmt-r2-apply.log), after-r2.log (lib, durable, db, cipher,
fuzz, segwriter, --no-fail-fast), clippy-r2.log.

## Repair round 3 (one blocking item, ten extra mutants)

1. **Blocking: the SyncPolicy doc promised a cipher run's put-back batch
   durable at the run's next commit, which `EveryN` does not sync.** True:
   `commit_direct` -> `log_run(&mem, due)` with `due` the policy's, so the
   run's next commit writes the put-back frames with its own and syncs
   them only where the policy syncs that commit. Reworded: `SyncPolicy`
   now says the next commit writes the batch again, both become visible
   and reach the device together "as durable as that commit's policy
   makes it -- synced by it under `Always`, by the next sync `EveryN`
   makes otherwise", a `sync` writes and syncs them whatever the policy,
   a `flush` makes them durable with the rest of the store; the run's
   exception is now about visibility only, and its durability "follows the
   same rule": the run's next commit as its policy makes it, a commit with
   nothing new, a `sync` or a `flush` written and synced whatever the
   policy (all three go through `log_run_again`, due true). `Db::commit`'s
   paragraph gains "durable as that commit's policy makes it, and whatever
   the policy at a `sync` or a `flush`". CLAUDE.md's entry says the second
   wording's slip and its correction in one sentence, under the existing
   corollary.
   Tests holding the new wording (new, in the runs helper):
   - `..._under_a_buffered_cipher_is_written_by_the_runs_next_commit`
     (`then` 3, `EveryN(u32::MAX)`): after the run's next commit the log's
     synced length is below its written length (the frames are on the
     file, unsynced), then a `flush` writes nothing more to the log, and
     the power-cut copy reads a0, a1, a2. First written with a `sync` after
     the commit: it failed (a0 lost at the power cut) because `Db::sync`
     skips `sync_wal` while the live table is ordered -- the
     sync-ordered-buffered defect (its P3), not this unit's -- so the test
     uses `flush`, and `SyncPolicy` makes no promise about a `sync` after
     the run's next commit beyond "the next sync `EveryN` makes".
   - `..._under_a_buffered_cipher_is_written_by_a_flush` (`then` 4): a
     flush right after the failed commit.
   Neither can fail before the fix as evidence (the seams did not exist
   at 3df85d22); counted through O10 (the run's commit always synced:
   kills the first) and O11 (the early return never closes the frames:
   kills the second, with three others).

2. Extra mutants: mutants.md, "Repair round 3". 14 run, 11 killed.
   - R1 dies at put_back's `debug_assert_eq!` under `checked`; R1b (the
     same with the assert removed, as a release build runs) dies at replay
     in 12 tests, the reviewer's expectation.
   - R6 (kind dropped) needed a test: the live sync seam now injects
     `ErrorKind::StorageFull` (a writeback out of space; the seam's doc
     says so), and `a_failed_log_sync_is_kept_until_the_store_is_reopened`
     asserts that kind on every answer after it (`kept_as`). O12 (the
     kind dropped at the re-arm only) dies there too.
   - R10 (write seam before seal seam) needed an assertion: the seal case
     of `a_failed_write_is_written_again` asserts the commit's answer is
     the seal's ("seal failed"), i.e. the write seam -- which would hand
     the file the stale seal buffer -- was not the path. What reaches the
     file is not observable after the cut back; the answer is.
   - R5 survives, and its distinguishing state is behind a defect found
     building its test (below). R7 (= X3) and R9 (= E10) equivalent, as
     before.

3. **Found, not this unit's, not fixed: a run whose close fails at
   `leave_direct` leaves its ordered table live.** `close_direct` takes
   `direct` and returns at `land_seal()?` before `set_mem`, so the write
   that ended the run goes into the ordered table: `put` and `delete`
   panic ("a put never reaches an ordered memtable", in release too, it is
   an `assert!`), and `append` of a key below the run's last trips the
   ordered table's debug assert (and in release appends out of order).
   Reached with a cipher store, a held seal whose landing is failed with
   `fail_next_landing`, and a write below the greatest with a run open;
   the code is the same at 3df85d22. Reported for the owner; it needs its
   own unit (leave_direct on a failed close has to leave the writer on a
   fresh hashed table, or refuse the write and keep the run).

Runs this round: run-r3a.log (first run of the new tests: the one-shot
test hit the defect above, the `sync` form of then 3 hit
sync-ordered-buffered), run-r3.log (durable 40/40 on the final tests),
mut5-run.log, fmt-r3.log (`scripts/fmt.sh --check`, exit 0, no apply
needed), after-r3.log (lib, durable, db, cipher, fuzz, segwriter,
--no-fail-fast), clippy-r3.log.
