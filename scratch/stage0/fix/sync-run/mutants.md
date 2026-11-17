# sync-run: mutants

Runner: mut.py (verdicts appended to mutants.jsonl as each comes; diffs m-<name>.diff, logs m-<name>.log).
Each mutant written into src/db.rs from keep/db.rs (sha256 b23a1ac1...), the whole `durable` binary run
under `cargo test --profile checked`, and keep/db.rs copied back and its sum checked after each.
Round 1 ran before the last three tests were added (a_sync_with_nothing_of_the_run_unsynced_syncs_nothing_of_it,
a_sync_starts_the_policys_count_over_for_an_open_run); round 2 ran O9 and O8 with them. src/db.rs after both
rounds: b23a1ac1... (the fixed copy).

| mutant | origin | what | verdict | killed by |
|---|---|---|---|---|
| M0-fix-reverted | own | Db::sync as at HEAD (`!ordered` gate, no run sync) | killed | all 8 tests of the section that existed then (the before-state of the round-1 tests, incl. RunHeld::One, the cipher test, the two sync-failure tests and the order test) |
| R1-tail-written-not-synced | report 1 | sync_run writes the held tail and the buffer, no fdatasync | killed | a_sync_makes_an_open_runs_commits_survive_a_power_cut, a_sync_with_a_run_open_makes_the_logs_unsynced_frames_survive_a_power_cut, a_sync_whose_run_fdatasync_fails_answers_it_and_keeps_it, a_sync_whose_write_of_the_runs_tail_fails_is_written_by_the_next_sync (the kill test passes, as the skeptic said) |
| R2-ordered-gate-kept | report 2 | `!self.mem().ordered &&` back on the log's gate | killed | a_sync_with_a_run_open_makes_the_logs_unsynced_frames_survive_a_power_cut, ..._logged_commits_survive_a_power_cut_under_a_cipher, a_sync_with_a_run_open_syncs_the_log_before_the_run |
| R3-run-synced-after-hand-tail | report 3 | the run synced after the adaptive_shape hand-off | killed | a_sync_that_hands_an_open_run_to_a_seal_has_put_its_commits_on_the_file |
| R5-run-synced-only-without-adaptive | report 3 | the run synced only when !adaptive_shape | killed | a_sync_that_hands_an_open_run_to_a_seal_has_put_its_commits_on_the_file |
| R4-gate-file-written-vs-synced | report 4 | gate `file_written > synced`, blind to the held stream and the buffer | killed | kill, power cut, hand-off, both sync-failure tests |
| (report 5) | report 5 | a gate on the `unsynced` counter | not a mutant here: it is the fix's gate. By reading it is exact: a run's commit raises the count and only a sync that covered the run lowers it (commit_direct's due branch, run_again, sync_wal from log_run's due path after the run's own sync, Db::sync, flush before its seal closes the run); the skeptic's case (a due commit, then one more commit, then sync) is RunHeld under EveryN(5) and passes, and one commit after nothing (RunHeld::One) is held by O3. |
| (report 6) | report 6 | the run's directory fsync left out | not run: the barrier is run-temp-dir-barrier's, deferred (deferred.md); no directory fsync was added here |
| O1-run-sync-deleted | own (deleted line) | `if run_owed { sync_run }` deleted | killed | 6 tests (kill, power cut, hand-off, log-beside-run, both failures) |
| O2-gate-read-after-log-sync | own (swapped order) | run_owed read after sync_wal, which zeroes the count | killed | a_sync_with_a_run_open_makes_the_logs_unsynced_frames_survive_a_power_cut |
| O3-count-gt-1 | own (flipped operator) | `unsynced != 0` -> `unsynced > 1` | killed | kill and power cut (RunHeld::One) |
| O4-sync-run-error-ignored | own | sync_run drops the writer's error | killed | both sync-failure tests |
| O5-sync-run-error-not-kept | own | sync_run answers the error without run_failed (not kept) | killed | a_sync_whose_run_fdatasync_fails_answers_it_and_keeps_it (a commit of nothing answered Ok after) |
| O6-run-before-log | own (swapped order) | the run synced before the log | killed | a_sync_with_a_run_open_syncs_the_log_before_the_run |
| O7-log-gate-frames-only | own | log gate on frames alone, loose files and the directory dropped | killed | a_failed_sync_of_a_loose_log_file_is_kept, a_failed_sync_of_the_log_directory_is_kept (log-errors' tests; the clause was there before) |
| O9-gate-any-open-run | own | run synced whenever a run is open, owed or not | killed | a_sync_with_nothing_of_the_run_unsynced_syncs_nothing_of_it |
| O8-unsynced-not-zeroed | own (deleted line) | Db::sync's `self.unsynced = 0` deleted | survived round 1; killed in round 2 | a_sync_starts_the_policys_count_over_for_an_open_run (added for it) |

Tests that cannot fail before the fix, and are counted only through their mutants:
a_sync_with_nothing_of_the_run_unsynced_syncs_nothing_of_it (O9), a_sync_starts_the_policys_count_over_for_an_open_run (O8).

## Repair round 1

Runner: round1/mut1.py, verdicts appended to round1/mutants1.jsonl as each came (diffs round1/m-*.diff, logs
round1/m-*.log). Fixed copy: keep1/db.rs (sha256 8f6156b8..., round 0's code with the docs restated); src/db.rs
read back at that sum after the run. The whole `durable` binary each time (115 tests).

| mutant | origin | what | verdict | killed by |
|---|---|---|---|---|
| M0-fix-reverted (again) | own | Db::sync as at HEAD | killed | the 9 fix tests, among them the new a_sync_over_a_run_whose_helper_sync_failed_answers_it_and_keeps_it (so it is evidence) |
| X1-run-owed-before-commit-staged | review 1 and 5 (one mutant) | run_owed read before commit_staged | killed (after RunHeld::SyncedThenStaged was added: sync, 100 appended, sync) | a_sync_puts_an_open_runs_commits_where_a_process_kill_keeps_them, a_sync_makes_an_open_runs_commits_survive_a_power_cut |
| X2-sync-run-without-helper-error | review 2 | sync_run writes, flushes and fdatasyncs itself, no synced_all (the helper's kept error not read) | killed | a_sync_over_a_run_whose_helper_sync_failed_answers_it_and_keeps_it (new; arms the helper's fdatasync through SegmentOptions::fail_helper_syncs) and a_sync_whose_run_fdatasync_fails_answers_it_and_keeps_it (arms the writer's own). Answer to the review: the old test armed the writer's fdatasync only. |
| X3-unsynced-zeroed-before-sync-run | review 3 | `self.unsynced = 0` above `if run_owed { sync_run }` | SURVIVED, equivalent | The gate is the local run_owed, read before; the field only differs after sync_run fails, and then the run's own state takes over: a failed fdatasync is kept (Db::lost) and every call answers it; a failed write leaves the writer owing, and the next call's commit_staged -> commit_direct "nothing new" asks attends()/owes() and writes it again (run_again) whatever `unsynced` says; a new commit raises the count again. |
| X4 (blocking item) | review 4 | the sync after a run closed into a seal in flight, the tree as built | the new test fails on this tree, 16 of 16 cases (round1/closed-before.log) | not built (deferred.md, ROADMAP); not in durable.rs |
| X6-seal-ordered-zeroes-unsynced | review 6 | `self.unsynced = 0` in Db::seal's ordered branch | killed | a_runs_commits_count_toward_the_policys_sync_after_it_closes (new; characterization, passes at HEAD) |
| X6b-close-direct-zeroes-unsynced | own (variant of 6) | `self.unsynced = 0` at close_direct's end | killed | the same |
| X7-copy-arms-flush-only | review 7 | sync_run only flushes (no fdatasync) when !direct_stream | killed | a_sync_makes_an_open_runs_commits_survive_a_power_cut (copy arms), both sync-failure tests |
| X8-log-gate-frames-only-when-hashed | review 8 | `(frames_unsynced && !ordered) \|\| loose \|\| dir` | killed | ..._logged_commits_survive_a_power_cut_under_a_cipher (both policies in one test), ..._logs_unsynced_frames_survive_a_power_cut, ..._syncs_the_log_before_the_run |
| X9a-every-failure-kept-as-lost | review 9 | sync_run keeps every failure as lost | killed | a_sync_whose_write_of_the_runs_tail_fails_is_written_by_the_next_sync |
| X9b-kept-unless-owed | review 9 (literal) | sync_run keeps the failure unless the writer owes() | SURVIVED, equivalent | Inside sync_run owes() and sync_lost() are complements: commit_staged runs first and writes any owed batch again (run_again), so sync_run starts owing nothing; its write failing sets owes (not lost), its fdatasync or the helper's failing sets lost and leaves owed_to <= synced (not owing). So the gate on owes and the gate on sync_lost pick the same branch on every reachable failure. |

## Repair round 2

Runner: round2/mut2.py, verdicts appended to round2/mutants2.jsonl as each came (diffs round2/m-*.diff, logs
round2/m-*.log). Fixed copy: keep2/db.rs (sha256 0ff377c4..., round 1's code with the SyncPolicy and Db::sync
docs restated again; no code changed this round); src/db.rs read back at that sum after the run. The whole
`durable` binary each time (116 tests; 117 for the E8 rerun).

| mutant | origin | what | verdict | killed by |
|---|---|---|---|---|
| E1-due-unsynced-not-zeroed | review extra 1 | commit_direct's due branch without `self.unsynced = 0` | killed | a_sync_with_nothing_of_the_run_unsynced_syncs_nothing_of_it ('every5-due', confirmed) |
| E2-gate-file-written | review extra 2 | run_owed = `direct.is_some_and(file_written() > 0)` | killed | the nothing-unsynced test ('always'), and kill, power cut, hand-off and both sync-failure tests (the shapes with nothing on the file yet). Not equivalent to O9, which only the nothing-unsynced test killed. |
| (extra 3) | review extra 3 | the closed-run test extended to a delete above the greatest, a put above it, a value that does not inline and a commit crossing seal_bytes, beside the lower key and Db::seal | the test fails on this tree in all 24 cases (round2/closed-before.log): every close leaves the gap, which is why the docs now name every close | not built (deferred.md, ROADMAP); not in durable.rs |
| E4-log-gate-or-run-owed | review extra 4 | the log's gate gains `\|\| run_owed` (a redundant log fdatasync) | survived the tests there were (as predicted); killed after a_sync_with_only_the_run_owing_syncs_nothing_of_the_log was added (Db::trace_wal_syncs around a sync where only the run owes) | a_sync_with_only_the_run_owing_syncs_nothing_of_the_log |
| E5-gate-ordered-not-direct | review extra 5 | run_owed = `unsynced != 0 && mem().ordered` | SURVIVED, equivalent | After commit_staged, an ordered live table has `direct` None only while it is handed (mem_handed Some), and that is reached only from sync's own hand_tail after `unsynced = 0`; Db::commit and commit_staged return at mem_handed before counting. A run with appends and no commit has an ordered table and no `direct`, but commit_staged at sync's top commits it and creates `direct`. close_direct (Db::seal, leave_direct) installs a hashed table (freeze, MemTable::new; the no-run branch likewise), so the reviewer's state (ordered live, direct None after a close) is not reached. |
| E6-sync-run-err-not-run-failed | review extra 6 | sync_run answers `Err(e)` without run_failed | killed | a_sync_whose_run_fdatasync_fails_answers_it_and_keeps_it, a_sync_over_a_run_whose_helper_sync_failed_answers_it_and_keeps_it. (run_failed does no owed bookkeeping: it keeps a lost sync and passes a write's error through; the owed bit is the writer's own, so this mutant is O5's kind and the write-failure test is not what kills it.) |
| E7-sync-run-skips-logged | review extra 7 | sync_run returns Ok when the table is RUN_LOGGED | SURVIVED, equivalent; not adopted | Under a cipher the run's commits are the log's frames, made durable by the log's sync just before; the run's file is a memory file whose fdatasync is of nothing, and its held tail reaches it at the next due commit or the close, which writes the writer's tail (the finish) before the envelope. No reader reads the held tail from the file: the stream's blocks are given up only once the file holds them. Not adopted: it saves a memory-file write and an empty fdatasync on a cipher store's sync only, and adds a branch to the run's sync. |
| E8-due-zeroed-before-sync | review extra 8 | in the due branch, `self.unsynced = 0` above `d.w.sync()` | survived the tests there were; killed after a_run_commit_after_a_failed_due_write_is_due was added | a_run_commit_after_a_failed_due_write_is_due. Not equivalent: a due commit whose write fails answers the error with its count unspent, so the next commit is the policy's sync; zeroed first, the next commit counts from one and the acknowledged commits before and after the failure stay unsynced past EveryN's bound. A lost fdatasync is equivalent (kept, every call answers it). |
| N1-log-gate-or-any-unsynced | own (flipped condition) | log gate gains `\|\| self.unsynced != 0` | killed | a_sync_with_only_the_run_owing_syncs_nothing_of_the_log |
| N2-due-gt | own (flipped operator) | run due test `unsynced > n` | killed | a_sync_starts_the_policys_count_over_for_an_open_run, a_sync_with_nothing_of_the_run_unsynced_syncs_nothing_of_it, a_run_written_again_restarts_the_policys_count, a_due_commit_is_not_acknowledged_over_a_failed_helper_sync |
| N3-due-sync-deleted | own (deleted line) | the due branch's `d.w.sync()` deleted | killed | many run-error tests (a_failed_run_fdatasync_is_kept_until_the_store_is_reopened, a_due_commit_is_not_acknowledged_over_a_failed_helper_sync, ...) |

Tests added this round that pass on the tree before the fix and are counted only through their mutants:
a_sync_with_only_the_run_owing_syncs_nothing_of_the_log (E4, N1), a_run_commit_after_a_failed_due_write_is_due (E8).

## Repair round 3

Runner: round3/mut3.py (fixed copy keep3/db.rs, sha256 a6925433...), then round3/mut4.py (keep4/db.rs,
sha256 69634585..., keep3 with the directory clause taken out of Db::sync's log gate); verdicts appended to
round3/mutants3.jsonl and round3/mutants4.jsonl as each came, diffs and logs round3/m-*.{diff,log}. src/db.rs
read back at the copy's sum after each. The whole `durable` binary each time (119 tests).

Tests added this round: RunHeld::AfterDue (six commits of a hundred: under EveryN(5) one commit past the due
sync; in the kill, power-cut and cipher matrices); a second `sync` in a row in the hand-off test;
a_sync_syncs_a_run_whose_commits_end_on_a_piece_boundary (every run arm; the last commit's value sized so the
stream ends on a 1 MiB boundary, premise asserted through the new test accessor Db::run_written_and_held:
held 0, written a whole number of pieces, synced below written); a_sync_with_a_run_open_pays_the_log_directorys_barrier
(rotated loose, sealed, run opened, the directory's fsync armed to fail). All four fail under M0 (the fix
reverted; round3/before-new-tests.log), so they are evidence.

| mutant | origin | what | verdict | killed by |
|---|---|---|---|---|
| M0-fix-reverted (again) | own | Db::sync as at HEAD | killed | 12 tests, among them the piece-boundary test, the hand-off test and the directory-barrier test, and the matrices' AfterDue cases |
| F1-run-sync-else-of-log | review extra 1 | `if log owes { sync_wal } else if run_owed { sync_run }` | killed | a_sync_with_a_run_open_makes_the_logs_unsynced_frames_survive_a_power_cut (the plain store's run tail), as expected. Under a cipher the log's sync is the run's durability, so the cipher tests do not see it. |
| F2-unsynced-zeroed-before-run-owed | review extra 2 | `self.unsynced = 0` moved above `let run_owed` | killed | 9 tests, as O1 (kill, power cut, piece boundary, hand-off, both sync failures, the write failure, ...) |
| F3-sync-run-calls-run-again | review extra 3 | sync_run's body is `self.run_again()` | SURVIVED, equivalent | run_again is `d.w.sync()` with run_failed (sync_run's body), then `unsynced = 0` (Db::sync zeroes it right after), then, under a cipher only: a log write if `logged < len` or frames are pending -- never, since commit_staged at sync's top logged every entry (log_run) or returned its error -- and a log sync if frames are unsynced or loose_wals non-empty -- never, since Db::sync synced the log just before under the same condition or returned its error. So no extra log sync is reachable; a_sync_with_only_the_run_owing_syncs_nothing_of_the_log and the cipher tests pass because there is none. |
| F4-gate-ordered-not-direct | review extra 4 (E5 again) | run_owed gated on `mem().ordered` | SURVIVED, equivalent (confirmed with the hand-off test's second sync in a row) | After hand_tail the live table is the handed ordered table with `direct` None, but Db::sync zeroed `unsynced` before the hand-off, and the second sync's commit_staged returns at `mem_handed` with nothing counted, so run_owed is false under both gates; a commit after the hand-off goes through take_fresh_mem first, which replaces the table. |
| F5-count-gt-1 | review extra 5 | `unsynced > 1 && direct.is_some()` | killed | the kill and power-cut matrices, in every arm (default, copyvals, threecalls, refvals, norecycle, keysarena, keycopies, fatentries): RunHeld::One under both policies and the new RunHeld::AfterDue under EveryN(5), a count of exactly one from the run alone after a due sync |
| F6-zero-before-run-sync-and-write-errors-unkept | review extra 6 | X3 plus sync_run answering a write error without run_failed | SURVIVED, equivalent | run_failed passes a write's error through unchanged (it keeps only a lost sync), so the second half is the identity; the first is X3. After a failed tail write the next sync's commit_staged -> commit_direct "nothing new" asks attends() and owes(), which are the writer's own state, and writes the batch again (run_again) whatever `unsynced` says: a_sync_whose_write_of_the_runs_tail_fails_is_written_by_the_next_sync passes under the mutant because that path does not read the count. |
| F7-run-synced-only-when-held | review extra 7 | sync_run returns Ok with nothing held (batch_buf, buffer, stream tail all empty) | killed | a_sync_syncs_a_run_whose_commits_end_on_a_piece_boundary (new) |
| F8-hand-tail-before-run-sync | review extra 8 | the hand_tail block above `if run_owed`, run_owed read before | killed | a_sync_that_hands_an_open_run_to_a_seal_has_put_its_commits_on_the_file (panics at sync_run's expect: the hand-off took `direct`) |
| F9-dir-clause-deleted | review extra 9 | `\|\| self.wal_dir_unsynced` deleted from Db::sync's log gate | SURVIVED: an unreachable check, taken out | wal_dir_unsynced is set only at a rotation: a loose one pushes its file to loose_wals beside it, which only sync_wal empties and only after the barrier; a chained one pays the barrier at once or keeps its failure, which commit_staged answers at sync's top (pending_err refilled from Db::lost). So outside the kept-error state the flag implies loose_wals non-empty. The clause is gone from Db::sync (keep4), with a comment saying why, as run_again already argued. The new directory-barrier test holds the barrier with an ordered table live through the loose clause. |
| Q1-loose-and-dir-gated-on-hashed | own | the loose and dir clauses gated on `!mem().ordered` | killed | a_sync_with_a_run_open_pays_the_log_directorys_barrier (new) |
| Q3-direct-flipped | own (flipped operator) | `direct.is_none()` | killed | 26 tests |
| G1-loose-clause-deleted (on keep4) | own (deleted clause) | `\|\| !self.loose_wals.is_empty()` deleted | killed | a_failed_sync_of_a_loose_log_file_is_kept, a_failed_sync_of_the_log_directory_is_kept, a_sync_with_a_run_open_pays_the_log_directorys_barrier |
| G2-frames-clause-deleted (on keep4) | own (deleted clause) | `self.wal.frames_unsynced() \|\|` deleted | killed | many (the cipher run tests, the reopen tests, ...) |
