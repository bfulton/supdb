# seal-sync mutants

Runner: mut.py (verdict appended to mutants.jsonl as each one finishes; the tree restored from keep/src_db.rs and its sha256 checked after each). Each mutant runs the section's 11 durable tests and tests/db.rs a_failed_presync_is_the_flushs_answer. Logs: m-<name>.log.

| mutant | what | verdict | killed by |
|---|---|---|---|
| M0-none | control: the fixed tree, nothing changed (must pass) | passes (control) | - |
| M1-wedge-only-not-landed | report seal-after #1: land_front wedges only under `if !landed` (the old guard) | KILLED | a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records, a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest |
| M2-publish-check-removed | report seal-after #2: Maint::publish does not refuse while wedged (merge path, close) | KILLED | a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_fsync_failed_is_not_named_durable_by_the_close, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest |
| M4-seal-fsync-retried | report seal-after #4: a retried fsync -- the hashed seal job goes on past its failed step as if a retry succeeded | KILLED | a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_fsync_failed_is_not_named_durable_by_the_close, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest |
| M4b-run-close-fsync-retried | report seal-after #6: the same at the direct run close site (close_run after) | KILLED | a_runs_close_whose_fsync_failed_is_named_by_no_manifest |
| P1-collect-lands-awaiting-job | report helper-presync: the job does not wait for the presync (collect lands a SealJob::awaits_writer job durable) | KILLED | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records |
| P2-presync-failure-not-wedged | report helper-presync: the writer answers the presync error but does not stop the queue | KILLED | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, a_failed_presync_is_the_flushs_answer |
| P4-wedge-cleared-after-answer | report helper-presync: the poison cleared after the first Err (join_seal clears seal_wedged) | KILLED | a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records, a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest, a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, a_failed_presync_is_the_flushs_answer |
| MP1-stop-discards-err | report manifest-pending: Maint::stop discards a joined seal error again | KILLED | a_seal_whose_fsync_failed_is_not_named_durable_by_the_close |
| L1-close-drops-run-on-refusal | own: close_direct drops the run when the wait for room is refused (the old take-before-wait) | KILLED | a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open |
| L2-dropped-write-goes-on | own: leave_direct reports the write may go on after a refused close | KILLED | a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing |
| L3-refusal-not-kept | own: the refused close is answered once (pending_err) and not kept | KILLED | a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing |
| S1-sync-check-removed | own: Db::sync does not answer the wedge | KILLED | a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records, a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest, a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records |
| O1-wedge-keeps-no-cause | own: Shared::wedge keeps no cause (answers lose the device words) | KILLED | a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records, a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, a_failed_presync_is_the_flushs_answer |
| O2-awaits-flag-flipped | own (flipped operator): hand_seal gets presync.is_none() for the non-inline arm | KILLED | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, a_runs_close_whose_fsync_failed_is_named_by_no_manifest |
| O3-readable-failure-wedged-without-cause | own: land_front_readable sets the flag without the cause | KILLED | a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing |
| O5-land-seal-old-words | own (swapped order/merged check): land_seal answers "the frozen table did not land" before the wedge | KILLED | a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing |

Not run, and why:
- report seal-after #3 (check missing in publish_unsynced / the pending path): those two checks were taken out, not kept -- no landing reaches them once the queue stops (seal_phases_due and join_seal return first), so a mutant of them could not be killed; the reasoning is in the comment at Maint::publish.
- report helper-presync "the inline arm joining the presync after seal_readable": that is the shipped order, kept (moving the join above the readable landing would add a wait before it, which the brief forbids); the wedge closes P2 and M1 is the mutant that reopens it (killed by the inline presync test).
- report helper-presync "the helper storing the error but the sync reading a different field": H1, the run-errors unit's.
- report manifest-pending "open fsyncs ..." mutants: the open half, the last unit's.

## Round 1 (repair)

Runner: mut1.py, fixed copy keep1/ (sha256 in keep1/SHA256SUMS), verdict appended to mutants-r1.jsonl as each finishes; the tree restored from keep1/src_db.rs and its sum checked after each. Each mutant runs the section's 15 durable tests and tests/db.rs a_failed_presync_is_the_flushs_answer. Logs m1-<name>.log. The round-0 table above is of the round-0 code and superseded where a row below names the same mutant.

| mutant | what | verdict | killed by |
|---|---|---|---|
| N0-none | control | passes (control) | - |
| B1-leave-direct-round0 | before: round 0 leave_direct (blocking 1) | KILLED | a_write_that_ends_a_run_beside_a_segment_work_error_waits_for_room_and_is_taken |
| B2-writes-go-round0 | before: writes_go of HEAD (drop the write, answer once) | KILLED | a_write_after_a_hand_off_over_a_stopped_queue_without_room_is_answered_until_reopen |
| B3-take-fresh-round0 | before: take_fresh_mem refuses a stopped queue with room | KILLED | a_write_after_a_hand_off_over_a_stopped_queue_with_room_is_taken |
| M1-wedge-only-not-landed | report seal-after #1 | KILLED | a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records, a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest |
| M2-hold-without-stopped | report seal-after #2: no manifest path asks the stopped queue | KILLED | a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest |
| M2b-stopped-never-set | own: stop_seals does not set the hold | KILLED | a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest |
| M4-seal-fsync-retried | report seal-after #4 | KILLED | a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_fsync_failed_is_not_named_durable_by_the_close, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest |
| M4b-run-close-fsync-retried | report seal-after #6 | KILLED | a_runs_close_whose_fsync_failed_is_named_by_no_manifest |
| P1-collect-lands-awaiting-job | report helper-presync | KILLED | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records |
| X5-collect-lands-awaiting-unless-wedged | extra 5 | KILLED | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records |
| P2-presync-failure-not-wedged | report helper-presync | KILLED | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, a_failed_presync_is_the_flushs_answer |
| P4-wedge-cleared-after-answer | report helper-presync | KILLED | a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken, a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest, a_failed_presync_is_the_flushs_answer |
| MP1-stop-discards-err | report manifest-pending | KILLED | a_seal_whose_fsync_failed_is_not_named_durable_by_the_close |
| MP2-stop-publishes-over-wedge | own: the close does not ask the stopped queue | KILLED | a_seal_whose_fsync_failed_is_not_named_durable_by_the_close |
| X6-stop-panic-as-success | extra 6 | SURVIVED | - |
| L1-close-drops-run-on-refusal | own r0 | KILLED | a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open |
| L2-dropped-write-goes-on | own r0 | KILLED | a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing |
| L3-refusal-not-kept | own r0 | KILLED | a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing |
| X1-leave-direct-if-true | extra 1 | KILLED | a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken, a_write_that_ends_a_run_beside_a_segment_work_error_waits_for_room_and_is_taken |
| X2-leave-direct-no-pending | extra 2 | KILLED | a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken, a_write_that_ends_a_run_beside_a_segment_work_error_waits_for_room_and_is_taken |
| X7-leave-direct-drop-e | extra 7 | KILLED | a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken, a_write_that_ends_a_run_beside_a_segment_work_error_waits_for_room_and_is_taken |
| X10-leave-direct-room-le | extra 10 | KILLED | a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_write_that_ends_a_run_beside_a_segment_work_error_waits_for_room_and_is_taken, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing |
| X10b-leave-direct-no-break | own: room with an error loops (inline wedge) | KILLED | hang: 100% CPU in a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken (inline arm loops); binary killed by hand |
| X10c-leave-direct-seals-ignored | own: no seal left is waited for | KILLED | a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log |
| X8-wedge-flag-before-cause | extra 8 (the flag before the lock) | SURVIVED | - |
| X11a-take-fresh-swap-checks | extra 11a | SURVIVED | - |
| X11b-take-fresh-no-wedge-branch | extra 11b | KILLED | hang: parks for good in a_write_after_a_hand_off_over_a_stopped_queue_without_room_is_answered_until_reopen; binary killed by hand |
| W1-writes-go-no-keep | own | KILLED | a_write_after_a_hand_off_over_a_stopped_queue_without_room_is_answered_until_reopen |
| O1-wedge-keeps-no-cause | own r0 | KILLED | a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records, a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log, a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_write_after_a_hand_off_over_a_stopped_queue_with_room_is_taken, a_write_after_a_hand_off_over_a_stopped_queue_without_room_is_answered_until_reopen, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest, a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, a_failed_presync_is_the_flushs_answer |
| O2-awaits-flag-flipped | own r0 | KILLED | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, a_runs_close_whose_fsync_failed_is_named_by_no_manifest |
| O3-readable-failure-wedged-without-cause | own r0 | KILLED | a_runs_close_whose_fsync_failed_is_named_by_no_manifest, a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_write_after_a_hand_off_over_a_stopped_queue_with_room_is_taken, a_write_after_a_hand_off_over_a_stopped_queue_without_room_is_answered_until_reopen, a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken, a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing |
| W2-writes-go-wedged-goes-on | own | KILLED | a_write_after_a_hand_off_over_a_stopped_queue_without_room_is_answered_until_reopen |

B1-B3 are the round-0/HEAD code put back: the "before" of the four new tests (each fails there and passes on the fix).

Survivors, and why:
- X6-stop-panic-as-success (extra 6): a seal thread that panics is an engine bug, and no input or seam makes one; the panic and the error now share one arm in Maint::stop (the map_err land_front uses too), and MP1 holds that arm. Unreachable; not counted as a test.
- X8-wedge-flag-before-cause (extra 8): the flag is now stored under the cause's lock, so the mutant as the reviewer wrote it (flag before the cause, inside the lock) is equivalent; the run mutant moves the store above the lock, a window between two instructions that no test can time. Survives by timing.
- X11a-take-fresh-swap-checks (extra 11, swap): both orders keep the write's failure as lost and both answers carry the cause; the swap changes only whether the first answer is the segment work's raw words (when one is still waiting) or the wedge's. Observably equivalent but for those words; no test pins which.

Not run, and why:
- extra 3 and 9 (Db::sync's wedge check moved above the log's sync): the check is gone (rule 1); a sync answers nothing of a stopped queue.
- S1 (round 0): the same check, gone.
- report seal-after #3 (a check in publish_unsynced / the pending slot): none was built; those landings are not reached once the queue stops (seal_phases_due, join_seal). M2/M2b remove the hold every other manifest path asks.
- W2 survived its first run; the test was strengthened (the dropped write is read by no table, not even the writer's own reads) and the rerun killed it.

## Round 2 (repair)

Runner: mut2.py, fixed copy keep2/ (sha256 in keep2/SHA256SUMS), verdict appended per mutant as it lands; the tree restored from keep2/src_db.rs and its sum checked after each. Each mutant runs the section's durable tests (19, then 20 with the same-look test) and tests/db.rs a_failed_presync_is_the_flushs_answer. Three passes, because the first two found problems in the tests themselves: mutants-r2-pass1.jsonl (seam in collect_seal), mutants-r2b.jsonl (seam moved into collect), mutants-r2c.jsonl (lifter and first-answer waits fixed; the verdicts below are from the latest pass that ran each mutant). Logs m2-<name>.log (the latest pass overwrites).

| mutant | what | verdict | killed by |
|---|---|---|---|
| N0-none | control | passes (control), every pass | - |
| R1-collect-skip-narrowed | extra 1: collect_seal lands an awaits_writer job whole when one poll sees both phases | KILLED (pass 3; pass 1 too) | a_flush_after_a_failed_presync_is_not_ok_whichever_way_its_seal_lands ("both" variant; the original presync test survives it, as the reviewer expected) |
| R1R12-collect-skip-narrowed-and-seam-gone | own: R1 with the readable-landing seam deleted | SURVIVED (expected) | - : shows the seam is what makes the "both" variant a test of R1 |
| R2-phases-due-sets-no-stop | extra 2 / the round-1 code: seal_phases_due does not set seals_stopped (deleted line) | KILLED | a_presync_that_fails_before_its_seals_publish_stops_every_manifest |
| R3-wedge-keeps-last-cause | extra 3 and 9: Shared::wedge keeps the last cause | KILLED | a_stopped_queue_answers_the_failure_that_stopped_it |
| R4-leave-direct-count-after-state | extra 4: the count read after the state | moot | the count was then taken out of leave_direct (R8/X10c below); while it stood it SURVIVED (timing, held by reading) |
| R5-stop-wedge-without-stopped | extra 5: Maint::stop sets the shared flag only | SURVIVED (pass 2; pass 1's kill was the racing first-answer test) | - : equivalent, the close's pending publish asks the shared flag and nothing after it writes a manifest |
| R6-stop-asks-local-flag | extra 6: Maint::stop asks seals_stopped instead of the shared flag | SURVIVED (pass 3; pass 2's kill was the racing test) | - : equivalent by timing; the loop's collect sets seals_stopped from the shared flag between the flush and the close in every run; held by reading (the close asks the shared flag because its own look may not have come) |
| R7a-readable-count-out-before-stop | extra 7 at land_front_readable: count out before the stop (the round-1 order) | SURVIVED (pass 3) | - : the words race, unheld by any test; the fixed order is held by reading (Release store of the flag before the AcqRel fetch_sub, read by the writer's Acquire load of the count then the flag) |
| R7b-front-count-out-before-stop | extra 7 at land_front's two failure sites | SURVIVED (pass 3) | - : as R7a |
| R8-writes-go-no-seals-arm | extra 8: writes_go without `seals == 0 ||` | SURVIVED (pass 3) | - : the arm was unreachable once the stop precedes the count-out; TAKEN OUT (the mutant is now the code) |
| X10c-leave-direct-seals-ignored | round-1 own, rerun: leave_direct without `seals == 0 ||` | SURVIVED (its round-1 kill was by the tests whose first answer raced) | - : unreachable as R8; TAKEN OUT |
| R10-between-before-collect-seal | extra 10: `between` read before collect_seal | SURVIVED | - : equivalent: a seal failure inside collect_seal makes `done` an Err, and the merge's landing asks `done.is_ok()` first, so nothing lands in that look |
| R10b-between-before-and-merge-lands-on-error | own: R10 with the `done.is_ok()` guard gone | KILLED | a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look (the same-look case is reached) |
| R11-between-and | own (flipped operator): `||` -> `&&` in seal_between_phases | KILLED | a_presync_that_fails_before_its_seals_publish_stops_every_manifest, a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look, a_seal_whose_directory_fsync_failed_is_named_by_no_manifest, a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest, a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest |
| R12-readable-hold-ignored | own: the seam deleted | SURVIVED (expected: a seam deleted weakens a test, cannot fail one) | - |

Own mutants on this round's lines: R2 (deleted line), R11 (flipped operator), R7a/R7b (swapped order), R10b, R1R12, R12.

Findings in the tests (not mutants): the first pass's R5 and the second pass's R6, R7a, R7b, R8 "kills" were one race in the tests -- the segment work stops the queue and only then leaves its error for the writer, so a commit right after seals_wedged() or !in_flight().0 can answer Ok and the next one the failure. Fixed by ss_commit_answer in a_stopped_queue_answers_the_failure_that_stopped_it, a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log and ss_run_close_fails (seal arm). Round 1's X10c kill was the same race.

Not run: extra 9 is extra 3 (same mutant).

## Round 3 (repair)

Runner: mut3.py (pass 1, mutants-r3.jsonl) and mut3b.py (pass 2, mutants-r3b.jsonl, after the watchdog and the checks taken out),
fixed copy keep3/ (sha256 in keep3/SHA256SUMS), verdict appended per mutant as it lands, src/db.rs restored from keep3/src_db.rs
and its sum checked after each. Each mutant runs the section's 21 durable tests (the new inline presync test included) and
tests/db.rs a_failed_presync_is_the_flushs_answer. Logs m3-<name>.log (pass 2 overwrites the ones it reran).

| mutant | what | verdict | killed by |
|---|---|---|---|
| N0-none | control | passes, both passes | - |
| E1-phases-due-sets-no-stop | extra 1: `self.seals_stopped = true` deleted in seal_phases_due | KILLED, both passes | a_presync_that_fails_before_its_seals_publish_stops_every_manifest_inline (new this round: slot (4,21) against (3,21), the inline commit lands the flush's merge) and the background test |
| E2-take-fresh-wedged-refuses-with-room | extras 2 and 8 (same mutant): `wedged && !room` -> `wedged` | KILLED | a_write_after_a_hand_off_over_a_stopped_queue_with_room_is_taken |
| E3-leave-direct-drops-with-room | extra 3: `!room && wedged` -> `wedged` | KILLED | a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken |
| E4-writer-presync-no-wedge | extra 4: the writer's presync keeps presync_failed and does not wedge | KILLED, both passes | a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records, ..._whichever_way_its_seal_lands, tests/db.rs a_failed_presync_is_the_flushs_answer; pass 1 HUNG in the two stops_every_manifest tests (the flush waits on a held seal / joins a held merge) -- fixed: SsWatchdog lifts the holds from a handle after 60 s (seam Reader::lift_landing_holds_from_handle); pass 2 the two fail instead of hanging |
| E5-stop-publishes-whatever-the-wedge | extra 5: `&& !seal_wedged` dropped in Maint::stop | KILLED, both passes | a_seal_whose_fsync_failed_is_not_named_durable_by_the_close (inline variant), by Maint::publish's debug assertion "a manifest written while a seal's segments are published and not durable" on the test's thread. The thread variant and the presync cause ("pending" variant of the presync test) hit the same assertion on the supdb-maint thread at the close, where the close swallows the panic: in the checked profile the assertion stops the publish either way; the clause is what holds a release build, by reading |
| E6-land-seal-no-count-check | extra 6: `if seals == 0 { return Err(...) }` deleted in land_seal | SURVIVED | - : unreachable (a frozen table is a seal in flight or a stopped queue's, the stop preceding the count-out); TAKEN OUT with its load -- the mutant is now the code |
| E7-take-fresh-no-count-check | extra 7: `seals == 0 && !wedged` -> `false` | SURVIVED | - : as E6; TAKEN OUT with its load |
| E9-awaits-writer-draining | extra 9: awaits_writer `presync.is_some()` -> `self.draining` | SURVIVED | - : equivalent: draining is set by `flush` alone, and `close_direct` returns Ok after `close_run`, so `seal()` is always followed by `maint_request(Drain)`, whose `join_seal` lands the job durable; the two differ only in whether the segment work may land it durable before the Drain ask arrives under close_presync off, which the flush waits out either way. No test separates them; held by reading |
| E10-stop-reads-wedge-before-joins | extra 10: Maint::stop reads seal_wedged before joining its seals | KILLED, both passes | a_seal_whose_fsync_failed_is_not_named_durable_by_the_close (inline variant: its seal's fsync fails in stop's own join), by the same debug assertion as E5 |
| O1-phases-due-wedged-no-return | own (deleted line): seal_phases_due's wedged branch does not return | KILLED | a_write_after_a_hand_off_over_a_stopped_queue_with_room_is_taken, a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken (supdb-maint: "a landed table is the live one or the oldest frozen one") |
| O2-writes-go-flipped | own (flipped operator): `if seal_wedged` -> `if !seal_wedged` in writes_go | KILLED (hang) | a_write_after_a_hand_off_over_a_stopped_queue_without_room_is_answered_until_reopen loops in writes_go; binary killed by hand after 9 min |
| O3-join-seal-no-early-answer | own (deleted line): join_seal's wedged early answer | KILLED, both passes | 14 section tests and tests/db.rs a_failed_presync_is_the_flushs_answer; pass 1 hung in the two stops_every_manifest tests, pass 2 (watchdog) fails them |

Own mutants on the unit's lines this round: O1 (deleted line), O2 (flipped operator), O3 (deleted line). No swapped-order own
mutant was run this round; round 2's R7a/R7b were the swapped orders.
