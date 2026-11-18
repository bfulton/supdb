# due-log mutants

report.md and deferred.md list no mutants for this defect; all below are the unit's own, on the lines it
changed (src/db.rs: commit_direct's run-open note, the due branch's check, Db::run_due_log_first).
Runner: mut.py (applies each to keep/src_db.rs.fixed, runs `cargo test --profile checked --test durable --
a_runs_due_commit a_runs_commit_under_always`, puts the fixed file back checked by its sha256).
Results: mut-results.txt (landing tree), mut-results.r1.txt (first round, on the tree before the refs tail
and the directory clause came out of the cold path). Logs: mut-<id>.log.

| mutant | verdict | killed by |
|---|---|---|
| M1 note never set (`log_owed = false && ..`) | killed | frames, loose, reopen, fail-order |
| M2 cipher gate dropped (note set under a cipher too) | killed | cipher: by the debug assertion (left 2 = RUN_LOGGED bit); in a build without it by the read-back (E2) |
| M3 cold path's log sync deleted | killed | frames, loose, reopen, fail-order |
| M4 run synced before the log in the cold path (swapped order) | killed | fail-order |
| M5 cold path never taken (`if false && d.log_owed`) | killed | frames, loose, reopen, fail-order |
| M6 cold path's re-ask dropped (log synced unconditionally) | killed | clean ("everyn-synced-after": a sync paid the log between open and due) |
| M7 loose clause dropped from the open note | killed | loose |
| M8 frames clause dropped from the open note | killed | frames, reopen, fail-order |
| M9 log sync's error ignored in the cold path | killed | fail-order |
| M10 note not cleared after the cold path | SURVIVED (argued) | -- |
| M11 check moved after the run's sync in the due branch | killed | fail-order |
| M12 `unsynced` not zeroed in the cold path | killed | clean ("everyn-synced-after": next due came at once, not after 5) -- round 1 survived; the test now asserts the commits between dues |
| M13 cold path's run sync deleted | killed | loose |
| M14 RUN_LOGGED gate flipped | killed | frames, loose, reopen, fail-order, cipher |
| M15 due-branch test flipped (`!d.log_owed`) | killed | frames, loose, reopen, fail-order, cipher |

Round 1 survivors and what was done:
- M11 (round 1: the cold path's refs tail dropped, `Options::direct_table_refs`): nothing observable --
  entries whose bytes the file holds become references at the run's next commit, as entries the file did
  not hold yet always do. Taken out: the cold path does no refs work, and the hot path's refs block is
  back to its text before the unit (no refactor into a helper).
- M12 (round 1 survivor): a missing test. Added the "everyn-synced-after" case and the assertion that the
  second due commit comes `n` commits after the first; killed in round 2.
- M10 survives: with the note left set, every later due commit of the run calls the cold path, which asks
  the log again, finds nothing owed and syncs only the run -- the same syscalls in the same order. The
  clear is a cost (one out-of-line call a due commit), not a check; no test can observe it. Kept.

Holding tests (pass before the fix, not evidence): clean and cipher; they hold the cost side (no log sync
where nothing is owed; under a cipher one log sync, behind the run's frames).

## Repair round 1 (mut2.py -> mut-results.r2.txt, logs mut-E<n>-*.log; restores src/db.rs and tests/durable.rs by sum)
New tests this round: `a_runs_commits_its_helper_synced_before_a_due_one_outlive_older_log_frames_until_it`
(the reviewers' window, held: small sync_ahead, the helper's fdatasync waited for by its count) and
`a_runs_due_commit_whose_log_sync_could_not_open_a_loose_file_syncs_it_at_the_next`.

| mutant | verdict | killed by |
|---|---|---|
| E1 re-ask drops the loose clause (`if self.wal.frames_unsynced()`) -- reviewer extras 1 and 4 | killed | loose: the rotation is after the commit's batch is written (`rotate_wal_by_size`), so the live file holds only its header and the re-ask finds nothing owed |
| E2 open note's RUN_LOGGED gate `true`, the debug assertion removed too -- extra 2 | killed | cipher, by its read-back (`sr_missing` of the run's keys: the cold path returns before `log_run`, so the due commit's frames never reach the log); its sync count reads 1 under this mutant, so the count alone does not kill it -- M2's "two live-log syncs" was the assertion's left value, not the test's count |
| E3 the helper test's first half asserting a prefix -- extra 3, the window probe | fails (as expected) | "keeps run commits (560 of 2140 keys) and loses the log's older ones (150 of 150)": the window is open on the fixed tree (before-r1.log) |
| E5 note tests RUN_REFS for RUN_LOGGED -- extra 5 | killed | frames and reopen under the refvals arm (`run_arms` has direct_table_refs), and cipher |
| E6 note cleared before the log sync -- extra 6 | survived round 1, killed after | not equivalent: sync_wal returns errors it does not keep (a loose file's open other than NotFound, the header's chain write), after which commits go on and the next due one must sync the log again. Killed by the unopened-loose test (loose file's name a link to itself during the due commit). For kept errors it is equivalent: every commit returns at `pending_err` before the due branch |
| E7 cold path never taken (M5) on the new tests | killed | helper test's second half ("kept 19940 ... lost 150 of the log's 150"), unopened-loose ("answered Ok"), and the round-0 tests -- this is the shape before the fix for the new tests (before-r1.log) |
| E8 cold path's log sync deleted (M3) on the new tests | killed | helper test, and the round-0 tests |
| E9 the power cut at the writer's syncs only (dl_power_cut using run_synced_bytes) | killed | helper test (the window half reads no run keys: the instrument is what reaches it) |
| E10 run_device_synced ignores the helper | killed | helper test |
| extra 8: `self.unsynced = 0` dropped where the re-ask finds nothing owed | killed (M12) | clean's "everyn-synced-after" case is exactly that variant (a `sync` since the open paid the log, so sync_wal is not called): "the run's due commits are the policy's, left 1 right 5" (mut-M12-unsynced-not-zeroed.log) |
| M10 note not cleared | survives, argued (unchanged) | cost only: a later due commit's cold path re-asks and finds nothing owed |

## Repair round 2 (mut3.py / mut3b.py -> mut-results.r3.txt, logs mut-<id>.log; restores src/db.rs and tests/durable.rs by sum)
New tests this round: `a_runs_owed_batch_written_again_by_a_commit_of_nothing_syncs_the_log_first` (all run arms),
`..._by_a_sync_syncs_the_log_first` (holding: passes before, Db::sync's own sync_wal covers the acknowledged state),
`..._whose_log_sync_fails_leaves_the_run_unsynced` (commit of nothing and sync), `..._over_a_log_a_sync_paid_syncs_no_log`
(holding, for O1), `a_runs_due_commit_under_a_cipher_beside_a_loose_log_file_syncs_it_behind_its_frames` (holding, for R5),
`a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames` (the close's window, held open).
before-r2.log: on the tree without the run_again fix, commit-default fails "kept 50 of the run's 50 keys and lost 150 of
the 150"; fail-commit fails "call 0 answered another error: the run's write failed" (the run synced, the log not);
before-r2-failsync.log (earlier shape of the fail test): fail-sync "the call whose log sync failed synced the run's file".

| mutant | verdict | killed by |
|---|---|---|
| R1 run_again unchanged (no log sync where `log_owed`) -- extras 1 and 5 | killed | commit-of-nothing (all arms), fail |
| R2 run_again syncs the run before the log -- extras 2 and 6 | killed | fail (fail_next_wal_sync: answers the run's held write first / run_synced_bytes moves) |
| R3 note cleared in run_again, log sync skipped -- extra 3 | killed | commit-of-nothing, fail |
| R4 close probe asserting a prefix -- extra 4 | fails (as expected) | the close window is open on the fixed tree: the cut keeps run keys and loses 150 of 150 log keys |
| R5 precedence slip in the open note -- extra 7 | killed | the new cipher loose test (the debug assertion in run_due_log_first) |
| R6 due re-ask drops the frames clause -- extra 8 | killed | frames, reopen, helper, fail-order |
| R7 note only under EveryN -- extra 9 | killed | reopen (Always) |
| O1 run_again's re-ask dropped (log synced whenever noted) | killed | over_a_log_a_sync_paid (trace grows) |
| O2 run_again leaves the note set | SURVIVED (argued, as M10) | the next due commit's cold path re-asks, finds nothing owed and syncs the run alone: same syscalls, one out-of-line call |
| O3 run_again's log sync error ignored | killed | fail |
| O4 run_again gate flipped (`!d.log_owed`) | killed | commit-of-nothing, fail |
| O5 run_again re-ask asks loose files only | killed | commit-of-nothing, fail |
| O6 close test's run file cut to nothing (instrument) | killed | close test: the window is reached only through the presync's extent |

## Repair round 3 (mut4.py -> mut-results.r4.txt, r4b, r4c; logs mut-<id>.log; restores src/db.rs and tests/durable.rs by sum after each)
New tests this round:
`a_runs_due_commit_under_always_whose_log_sync_could_not_open_a_loose_file_owes_its_batch` (X1; before-r3.log: fails
"a commit of nothing answered Ok under Always over the batch its due commit failed to sync, with nothing of the run synced",
left Some(0)), `a_runs_owed_batch_written_again_whose_log_sync_could_not_open_a_loose_file_syncs_it_at_the_next` (holding:
passes before; run_again already leaves the batch owed and the note standing on a sync_wal error, it is there for X4/X5),
`a_runs_due_commit_after_a_run_closed_into_a_held_seal_outlives_that_runs_commits` (holding: the window of blocking item 2,
open on the fixed tree).
Repair: `run_due_log_first` marks the batch owed by the run's writer (`SegmentWriter::owe_unsynced`) where `sync_wal` fails.

| mutant | verdict | killed by |
|---|---|---|
| X2 the repair deleted (no owe on sync_wal's error) | killed | X1 |
| X3 run_again's log-first block skips "whatever the repair adds" on sync_wal's error | not built: run_again adds nothing there. Its batch is owed by the write failure that brought it there (owes() true until a sync covers owed_to), and `sync_wal()?` returns before the note is cleared, so the next call syncs the log first again; held by the run_again unopened-loose test, which kills X5 (the clear moved ahead of the sync) |
| X4 run_again's re-ask drops the loose clause | killed | run_again unopened-loose test, X1 |
| X5 run_again clears the note before the log sync | killed | run_again unopened-loose test |
| X6 note taken from `self.unsynced != 0` instead of `wal.frames_unsynced()` | killed | reopen (Always after a reopen: the open leaves `unsynced` 0); the Loose case does not kill it (its n commits leave unsynced = n) |
| X7 run_again's log-first block only under EveryN | killed | X1 (its commit of nothing goes run_attended -> run_again under Always) |
| Y1 owe_unsynced's body empty | killed | X1 |
| Y4 the log error swallowed after owing (answers Ok) | killed | X1, unopened-loose (EveryN), fail-order |
| Y5 closed-run test: A's file left uncut (instrument) | survived first (A's batches were under a write piece, nothing on A's file: the cut was vacuous); the test was given write_piece 4096 and batches of 50 with A's written bytes asserted > 0; then killed | closed-run test |
| Y6 closed-run test: seal not held | survived on the second shape (the cut of A's file is not a crash state once its seal has synced it); the test now asserts A's file length unchanged by its held seal; then killed | closed-run test (by that premise) |
| Y7 owed only under EveryN | killed (first shape did not build; rebuilt with the policy read before the borrow) | X1 |
