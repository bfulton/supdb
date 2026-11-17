# run-errors (run-write-fail, helper-presync-error H1): notes

Base: HEAD 00c1c218 (delete-max, log-errors committed on 3df85d22).
Uncommitted: src/db.rs, tests/durable.rs, CLAUDE.md, ROADMAP.md.

## What was wrong
- A direct run's writer wrote at the file's cursor and returned through
  `?` on any write failure. At the sync arm (the tail held for the sync),
  a failed or partial write left the batch counted (`d.committed`,
  `mem.commit()`), so a commit with nothing new and a `sync` answered Ok at
  `commit_direct`'s "nothing to commit" with no I/O; a partial write moved
  the cursor past `written`, so the next write landed past the stray bytes
  and the recovery walk lost it. At a piece written by the marker
  (`mark_stream`, or `AlignedWriter::write` at a boundary in the copy arms),
  a failure returned after the writer had taken the batch's keys, marker
  and CRC, so the next commit handed the same keys again and every commit
  after it was refused as "keys must arrive in strictly increasing order".
  The same for the run's first write (its head) in `stream_hash`.
- A failed fdatasync of the run's file was the commit's answer and then
  forgotten (no kept error); a write below the greatest then closed the run
  (finish + fsync of the same file, a retry) and the landing named it.
- H1: the helper thread's failed fdatasync was kept only for `finish`;
  `SegmentWriter::sync` (every run commit's sync) never read it.

## What changed (src/db.rs)
- `pwritev_all` (replaces `writev_all`): every write of `AlignedWriter`
  (stream pieces, the held tail with the buffer, the buffer) is positioned
  at `written` (`pwritev`, or `write_all_at` where the code wrote one
  slice), never the cursor. Same calls, pieces, offsets and sizes as
  before: the cursor equalled `written` on every success path (file
  created truncated, every write advanced both by the same amount) -- by
  reading, not by strace.
- `AlignedWriter::defers` (set by `set_marks(true)`, i.e. a run's writer):
  a write failure between two syncs is held, not answered (`defer`): the
  error kept in `held_err`, the bytes owed (`owed_to`), and the writer
  "stuck" -- `piece` set to `STUCK_PIECE` so nothing is written again
  before the next sync, which writes everything held as one write at
  `written`. Sites: `AlignedWriter::write`'s boundary write, `mark_stream`'s
  `stream_flush(false)`, `stream_hash`'s head write (with `stream_copy`,
  cold: the stream copied into the buffer behind the head, hashed into the
  row through `write`). A seal's/merge's writer (`defers` false) answers
  where it fails, as before.
- `SegmentWriter::sync`: first answers a held failure (`answer_held`) and
  writes nothing; a failed write/flush there marks the bytes owed
  (`owing`); a failed `sync_data` or a helper error marks `sync_lost`
  (`lose`), after which every sync and `into_inner` (every finish) answers
  it. `synced_all` reads the helper's error before counting anything as
  synced (H1), inside the `if let Some(h)` branch it already had.
- `SegmentWriter::owes` (`owed_to > synced`), asked inside `commit_direct`'s
  existing "nothing to commit" branch: `Db::run_again` (cold) syncs the
  run and, under a cipher, logs the batch (`log_run_again`) that the failed
  commit never logged. `Db::run_failed` (cold) turns a lost sync into the
  store's kept error (`keep_lost`, the log unit's), a write failure into
  the commit's answer.
- Seams (#[doc(hidden)] pub, no cfg): `Db::fail_next_run_write`,
  `fail_next_run_write_partial`, `fail_next_run_writes_twice`,
  `fail_next_run_sync` (on the open run's writer, or with none open on the
  next run's at its open), `run_synced_bytes`; `SegmentOptions::fail_helper_syncs`;
  `SegmentWriter::fail_next_write`, `fail_next_sync`. The fault is a byte
  on the writer asked beside the write (`>= RUN_FAULT_WRITE`, one compare)
  or the fdatasync it stands in for; the helper's `is_some` beside its
  fdatasync.
- Docs: SyncPolicy (a paragraph on the run), Db::commit, Db::sync,
  Db::flush, Db::open. CLAUDE.md: the "Crash discipline" paragraph's
  ordered-ingest window; "A sync whose error the next sync could not see"
  (came back at the helper and at the run's close); "A failure that left
  the buffer it failed on..." (came back at the run). ROADMAP: the run's
  file added to the dm-flakey item; a new item for the helper sync in
  flight when the commit reads its error.

## Success-path cost (rule 1) -- by reading the diff, not counted (as of round 2)
- `SegmentWriter::sync` (every run commit's sync): one compare of the writer's
  `fault` byte, the seam's own compare beside the fdatasync, standing for the
  seam, a held write failure and a lost sync (cold `sync_attended`). A writer
  with a helper adds one atomic load+branch inside `synced_all`'s existing
  `if let Some(h)` branch (none without a helper). The `?`s became `if let Err`
  over the same results.
- Every write of `AlignedWriter`: the seam's compare `fault >= RUN_FAULT_WRITE`
  beside the write syscall; held/lost states ride it (cold `run_write_fault`).
- Commit of nothing / sync / flush with nothing new for the run: two loads and
  a compare (`owes`) inside the existing "nothing to commit" branch.
- Segment finish (`into_inner`): no new check (a lost file is refused in the
  write's cold path).
- Run open: one take+store of the open seam byte; segment create: one clone of
  `fail_helper_syncs` (an Option, None outside tests).
- `pwritev` replaces `writev` one for one; the iovec Vec was there before.
- No syscall, wait, lock or allocation added on a success path. Not counted
  under callgrind.

## Tests (tests/durable.rs, section "run-errors")
Before the fix (before.log, seams in, fix not): 12 of the 13 first tests
failed at the assertion named; one passed.

| test | before | holds |
|---|---|---|
| a_run_write_that_failed_is_written_again_by_a_commit_of_nothing | "nothing-default, the power cut: ... k2", left [] | every arm, power cut and kill |
| ..._by_a_sync | "sync-default, the power cut: ... k2", left [] | |
| ..._by_the_runs_next_commit | PASSED before: not evidence | R2 etc. |
| a_run_write_that_failed_part_way_loses_no_commit_acknowledged_after_it | "partial-default, the power cut: ... k2", left [] | R1 |
| a_run_write_that_failed_part_way_is_flushed_whole | "default: the flush: ... the block table is not readable" | R1, every arm + latesync + finishinline |
| a_runs_first_write_that_failed_is_written_again | "default: the commit after: keys must arrive in strictly increasing order" | O2, O9 |
| a_failed_run_piece_write_is_held_and_written_by_the_next_commit | "default: the commit after the failed piece: keys must arrive..." | R6, O8 |
| a_failed_run_fdatasync_is_kept_until_the_store_is_reopened | "default: a commit of nothing after: answered Ok after the log's device error"; ends asserting the sync after is the kept error | R3, R5 |
| a_run_whose_fdatasync_failed_is_not_closed_into_a_named_segment | "default: the commit of the write that left: answered Ok ..." | O1 |
| a_run_write_that_failed_under_a_cipher_is_written_again | "cipher-nothing: the batch written again: k2", left [] | O6 |
| a_failed_run_fdatasync_is_kept_under_a_cipher | "a commit of nothing after: answered Ok ..." | |
| a_marked_segments_sync_reports_its_helpers_failed_sync | "a sync over records whose earlier fdatasync failed is not answered Ok" | R7 |
| a_due_commit_is_not_acknowledged_over_a_failed_helper_sync | "the policy's sync ... is not answered Ok" | R7, O7 |

Added after the fix, during the mutant pass (no before run; counted
through their mutants only): a_buffered_run_piece_write_that_failed_is_answered_and_written_by_a_sync
(R2b), a_run_write_the_device_refuses_again_is_tried_once_a_sync (O3),
a_marked_segments_failed_sync_is_every_later_syncs_answer (R4),
an_unmarked_segments_failed_write_is_answered_where_it_happens (O10),
a_commit_of_nothing_in_a_run_that_owes_nothing_syncs_nothing (O5).

Arms held (`run_arms`): default, direct_stream off (copyvals),
direct_stream + direct_put_one off, direct_table_refs (with the stream
off), direct_recycle off, direct_keys_in_stream off, direct_derive_keys
off, direct_slim_entries off; close_presync off and close_finish_inline on
for the flushed test. Cipher: write failure (three ways again) and sync
failure.

Skeptic's problems dealt with: the fdatasync test ends asserting the sync
is Err (the kept words); the (a) write test requires Ok and the batch back
(the reproduction's `promised` branch made strict); (b) requires the next
commit Ok (the store goes on) and holds every arm; RUN_FAULT consts sit
above AlignedWriter's doc; power cuts cut every name of the run's inode.

## Mutants
mutants.md: 37 mutant rows over three runners (report's 8, the reviews' 10,
19 own); all killed, O3 by a hang; E1 and E11 not run because round 1 took the
checks they mutate out.

## Restated tests
None. No existing test went red: the whole suite at the end of round 2
(round2/suite2.log) passed every binary, tests/db.rs's crash windows among
them (226); clippy --release --all-targets -D warnings and fmt.sh --check
exit 0.

## Encryption
- Kernel: the plain code path.
- Cipher: the run's file is a memory file through the same writer; a
  failed write is owed the same way and `run_again` logs the batch the
  failed commit left unlogged (tested, three ways). A failed fdatasync of
  the memory file is kept as in the clear (tested): that is conservative,
  since the log is a cipher run's durability and a memory file's sync is
  of nothing; it never fails in practice. The close of a cipher run whose
  sync was lost refuses at the finish, so no envelope is written over it.
- vault::persist / pager, by reading: no analogue of a positioned-write or
  owed-batch defect (persist rewrites a whole envelope from a fresh
  create; the pager issues no sync); the log unit read the same.

## Left out, and why (not_built)
- The helper's error read races a helper fdatasync in flight (its syscall
  returned, its store not yet made): the commit is acknowledged and the
  next sync answers. Closing it needs a wait on the helper or a count it
  raises before its sync -- a new atomic RMW per helper sync and a load
  per commit; ROADMAP item.
- P1/P2 of helper-presync-error (the close's presync) are the seal-sync
  unit's.
- A failed run fdatasync makes the run's close refuse at the finish, so
  the seal job fails before its readable landing and the seal queue
  wedges until reopen; a leave_direct after a kept error therefore leaves
  a frozen table nothing lands. Reads go on; reopen recovers the run's
  file. Not tested beyond the reopen.
- A stuck writer (after a held write failure) writes once a sync for the
  rest of the run, one write longer than a piece: folios differ after a
  device error only.

## Not verified
- Real errseq/ENOSPC behaviour (seams only); ROADMAP.
- That the syscall pattern on the success path is identical (by reading;
  no strace comparison against the base).
- No callgrind count, no timing.
- The H1 race above.


## Repair round 1 (review-r1.json), as finished after the container restart

State at the restart, checked: `cmp src/db.rs round1/keep/db.rs` equal (no
mutant left in the tree); `diff tests/durable.rs round1/keep/durable.rs` showed
only rustfmt's reflows (the format step ran after keep was taken).

What round 1 did about the reviews (from the diff and its mutant runs):
- Blocking 1 (cost: two checks not folded). Folded: the held write failure and
  the lost sync are bits of the writer's `fault` byte (`RUN_ATTN_HELD` 0x40,
  `RUN_ATTN_LOST` 0x80) beside the seam's `RUN_FAULT_*` values.
  `SegmentWriter::sync` asks `fault != 0` once (the seam's own compare) and
  goes to the cold `sync_attended`; `into_inner` has no check of its own: a
  lost file's writes are refused in the write's cold path (`run_write_fault`,
  reached by the seam compare `pwritev_all` and `write_buf_with` already make),
  and a finish always has its trailer to write. The `owe` floor
  (`.max(synced + 1)`) and `lose`'s `owe()` were taken out as checks nothing
  reaches.
- Blocking 2 (H1 at the Db level under Always): a_commit_under_always_is_not_acknowledged_over_a_failed_helper_sync.
  Failed before round 1's code (round1/before.log: "a commit's sync, after the
  run's records failed theirs, is not answered Ok").
- Blocking 3 (closing a stuck writer): a_run_closed_over_a_failed_write_writes_the_batch_into_its_segment,
  run_arms x {Always, EveryN(MAX)} x {head, piece, tail}. Before round 1's fix it
  failed ("always-Piece-default: the commit that closed the run: direct finish:
  segment writer: the key source holds a different count of keys than were
  written"; "buf-Piece-default: a commit the policy does not sync: the run's
  write failed (injected)"). The SyncPolicy doc says what a close does with a
  held failure.
- Tests round 1 added beside those: a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again
  (E5), a_run_written_again_restarts_the_policys_count (E6).
- Extra mutants: all run (round1/mut-results-run2.json); E7 and E8 survived.

What this session did to finish the round (round2/):
- E7 (a stuck writer's head not retried): the retry is behaviour the `defer`
  doc names and keeps a transient failure from buffering the run until its
  sync; held by a_buffered_runs_failed_first_write_is_tried_again_before_its_sync
  (stream arms; the retry happens at the same commit's marker, so the file holds
  the head after the commit whose head write failed). Kills E7. Not a before:
  it holds existing behaviour.
- E8 (held check moved below the batch_buf write): showed a real wrinkle. A
  write failure met inside a sync (the records a sync writes ahead of their
  marker crossing a piece) left the held bit set after that sync wrote and
  synced every byte; the sync after answered it without writing its own batch,
  and `owes()` read false. Reachable only through the public SegmentWriter API
  (every Db commit marks first), but fixed: `answer_held` returns `None` when
  `owed_to <= synced` (paid), cold. Tests:
  a_marked_segments_write_that_failed_inside_a_sync_is_paid_by_that_sync (fails
  under F1 = the fix undone, and under E8) and
  a_write_that_failed_inside_a_sync_and_owed_exactly_what_it_synced_is_paid
  (records ending at a piece's boundary; kills F2, `>` -> `>=`).
- Cipher nonblocking item (review cost r1): under Encryption::Cipher with
  EveryN, a commit whose run write was deferred logged its batch unsynced, and
  the rescue (`run_again`) synced the memory file and skipped the log, so its
  `Ok` stood over a batch whose one durable form was unsynced. Fixed in
  `run_again` (cold): where the batch is logged, the log is synced if it has
  unsynced frames, loose files or a directory to sync (Db::sync's own test).
  Test a_cipher_runs_batch_written_again_by_a_commit_of_nothing_is_synced_in_its_log
  failed before (round2/before-cipherlog.log: "the batch written again:
  a0000001 reads back once", left []); passes after; G1/G2 killed. Its reads
  put the batch first because `Db::sync` under a cipher does not sync the log
  for an ordered table (the sync-ordered-buffered unit's defect), so the key
  committed and "synced" before the failure is durable only through this sync.
- ROADMAP: the H1 race item names the cheaper shape to price (the helper's own
  open file description, so errseq reports the error to the writer's fdatasync
  too).
- CLAUDE.md: one clause in the run's entry (a failure met inside a sync that
  wrote and synced every byte is paid by that sync).
- Whole suite (round2/suite2.log, --profile checked --no-fail-fast, own
  TMPDIR): exit 0, 20 binaries, 937 passed, 0 failed (lib 404, db 226, durable
  66, cipher 24, zpages 136, segwriter 19, fuzz 5, ...). Then one clippy idiom
  in a new test (`is_multiple_of`), durable rerun 66 passed; clippy --release
  --all-targets -D warnings exit 0; fmt.sh --check exit 0.

## Nonblocking review items, as they stand
- stream_hash retry on a stuck writer: doc of `defer` names it; tested (E7).
- Cipher EveryN log unsynced behind run_again's Ok: fixed (above). `Db::sync`
  under a cipher with an ordered table still skips the log: the
  sync-ordered-buffered unit's; hand-off.
- H1 race: not built (rule 1); ROADMAP item, with the cheaper shape.
- Seam costs at run open (`mem::take` of `run_fault_open`, a load and a store
  once a run) and `fail_helper` clone per segment create: left; once per run /
  per segment, beside a file create.
- `lose` keeps a copy of the error by kind and text, so later answers lose
  `raw_os_error`; the Db's kept error is built from the original. Left.
- Db::commit says "where its policy syncs", SyncPolicy "whatever the policy":
  both true (a commit with a new batch under a policy that does not sync
  answers Ok unsynced; a call with nothing new syncs whatever the policy).
- pwritev is Linux/unix libc; db.rs already uses libc unconditionally in the
  segment writer (start_writeback is cfg'd, pwritev is not): not checked on
  another platform.
- `SegmentOptions::fail_helper_syncs` is a new pub field; `SegmentWriter::finish`
  refuses after a failed sync: not written into SegmentWriter's crate docs.
- RunMap/grow_run_map failure in commit_direct still wedges with the ordering
  error: outside this unit's defects.
- Pages dropped after a real writeback EIO read through the run's mapping:
  not testable with seams; not in ROADMAP.
- O3 (not stuck) is killed by a hang (an infinite loop in AlignedWriter::write),
  not an assertion.

## Not verified (as of round 2)
- No callgrind count of the success path (owed: load_only.rs, work_probe.rs by
  function against origin/main); the success-path cost is by reading.
- No strace comparison that pwritev at `written` makes the same calls, pieces
  and offsets as the cursor writev did (by reading only).
- Real errseq/ENOSPC behaviour (seams only).
- The H1 race.

## Repair round 3 (this harness round: blocking owes() cost; 11 extra mutants)
Started. keep = round3/keep (db.rs, durable.rs, CLAUDE.md, ROADMAP.md at round start).
- owes folded into fault byte (RUN_ATTN_OWED 0x20; owe sets, sync_attended clears via paid(); commit_direct asks attends() then cold run_attended). New seams run_attends, short_next_run_write (RUN_FAULT_SHORT, pwritev_loop shared), wal_unsynced. New tests: short write, loose rotation cipher; cipher arm in run-closed test; synced-unchanged assert in a_failed_run_sync_is_kept. Fixed copy round3/fixed. Next: mutants (round3/mut4.py).
### Round 3 detail
- Blocking (owes() cost): folded. `AlignedWriter::owe` sets RUN_ATTN_OWED (0x20) in
  the `fault` byte; `SegmentWriter::sync_attended` clears it (`paid`) after
  `sync_with` returns Ok, when owed_to <= synced. `answer_held` does NOT call
  paid: a None from it falls through to sync_with in sync_attended, which pays;
  a call there was a check nothing could reach. `synced_all` (hot) unchanged.
  commit_direct's nothing-new return asks `attends()` (fault != 0, one byte load
  + branch beside the old loads) and goes to cold `run_attended`, which asks
  `owes()` and otherwise does the old pending/Ok path (O5 holds). A stale OWED
  bit (synced moved past owed_to by the sync_every path at 7951, which no run
  writer is known to take) costs only the cold path; behaviour exact via owes().
- Seams: `Db::run_attends()` (the bit is observable: asserted false after the
  rescue in a_run_write_that_failed and the buffered-piece test),
  `Db::short_next_run_write()` (RUN_FAULT_SHORT=5: pwritev_at's loop now
  `pwritev_loop(at, parts, call)`, the hot caller hands the libc syscall in an
  inline closure, the seam hands one that is short once), `Db::wal_unsynced()`.
- New tests: a_short_run_write_is_written_whole_at_the_offset_it_reached (tail
  and piece, run_arms; for X1); a_cipher_runs_batch_written_again_is_synced_in_a_log_file_a_rotation_left_loose
  (seal_bytes 3_300_000 so the log passes it and the table does not; for X8);
  a_failed_run_sync_is_kept asserts run_synced_bytes unchanged over the failed
  fdatasync (X2); a_run_closed_over_a_failed_write_... gains a cipher arm (rule 3;
  premise checks moved from the temp file in the directory to run_synced_bytes,
  the kill end skipped under a cipher, where the store stays held in-process).
  None of these failed before (they hold behaviour already there); counted
  through their mutants only.
- Observed, not changed: a commit that crosses seal_bytes while the run owes a
  held failure answers that failure (Db::commit -> seal -> commit_direct ->
  run_again -> answer_held) and the run does not close at it; the batch stays
  visible and owed, and the next call writes it. Conservative; not a lost ack.
- Success-path cost as of round 3 (by reading, not counted): commit of nothing /
  sync / flush with nothing new for a run: one byte load of `fault` and a branch
  (`attends`), replacing the two u64 loads and compare of `owes`. pwritev_at's
  body now goes through `pwritev_loop` with an inline closure (both
  #[inline(always)]); the same syscalls, offsets and sizes; the generated code
  was not compared.
- Mutants round 3: mutants.md, section "Round 3". Survivor X9 (clone vs take of
  the held error; stale words only through a path no test builds).
- Tests added in round 3 after the first mutant run:
  a_cipher_runs_unlogged_batch_is_logged_by_a_commit_of_nothing_beside_an_armed_writer
  (Y4), and a nothing-new sync assert in a_marked_segments_failed_sync_is_every_later_syncs_answer (X5, X11).
- fmt.sh apply + --check exit 0. Whole suite + clippy: round3/suite.log, clippy.log.
- Round 3 finished: suite exit 0 (20 binaries, durable 69, db 226), clippy exit 0; TMPDIR removed.

## Repair round 4 (harness: 9 extra mutants, no blocking)
Started. keep = round4/keep (db.rs, durable.rs, CLAUDE.md, ROADMAP.md at round start = round3/fixed).
- Mutant runner round4/mut5.py (from round3/mut4.py; fixed copy round4/fixed/db.rs). Z1 (defer overwrites, = extra items 1 and 8) killed; Z2a, Z2b killed. Results land in round4/mut-run.log.
- Z6 (run_again's dir half) reasoned equivalent before its run: rotate_wal sets wal_dir_unsynced with a push to loose_wals (loose) or calls the barrier at once (clears it, or keep_lost, after which commit/sync/flush answer pending_err before run_again); sync_wal clears the dir before it empties loose_wals. Plan: take the half out of run_again with that comment, once the runner is done.
- Runner done (EXIT 0, tree cmp-equal to round4/fixed). Killed: Z1, Z2a, Z2b, Z4 (by the close test), Z5 (14 tests), Z7 (a_due_commit only: the Always variant missed it), Z8 (6 tests, the twice test among them). Survived: Z3 (Relaxed: equivalent, the load reads the code alone), Z6 (equivalent, as reasoned).
- Next: (a) run_again drops the dir half; (b) the Always helper test asserts synced unchanged at the first commit that answers Err, inside its loop; (c) Z5 once more with the leave test restricted to copyvals.
- Done: run_again's dir half out (comment says why); Always helper test asserts synced at the first Err in its loop (first draft compared None with Some(0) when the first commit erred: the run opens at it; fixed with unwrap_or(0)); Z5 under copyvals killed (m/Z5-copyvals.log); Z7 against the new test killed 3 of 3 (both helper tests). New test a_run_write_failure_after_one_answered_is_answered_in_its_own_words (for X9). fixed2 = current tree. Next: durable run on fixed2, then X9 and Z7 on it.
- durable on fixed2: 70 passed (round4/after-durable2.log). X9 now killed by a_run_write_failure_after_one_answered_is_answered_in_its_own_words. Z7 x3 on fixed2 running.
- fmt apply/check exit 0 (no change). Whole suite + clippy running: round4/suite.log, round4/clippy.log. mutants.md has the Round 4 section.
- Round 4 finished: whole suite (round4/suite.log, --profile checked --no-fail-fast, own TMPDIR) SUITE EXIT 0, 20 binaries, 941 passed, 0 failed (db 226, durable 70); clippy --release --all-targets -D warnings exit 0; fmt.sh --check exit 0. Tree cmp-equal to round4/fixed2. TMPDIR removed.
- Round 4 changes: src/db.rs run_again's log-sync condition drops `|| self.wal_dir_unsynced` (equivalent; comment says why). tests/durable.rs: the Always helper test holds the synced count at the first Err commit wherever it falls; new test a_run_write_failure_after_one_answered_is_answered_in_its_own_words. No CLAUDE.md/ROADMAP change this round (no new shape; the docs' contracts unchanged).
- Not run this round: callgrind, strace (as before).

## Repair round 5 (harness "repair round 3": 10 extra mutants, no blocking)
Started. keep = round5/keep (= round4/fixed2 + docs as at round start). Runner round5/mut7.py; fixed copy round5/fixed/db.rs. Mutants W1..W10 map to the harness's items in order (items 2 and 6 are one edit, W2).
- W1 killed: a_commit_under_always_..., a_due_commit_... (round5/m/W1-*.log)
- Runner done EXIT 0, tree restored. Killed W1 W2 W4 W5 W7 W8 W10. Survived W3 (dead stores: held range empty by assert, readers ask its length, stream_hash resets) -> stores taken out with comment, round5/fixed updated; W9 equivalent (LOST stops writes; helper already fetch_max(landed) after its own failed sync). Next: durable + neighbours, fmt, clippy, docs.
- Round 5 finished: fmt.sh apply/check exit 0; whole suite (round5/suite.log, --profile checked --no-fail-fast, own TMPDIR) SUITE EXIT 0, 20 binaries, 941 passed, 0 failed; clippy --release --all-targets -D warnings exit 0. Tree = round5/fixed. TMPDIR removed.
- Round 5 changes: src/db.rs stream_copy's two dead stores of the held range out (comment). No test, CLAUDE.md or ROADMAP change (no new shape, no contract moved). mutants.md has the Round 5 section.
