# run-errors: mutants

Runners: round 0 `mut.py` (its first table is kept as round2/mutants-round0.md);
round 1 `round1/mut2.py` (exact-string mutants of src/db.rs from round1/keep/db.rs,
build `--test durable` under `checked`, run the whole durable binary
`--test-threads 2` under `timeout 420`, restore keep byte for byte and assert it),
results round1/mut-results-run1.json, -run2.json (diffs and logs in round1/m/);
round 2 (after the restart) `round2/mut3.py`, same procedure from round2/keep/db.rs,
results round2/mut-run.log, mut-run-F2.log, mut-run-G.log (diffs and logs in round2/m/).
After every runner, `cmp src/db.rs round2/keep/db.rs` (or round1/keep) was checked.

Round 1's code is the folded shape (held and lost state as bits of the writer's
`fault` byte, `RUN_ATTN_HELD`/`RUN_ATTN_LOST`); the mutants below are of that
shape. Each was run against the round-1 tests unless it says round 2.

| mutant | source | killed by |
|---|---|---|
| R1 cursor writes (pwritev -> writev at the cursor, write_all_at -> write_all, the partial seam at the cursor) | report | a_run_write_that_failed_part_way_is_flushed_whole, a_run_write_that_failed_part_way_loses_no_commit_acknowledged_after_it |
| R2 `owes` not asked at commit_direct's early return | report | 8 tests, among them ..._by_a_commit_of_nothing, ..._by_a_sync, ..._under_a_cipher_is_written_again |
| R2b defer owes nothing | report | a_buffered_run_piece_write_that_failed_is_answered_and_written_by_a_sync, a_run_write_the_device_refuses_again_is_tried_once_a_sync, a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again, a_run_written_again_restarts_the_policys_count |
| R3 latch not set (`lose` leaves `RUN_ATTN_LOST` clear) | report | run 1 / round1 mut-results.json: 6 tests (a_failed_run_fdatasync_is_kept_until_the_store_is_reopened, ..._under_a_cipher, a_run_whose_fdatasync_failed_is_not_closed_into_a_named_segment, a_commit_under_always_..., a_due_commit_..., a_marked_segments_failed_sync_is_every_later_syncs_answer). Its run-2 pattern was stale; re-run alone, killed. |
| R4 latch taken (a lost sync's error answered once) | report | a_marked_segments_failed_sync_is_every_later_syncs_answer |
| R5 the run's failed sync not kept (`run_failed` answers `e`) | report ("a fix that retries") | 5 tests: the fdatasync tests and both helper tests through the store |
| R6 mark_stream answers its piece error | report | 6 tests, among them a_failed_run_piece_write_is_held_and_written_by_the_next_commit |
| R7 helper error load deleted from synced_all (H1) | report | a_marked_segments_sync_reports_its_helpers_failed_sync, a_due_commit_is_not_acknowledged_over_a_failed_helper_sync, a_commit_under_always_is_not_acknowledged_over_a_failed_helper_sync |
| O1 a lost file's writes not refused (`run_write_fault`'s LOST arm deleted: the finish's refusal) | own, deleted line | a_marked_segments_failed_sync_is_every_later_syncs_answer, a_run_whose_fdatasync_failed_is_not_closed_into_a_named_segment |
| O2 no stream copy behind a failed head | own, deleted line | a_run_closed_over_a_failed_write_writes_the_batch_into_its_segment, a_runs_first_write_that_failed_is_written_again |
| O3 not stuck (piece left after a deferral) | own, deleted line | a hang: a_marked_segments_failed_sync_is_every_later_syncs_answer loops in `AlignedWriter::write` (no room at a boundary that can never be written), the runner's timeout (exit 124). A kill, but by a hang, not an assertion. |
| O4 sync does not ask the held state | own, deleted line | 8 tests |
| O5 `owes` `>` -> `>=` | own, flipped operator | a_commit_of_nothing_in_a_run_that_owes_nothing_syncs_nothing |
| O6 run_again does not log under a cipher | own, deleted line | a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again, a_run_write_that_failed_under_a_cipher_is_written_again |
| O7 synced counted before the helper's error is read | own, swapped order | a_due_commit_is_not_acknowledged_over_a_failed_helper_sync |
| O8 AlignedWriter::write answers its failed piece | own, deleted line | 5 tests |
| O9 stream_hash answers the head's failed write | own, deleted line | a_run_closed_over_a_failed_write_..., a_runs_first_write_that_failed_is_written_again |
| O10 defer ignores `defers` | own, deleted line | an_unmarked_segments_failed_write_is_answered_where_it_happens |
| N1 held bit not set in defer | own (round 1), deleted line | 7 tests |
| N2 held bit not cleared when answered | own (round 1), deleted line | 7 tests |
| N3 attention write swallowed (`run_write_fault` returns Ok without writing) | own (round 1) | a_run_closed_over_a_failed_write_..., a_runs_first_write_that_failed_is_written_again |
| N4 sync's top compare asks only the seam | own (round 1), the fold | 7 tests |
| E1 `owe`'s `.max(self.synced + 1)` dropped | review (cost, crash) | no mutant to run: the term was taken out in round 1 as a check nothing reaches (pattern not found) |
| E2 defer overwrites the held error | review (crash) | a_marked_segments_failed_sync_is_every_later_syncs_answer |
| E3 held write error sticky (not taken) | review (crash) | 7 tests |
| E4 `owing` owes nothing | review (crash) | 4 tests, the commit-of-nothing and sync rescues |
| E5 run_again drops `|| !pending.is_empty()` | review (cost, crash) | a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again |
| E6 run_again keeps `unsynced` | review (cost, crash) | a_run_written_again_restarts_the_policys_count |
| E7 a stuck writer's head not retried in stream_hash | review (crash) | survived round 1; round 2: a_buffered_runs_failed_first_write_is_tried_again_before_its_sync (the file holds the head after the commit whose head failed) |
| E8 the held check moved below the batch_buf write in sync | review (crash) | survived round 1; it showed a real wrinkle (a failure met inside a sync that then wrote and synced every byte left the held bit set, and the sync after answered it without writing its own batch), fixed in `answer_held`; round 2: a_marked_segments_write_that_failed_inside_a_sync_is_paid_by_that_sync |
| E9 stream_copy's base wrong | review (cost) | a_run_closed_over_a_failed_write_writes_the_batch_into_its_segment |
| E11 `lose` calls `owe` | review (cost) | no mutant to run: the call was taken out in round 1 (pattern not found) |
| F1 `answer_held` never paid (always the error) | own (round 2), deleted check = the round-2 fix undone | a_marked_segments_write_that_failed_inside_a_sync_is_paid_by_that_sync |
| F2 paid test `>` -> `>=` | own (round 2), flipped operator | survived the first round-2 run; then a_write_that_failed_inside_a_sync_and_owed_exactly_what_it_synced_is_paid (records ending at a piece's boundary, so the owed bytes equal the synced) |
| F3 the held bit kept when paid | own (round 2), deleted line | 8 tests (the debug `expect("a held failure")` fires) |
| G1 run_again's log sync deleted (cipher, EveryN) | own (round 2), deleted block | a_cipher_runs_batch_written_again_by_a_commit_of_nothing_is_synced_in_its_log |
| G2 run_again's log-sync condition flipped | own (round 2), flipped operator | the same |

Not run as separate mutants (report's list, by mapping): "the helper storing the
error but the sync reading a different field" is R7's shape; "the poison
cleared after the first Err" is R4.

Survivors: none. O3 is killed by a hang rather than by an assertion.

## Round 3 (repair: owes folded into the fault byte; the reviewers' extra mutants)

Runner `round3/mut4.py`: exact-string mutants of src/db.rs from round3/fixed/db.rs,
build `--test durable` under `checked`, whole durable binary `--test-threads 2`
under `timeout 420`, restore byte for byte and assert it. Results round3/mut-run.log
(all) and round3/mut-run2.log (re-run after tests were strengthened); diffs and
logs in round3/m/. After both runs `cmp src/db.rs round3/fixed/db.rs` was equal.

| mutant | source | killed by |
|---|---|---|
| X1 pwritev loop: `at += n` deleted (the two reviewers' entries are one mutant) | review | a_short_run_write_is_written_whole_at_the_offset_it_reached (new; seam `short_next_run_write`, RUN_FAULT_SHORT, through the shared `pwritev_loop`) |
| X2 sync_with: synced advanced whatever the fdatasync answered (`synced_all().and(r)`) | review | a_failed_run_fdatasync_is_kept_until_the_store_is_reopened (now asserts run_synced_bytes unchanged over the failed fdatasync) |
| X3 stream_copy: `held_to = to` deleted | review | a_buffered_runs_failed_first_write_is_tried_again_before_its_sync, a_run_closed_over_a_failed_write_writes_the_batch_into_its_segment, a_runs_first_write_that_failed_is_written_again |
| X4 run_attended: pending checked before owes (cipher) | review | a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again |
| X10 commit_direct: pending checked before attends | review | a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again |
| X5 answer_held: the paid check before the LOST check | review | survived run 1; run 2: a_marked_segments_failed_sync_is_every_later_syncs_answer (now asserts a sync with nothing new after the failed fdatasync is Err) |
| X6 run_again: `logged < len` half deleted | review | a_run_write_that_failed_under_a_cipher_is_written_again |
| X7 AlignedWriter::write breaks after defer | review | 5 tests, among them a_failed_run_piece_write_is_held_and_written_by_the_next_commit |
| X8 run_again's log-sync condition reduced to frames_unsynced | review | a_cipher_runs_batch_written_again_is_synced_in_a_log_file_a_rotation_left_loose (new; seam `wal_unsynced`) |
| X9 answer_held clones the held error instead of taking it | review | SURVIVED both runs. Observable only as a stale error's words: after a held failure is answered, `held_err` is read again only by `defer`'s `is_none` and overwritten by `lose`; a second deferral after an answered one needs a write between syncs on a stuck writer, which only `stream_hash`'s retry of a buffer a failed sync write left makes, and no test builds that with two distinguishable seams. Not built. |
| X11 sync_attended's mask narrowed to HELD | review | a_marked_segments_failed_sync_is_every_later_syncs_answer (the nothing-new sync after the failed fdatasync) |
| Y1 owe sets no OWED bit | own, deleted line | 10 tests |
| Y2 sync_attended does not pay (`paid()` deleted) | own, deleted line | 6 tests (the new `run_attends()` asserts) |
| Y3 paid `<=` -> `<` | own, flipped operator | 4 tests (the `run_attends()` asserts where the rescue owed exactly what it synced) |
| Y4 run_attended's pending check deleted | own, deleted line | survived run 1; run 2: a_cipher_runs_unlogged_batch_is_logged_by_a_commit_of_nothing_beside_an_armed_writer (new) |
| Y5 commit_direct does not ask attends | own, deleted line | 11 tests |

Survivors after round 3: X9 (above).

## Round 4 (review's extra mutants)

Runner `round4/mut5.py` (from round3/mut4.py; fixed copy round4/fixed/db.rs, the
round-3 tree), then `round4/mut6.py` against round4/fixed2/ (this round's tree)
for X9 and Z7 again. Same procedure: exact-string mutant of src/db.rs, build
`--test durable` under `checked`, the whole binary `--test-threads 2` under
`timeout 420`, the fixed copy put back byte for byte and compared. Diffs and logs
in round4/m/.

| mutant | source | killed by |
|---|---|---|
| Z1 defer: `held_err = Some(e)` unconditional (the review's items 1 and 8, the same edit) | review | a_marked_segments_failed_sync_is_every_later_syncs_answer (the syncs after a lost sync are asked the kept words after a batch whose writes the lost file refuses) |
| Z2a commit_direct: pending asked before attends (X10 again) | review | a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again |
| Z2b run_again: `logged < len` only, the pending half dropped | review | a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again |
| Z3 synced_all: helper error loaded Relaxed | review | SURVIVED, equivalent: the load reads the error code alone, and nothing is read through it; a test's visibility comes from the helper's counter (Release after the error's CAS) and the test thread's Acquire of it, which coherence carries to any later load on that thread, Relaxed included; in production there is no happens-before either way (the H1 race, ROADMAP). Ordering is not tested and is not load-bearing. |
| Z4 stream_copy: `stream_base = written - from` | review | a_run_closed_over_a_failed_write_writes_the_batch_into_its_segment (not a_buffered_runs_failed_first_write_..., whose marker retry empties the buffer before a copy) |
| Z5 write_buf_with: the `fault >= RUN_FAULT_WRITE` arm deleted | review | 14 tests; the finish's refusal after a lost sync: a_run_whose_fdatasync_failed_is_not_closed_into_a_named_segment under default ("the cut opens: ... truncated") and, run again with the test restricted to the copyvals arm (m/Z5-copyvals.log), under copyvals too; a_marked_segments_failed_sync_is_every_later_syncs_answer (the finish is Err) |
| Z6 run_again: `|| self.wal_dir_unsynced` dropped | review | SURVIVED, equivalent: `rotate_wal` sets the flag beside a push to `loose_wals` (loose) or pays it at once through `wal_dir_barrier` (chained), whose failure is kept and answered through `pending_err` before any call reaches `run_again`; `sync_wal` pays it before it empties `loose_wals`. So the half was a check nothing reaches; taken out of `run_again`, with a comment saying why. (Db::sync's and commit's copies of the triple are not this unit's lines; left.) |
| Z7 synced_all: `synced = written` and the helper's fetch_max above the error check | review | a_due_commit_is_not_acknowledged_over_a_failed_helper_sync; the Always variant missed it in round4/mut5 (its first answering commit fell inside its loop, before the count it compared was taken). It now holds the count at the first commit that answers Err, in the loop or after it, and kills Z7 in 3 runs of 3 (m/Z7-fixed-*.log). |
| Z8 sync_attended: `paid()` before `sync_with` | review | 6 tests, among them a_run_write_the_device_refuses_again_is_tried_once_a_sync (the twice seam) and a_run_write_that_failed_part_way_loses_no_commit_acknowledged_after_it |
| X9 (round 3's survivor) answer_held clones the held error | round 3 | a_run_write_failure_after_one_answered_is_answered_in_its_own_words (new: head refused twice, the sync answers it writing nothing, the next commit's try of the head fails part way, and the sync after must answer "after a partial write") |

Survivors after round 4: Z3 (equivalent, ordering not tested), Z6 (equivalent;
the check it mutated is gone).

## Round 5 (harness "repair round 3": the reviews' ten extra mutants)

Runner `round5/mut7.py` (from round4/mut6.py; fixed copy round5/fixed/db.rs, the
round-4 tree). Same procedure: exact-string mutant of src/db.rs, build `--test
durable` under `checked`, the whole binary `--test-threads 2` under `timeout 420`,
the fixed copy put back byte for byte and compared. Results round5/mut-run.log
(EXIT 0), diffs and logs round5/m/. The harness's items 2 and 6 are one edit (W2).

| mutant | source | killed by |
|---|---|---|
| W1 sync_with: a `synced_all` Err answered without `lose` | review 1 | a_commit_under_always_is_not_acknowledged_over_a_failed_helper_sync, a_due_commit_is_not_acknowledged_over_a_failed_helper_sync |
| W2 paid: `<=` -> `<` (Y3 again) | review 2, 6 | a_run_write_that_failed_is_written_again_by_a_commit_of_nothing, ..._by_a_sync, a_run_write_that_failed_part_way_loses_no_commit_acknowledged_after_it, a_run_write_the_device_refuses_again_is_tried_once_a_sync (the `run_attends() == Some(false)` asserts after the rescue) |
| W3 stream_copy: `held_from = to; held_to = to;` deleted | review 3 | SURVIVED, equivalent: the held range is empty on entry (stream_copy's debug assertion), every reader of it asks its length (`stream_held`, `pos`, `stream_flush`, `write_buf_with`, `read_at`) or offsets from `held_from` only where the length is not zero, and `stream_hash` restarts an empty range at its own `from`; nothing goes out twice since nothing is held. The two stores were taken out with a comment saying why (round5/fixed is the tree without them). |
| W4 run_write_fault: the seam dispatch above the LOST refusal | review 4 | a_marked_segments_failed_sync_is_every_later_syncs_answer, a_run_whose_fdatasync_failed_is_not_closed_into_a_named_segment |
| W5 stream_hash: the stream held ahead of a buffer a failed head left (no `stream_copy`) | review 5 (O2's shape) | a_buffered_runs_failed_first_write_is_tried_again_before_its_sync, a_run_closed_over_a_failed_write_writes_the_batch_into_its_segment, a_run_write_failure_after_one_answered_is_answered_in_its_own_words, a_runs_first_write_that_failed_is_written_again |
| W7 owe: `handed = self.written` | review 7 | 11 tests, among them a_run_write_that_failed_is_written_again_by_a_commit_of_nothing and a_failed_run_piece_write_is_held_and_written_by_the_next_commit |
| W8 answer_held: `>` -> `>=` (F2 again) | review 8 | a_write_that_failed_inside_a_sync_and_owed_exactly_what_it_synced_is_paid |
| W9 synced_all: the helper's `fetch_max` above the error check, early return kept | review 9 | SURVIVED, equivalent: the Err goes to `lose`, so RUN_ATTN_LOST refuses every later write and `written` never moves again; the helper's `synced` only gates whether it issues another fdatasync, whose error the CAS drops behind the first, and the helper already raises `synced` to `landed` after its own failed sync. Nothing a caller can read moves. |
| W10 run_write_fault: `seam < RUN_FAULT_WRITE` -> `<=` | review 10 | 16 tests, among them an_unmarked_segments_failed_write_is_answered_where_it_happens and a_run_write_that_failed_is_written_again_by_the_runs_next_commit |

Survivors after round 5: W3 (equivalent; the stores it deleted are gone), W9
(equivalent), and from earlier rounds Z3 (equivalent) and Z6 (equivalent; check gone).
